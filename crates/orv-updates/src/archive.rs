// SPDX-License-Identifier: GPL-3.0-or-later
use crate::protocol::{MAX_EXPANDED, MAX_FILES, Result};
use serde::{Deserialize, Serialize};
#[cfg(any(test, not(target_os = "macos")))]
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs,
    io::{Read, Seek, SeekFrom, Write},
    path::{Component, Path, PathBuf},
};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Layout {
    pub schema_version: u32,
    pub product: String,
    pub target: String,
    pub files: Vec<String>,
}

#[cfg(any(test, not(target_os = "macos")))]
#[derive(Deserialize)]
struct BuildInventory {
    version: String,
    target: String,
    files: std::collections::BTreeMap<String, String>,
}

/// Compare portable managed files with the local package inventory before
/// replacing or deleting them. The inventory is not a signature on its own.
#[cfg(any(test, not(target_os = "macos")))]
pub(crate) fn validate_installed_content(
    root: &Path,
    target: &str,
    version: Option<&str>,
    cancelled: impl Fn() -> bool,
) -> Result<()> {
    const CHANGED: &str = "The installed app contains modified files or an invalid inventory. Update manually to preserve your files.";
    validate_layout(root, target)?;
    let build_path = root.join("build.json");
    let metadata = fs::symlink_metadata(&build_path).map_err(|_| CHANGED)?;
    if !metadata.is_file() || metadata.len() > 1024 * 1024 {
        return Err(CHANGED);
    }
    let mut bytes = Vec::new();
    fs::File::open(&build_path)
        .map_err(|_| CHANGED)?
        .take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| CHANGED)?;
    if bytes.len() > 1024 * 1024 {
        return Err(CHANGED);
    }
    let build: BuildInventory = serde_json::from_slice(&bytes).map_err(|_| CHANGED)?;
    if build.target != target
        || version.is_some_and(|expected| expected != build.version)
        || crate::protocol::stable_version(&build.version).is_err()
    {
        return Err(CHANGED);
    }
    let layout: Layout = serde_json::from_slice(&fs::read(layout_path(root)).map_err(|_| CHANGED)?)
        .map_err(|_| CHANGED)?;
    let expected: BTreeSet<_> = layout
        .files
        .into_iter()
        .filter(|name| name != "build.json")
        .collect();
    if !root.join("build.json").is_file()
        || build.files.keys().cloned().collect::<BTreeSet<_>>() != expected
    {
        return Err(CHANGED);
    }
    let mut total = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    for (name, expected_hash) in build.files {
        if cancelled() {
            return Err("Installation was canceled because the app is closing.");
        }
        if expected_hash.len() != 64
            || !expected_hash
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(CHANGED);
        }
        let path = root.join(relative(&name)?);
        if !fs::symlink_metadata(&path).map_err(|_| CHANGED)?.is_file() {
            return Err(CHANGED);
        }
        let mut input = fs::File::open(path).map_err(|_| CHANGED)?;
        let mut hash = Sha256::new();
        loop {
            if cancelled() {
                return Err("Installation was canceled because the app is closing.");
            }
            let read = input.read(&mut buffer).map_err(|_| CHANGED)?;
            if read == 0 {
                break;
            }
            total = total
                .checked_add(read as u64)
                .filter(|n| *n <= MAX_EXPANDED)
                .ok_or(CHANGED)?;
            hash.update(&buffer[..read]);
        }
        let actual: String = hash
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        if actual != expected_hash {
            return Err(CHANGED);
        }
    }
    Ok(())
}
pub(crate) fn relative(value: &str) -> Result<PathBuf> {
    if value.is_empty()
        || value.len() > 240
        || value.contains(['\\', ':', '\0'])
        || value.ends_with([' ', '.'])
    {
        return Err("The update contains an unsafe path.");
    }
    let path = Path::new(value);
    if !path.components().all(|c| matches!(c, Component::Normal(_)))
        || path.components().any(|c| {
            let text = c.as_os_str().to_string_lossy();
            let stem = text.split('.').next().unwrap_or("").to_ascii_uppercase();
            text.ends_with([' ', '.'])
                || matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
                || (stem.len() == 4
                    && (stem.starts_with("COM") || stem.starts_with("LPT"))
                    && stem.as_bytes()[3].is_ascii_digit())
        })
    {
        return Err("The update contains an unsafe path.");
    }
    Ok(path.to_path_buf())
}
fn copy_entry(
    root: &Path,
    name: &str,
    size: u64,
    executable: bool,
    content: impl Read,
    seen: &mut BTreeSet<String>,
    expanded: &mut u64,
) -> Result<()> {
    let path = relative(name)?;
    if seen.len() >= MAX_FILES || !seen.insert(name.to_ascii_lowercase()) {
        return Err("The update contains too many or duplicate files.");
    }
    *expanded = expanded
        .checked_add(size)
        .filter(|n| *n <= MAX_EXPANDED)
        .ok_or("The expanded update exceeds its size limit.")?;
    let destination = root.join(path);
    fs::create_dir_all(destination.parent().ok_or("Invalid update directory.")?)
        .map_err(|_| "Cannot create the staging directory.")?;
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&destination)
        .map_err(|_| "Cannot stage an update file.")?;
    let n = std::io::copy(&mut content.take(size + 1), &mut output)
        .map_err(|_| "Cannot unpack the update.")?;
    if n != size {
        return Err("The update contains a truncated file.");
    }
    output
        .flush()
        .map_err(|_| "Cannot finish staging the update.")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            destination,
            fs::Permissions::from_mode(if executable { 0o755 } else { 0o644 }),
        )
        .map_err(|_| "Cannot set staged file permissions.")?;
    }
    #[cfg(not(unix))]
    let _ = executable;
    Ok(())
}
pub(crate) fn unpack(archive: &Path, destination: &Path) -> Result<()> {
    let mut input = fs::File::open(archive).map_err(|_| "Cannot open the verified update.")?;
    let mut seen = BTreeSet::new();
    let mut expanded = 0;
    if archive.extension().is_some_and(|e| e == "zip") {
        zip_directory_limit(&mut input)?;
        let mut zip = zip::ZipArchive::new(input).map_err(|_| "Invalid update ZIP.")?;
        if zip.len() > MAX_FILES {
            return Err("Too many ZIP entries.");
        }
        for i in 0..zip.len() {
            let mut entry = zip.by_index(i).map_err(|_| "Invalid update ZIP entry.")?;
            relative(
                entry
                    .name()
                    .map_err(|_| "Invalid ZIP filename.")?
                    .trim_end_matches('/'),
            )?;
            let kind = entry.unix_mode().unwrap_or(0) & 0o170000;
            if entry.is_dir() {
                if !matches!(kind, 0 | 0o040000) {
                    return Err("Invalid ZIP directory.");
                }
                continue;
            }
            if !matches!(kind, 0 | 0o100000) || entry.encrypted() {
                return Err("Update links and special files are not allowed.");
            }
            let name = entry
                .name()
                .map_err(|_| "Invalid ZIP filename.")?
                .into_owned();
            let size = entry.size();
            let executable = entry.unix_mode().unwrap_or(0) & 0o111 != 0;
            copy_entry(
                destination,
                &name,
                size,
                executable,
                &mut entry,
                &mut seen,
                &mut expanded,
            )?;
        }
    } else {
        let input = flate2::read::GzDecoder::new(input).take(MAX_EXPANDED + 16 * 1024 * 1024);
        let mut tar = tar::Archive::new(input);
        let entries = tar.entries().map_err(|_| "Invalid update TAR.")?.raw(true);
        let mut count = 0;
        for entry in entries {
            count += 1;
            if count > MAX_FILES {
                return Err("Too many TAR entries.");
            }
            let mut entry = entry.map_err(|_| "Invalid update TAR entry.")?;
            let name = entry
                .path()
                .map_err(|_| "Invalid TAR path.")?
                .to_str()
                .ok_or("Invalid TAR path encoding.")?
                .to_owned();
            relative(name.trim_end_matches('/'))?;
            let kind = entry.header().entry_type();
            if kind.is_dir() {
                continue;
            }
            if !kind.is_file() {
                return Err("Update links and special files are not allowed.");
            }
            let size = entry.size();
            let executable = entry
                .header()
                .mode()
                .map_err(|_| "Invalid TAR permissions.")?
                & 0o111
                != 0;
            copy_entry(
                destination,
                &name,
                size,
                executable,
                &mut entry,
                &mut seen,
                &mut expanded,
            )?;
        }
    }
    if seen.is_empty() {
        return Err("The update archive is empty.");
    }
    Ok(())
}
pub(crate) fn layout_path(root: &Path) -> PathBuf {
    if cfg!(target_os = "macos") {
        root.join("Contents/Resources/update-layout.json")
    } else {
        root.join("update-layout.json")
    }
}
pub(crate) fn validate_layout(root: &Path, target: &str) -> Result<()> {
    let path = layout_path(root);
    let metadata = fs::symlink_metadata(&path).map_err(
        |_| "This installation needs a manual update before in-app installation is available.",
    )?;
    if !metadata.is_file() || metadata.len() > 1024 * 1024 {
        return Err("Invalid managed installation layout.");
    }
    let layout: Layout =
        serde_json::from_slice(&fs::read(path).map_err(|_| "Cannot read installation layout.")?)
            .map_err(|_| "Invalid installation layout.")?;
    if layout.schema_version != 1
        || layout.product != "OpenRadView"
        || layout.target != target
        || layout.files.is_empty()
        || layout.files.len() > MAX_FILES
    {
        return Err("Invalid managed installation layout.");
    }
    let mut expected = BTreeSet::new();
    for name in layout.files {
        relative(&name)?;
        if !expected.insert(name) {
            return Err("Duplicate installation layout entry.");
        }
    }
    let mut actual = BTreeSet::new();
    let mut pending = vec![root.to_path_buf()];
    let mut dirs = 0;
    while let Some(dir) = pending.pop() {
        dirs += 1;
        if dirs > MAX_FILES {
            return Err("Too many installation directories.");
        }
        if fs::symlink_metadata(&dir)
            .map_err(|_| "Cannot inspect installation.")?
            .file_type()
            .is_symlink()
        {
            return Err("Linked installation directories need a manual update.");
        }
        for entry in fs::read_dir(&dir).map_err(|_| "Cannot inspect installation.")? {
            let entry = entry.map_err(|_| "Cannot inspect installation.")?;
            let kind = entry
                .file_type()
                .map_err(|_| "Cannot inspect installation.")?;
            if kind.is_dir() {
                pending.push(entry.path());
            } else if kind.is_file() {
                if actual.len() >= MAX_FILES {
                    return Err("Too many installation files.");
                }
                actual.insert(
                    entry
                        .path()
                        .strip_prefix(root)
                        .map_err(|_| "Invalid installation path.")?
                        .to_str()
                        .ok_or("Invalid installation path encoding.")?
                        .replace('\\', "/"),
                );
            } else {
                return Err("Linked or special installation files need a manual update.");
            }
        }
    }
    // Stapler adds this reserved ticket after the bundle was sealed. It is
    // assessed by macOS before installation and is never an arbitrary extra file.
    if cfg!(target_os = "macos") {
        actual.remove("Contents/CodeResources");
        expected.remove("Contents/CodeResources");
    }
    if actual != expected {
        return Err(
            "This app folder contains added or missing files. Install the update manually to preserve your files.",
        );
    }
    Ok(())
}

