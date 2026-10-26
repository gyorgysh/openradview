// SPDX-License-Identifier: GPL-3.0-or-later
use crate::protocol::Result;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Preferences {
    pub chosen: bool,
    pub enabled: bool,
    pub last_check: u64,
    #[serde(default)]
    pub cleanup: Option<Cleanup>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Cleanup {
    pub backup: PathBuf,
    pub installed_version: String,
}
pub(crate) fn location() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    let folder =
        PathBuf::from(std::env::var_os("HOME")?).join("Library/Application Support/OpenRadView");
    #[cfg(target_os = "windows")]
    let folder = PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("OpenRadView");
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let folder = match std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
    {
        Some(path) => path.join("openradview"),
        None => PathBuf::from(std::env::var_os("HOME")?).join(".config/openradview"),
    };
    if !folder.is_absolute() {
        return None;
    }
    Some(folder.join("updates.json"))
}
pub(crate) fn load(path: &Path) -> Preferences {
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if !metadata.is_file() || metadata.len() > 8192 {
            return Preferences::default();
        }
        return fs::read(path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
    }
    #[cfg(all(target_os = "macos", not(test)))]
    {
        use objc2_foundation::{NSUserDefaults, ns_string};
        let defaults = NSUserDefaults::standardUserDefaults();
        // Preserve an existing Sparkle choice, including an opt-out. This reads
        // app preferences only; never Keychain or signing credentials.
        if !defaults.boolForKey(ns_string!("ORVSharedUpdaterMigrated"))
            && defaults.boolForKey(ns_string!("ORVUpdatePreferenceChosen"))
        {
            return Preferences {
                chosen: true,
                enabled: defaults.boolForKey(ns_string!("SUEnableAutomaticChecks")),
                ..Default::default()
            };
        }
    }
    Preferences::default()
}
pub(crate) fn save(path: &Path, preferences: &Preferences) -> Result<()> {
    let parent = path.parent().ok_or("Cannot locate update preferences.")?;
    fs::create_dir_all(parent).map_err(|_| "Cannot create update preferences.")?;
    let mut file =
        tempfile::NamedTempFile::new_in(parent).map_err(|_| "Cannot save update preferences.")?;
    let bytes = serde_json::to_vec(preferences).map_err(|_| "Cannot encode update preferences.")?;
    file.write_all(&bytes)
        .map_err(|_| "Cannot save update preferences.")?;
    file.as_file()
        .sync_all()
        .map_err(|_| "Cannot finish saving update preferences.")?;
    file.persist(path)
        .map_err(|_| "Cannot replace update preferences.")?;
    #[cfg(all(target_os = "macos", not(test)))]
    {
        use objc2_foundation::{NSUserDefaults, ns_string};
        NSUserDefaults::standardUserDefaults()
            .setBool_forKey(true, ns_string!("ORVSharedUpdaterMigrated"));
    }
    Ok(())
}
