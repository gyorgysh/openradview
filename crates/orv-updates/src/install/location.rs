// SPDX-License-Identifier: GPL-3.0-or-later
//! Conservative local ownership checks. Never invoke or modify a package manager.
use crate::protocol::Result;
use std::{ffi::OsString, path::Path};
#[cfg(test)]
mod tests;

pub(super) fn check(root: &Path) -> Result<()> {
    policy(root, |key| std::env::var_os(key))
}

pub(crate) fn policy(root: &Path, env: impl Fn(&str) -> Option<OsString>) -> Result<()> {
    let canonical = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let root = canonical.as_path();
    let managed = || {
        Err(
            "This installation is managed externally. Update it with its package manager or install a separate app copy.",
        )
    };
    // These describe the running container/image, unlike a globally exported
    // HOMEBREW_PREFIX or SCOOP variable, which alone says nothing about this app.
    if ["FLATPAK_ID", "SNAP", "APPIMAGE"]
        .iter()
        .any(|key| env(key).is_some_and(|v| !v.is_empty()))
        || Path::new("/.flatpak-info").exists()
    {
        return managed();
    }
    for prefix in [
        "/nix/store",
        "/snap",
        "/var/lib/snapd/snap",
        "/var/lib/flatpak/app",
    ] {
        if root.starts_with(prefix) {
            return managed();
        }
    }
    // Canonical executable paths resolve Homebrew opt links and Scoop junctions.
    // Check whole components, never substrings in an ordinary user directory.
    if root
        .components()
        .any(|part| matches!(part.as_os_str().to_str(), Some("Cellar" | "Caskroom")))
    {
        return managed();
    }
    for key in [
        "NIX_STORE_DIR",
        "SCOOP",
        "SCOOP_GLOBAL",
        "ChocolateyInstall",
    ] {
        if let Some(prefix) = env(key).filter(|p| !p.is_empty()) {
            let prefix = Path::new(&prefix);
            if prefix.is_absolute()
                && root.starts_with(prefix.canonicalize().as_deref().unwrap_or(prefix))
            {
                return managed();
            }
        }
    }
    if let Some(home) = env("HOME").filter(|p| !p.is_empty()) {
        let home = Path::new(&home);
        if home.is_absolute() && root.starts_with(home.join(".local/share/flatpak/app")) {
            return managed();
        }
    }
    // Scoop's renamed receipts also recognize custom installs without SCOOP in
    // the environment. Legacy generic filenames need the paired apps layout.
    for ancestor in root.ancestors().take(4) {
        if ancestor.join("scoop-install.json").exists()
            || ancestor.join("scoop-manifest.json").exists()
            || ancestor.join("INSTALL_RECEIPT.json").exists()
            || (ancestor.join("install.json").exists()
                && ancestor.join("manifest.json").exists()
                && ancestor
                    .parent()
                    .and_then(Path::parent)
                    .and_then(Path::file_name)
                    .is_some_and(|n| n == "apps"))
        {
            return managed();
        }
    }
    if root.join(".openradview-no-self-update").exists() {
        return managed();
    }
    Ok(())
}