// App archives are far below ZIP64 limits. Check counts and central-directory
// bytes before ZipArchive allocates entry metadata. Study ZIP64 is unrelated.
fn zip_directory_limit(input: &mut fs::File) -> Result<()> {
    let size = input
        .metadata()
        .map_err(|_| "Cannot inspect the update ZIP.")?
        .len();
    let start = size.saturating_sub(65557);
    input
        .seek(SeekFrom::Start(start))
        .map_err(|_| "Cannot inspect the update ZIP.")?;
    let mut tail = Vec::new();
    input
        .take(65557)
        .read_to_end(&mut tail)
        .map_err(|_| "Cannot inspect the update ZIP.")?;
    let at = (0..tail.len().saturating_sub(21))
        .rev()
        .find(|i| {
            tail[*i..].starts_with(b"PK\x05\x06")
                && *i + 22 + u16::from_le_bytes([tail[*i + 20], tail[*i + 21]]) as usize
                    == tail.len()
        })
        .ok_or("Missing ZIP directory record.")?;
    let u16_at = |i| u16::from_le_bytes([tail[at + i], tail[at + i + 1]]);
    let u32_at = |i| {
        u32::from_le_bytes([
            tail[at + i],
            tail[at + i + 1],
            tail[at + i + 2],
            tail[at + i + 3],
        ])
    };
    if u16_at(4) != 0
        || u16_at(6) != 0
        || u16_at(8) != u16_at(10)
        || u16_at(10) == 0
        || usize::from(u16_at(10)) > MAX_FILES
        || u32_at(12) > 8 * 1024 * 1024
        || u64::from(u32_at(16)).checked_add(u64::from(u32_at(12))) != Some(start + at as u64)
    {
        return Err("The update ZIP directory exceeds its limits or uses an unsupported layout.");
    }
    input
        .seek(SeekFrom::Start(0))
        .map_err(|_| "Cannot rewind the update ZIP.")?;
    Ok(())
}
