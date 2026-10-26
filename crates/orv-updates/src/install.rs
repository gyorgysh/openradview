// SPDX-License-Identifier: GPL-3.0-or-later
mod location;
use crate::{
    archive,
    protocol::{Manifest, Result, TARGET},
};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

pub(crate) fn executable_relative() -> &'static str {
    if cfg!(target_os = "macos") {
        "Contents/MacOS/OpenRadView"
    } else if cfg!(target_os = "windows") {
        "openradview.exe"
    } else {
        "openradview"
    }
}
pub(crate) fn companion_relative() -> &'static str {
    if cfg!(target_os = "macos") {
        "Contents/MacOS/openradview-mcp"
    } else if cfg!(target_os = "windows") {
        "openradview-mcp.exe"
    } else {
        "openradview-mcp"
    }
}
pub(crate) fn root(executable: &Path) -> Result<PathBuf> {
    let mut root = executable
        .parent()
        .ok_or("Cannot locate the application folder.")?;
    if cfg!(target_os = "macos") {
        root = root
            .parent()
            .and_then(Path::parent)
            .ok_or("Cannot locate the application bundle.")?;
    }
    location::check(root)?;
    archive::validate_layout(root, TARGET)?;
    if root.join(executable_relative()) != executable {
        return Err("This application layout needs a manual update.");
    }
    Ok(root.to_path_buf())
}
fn command_ok(command: &mut Command, timeout: Duration, message: &'static str) -> Result<()> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| message)?;
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return if status.success() {
                    Ok(())
                } else {
                    Err(message)
                };
            }
            Ok(None) if start.elapsed() < timeout => std::thread::sleep(Duration::from_millis(25)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(message);
            }
        }
    }
}
fn probe(executable: &Path, version: &str) -> Result<()> {
    command_ok(
        Command::new(executable).args(["--update-health-check", version]),
        Duration::from_secs(15),
        "The staged update failed its version check or did not finish.",
    )
}
pub(crate) fn current_trust(root: &Path) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        signing_team(root)?;
    }
    #[cfg(not(target_os = "macos"))]
    let _ = root;
    Ok(())
}
#[cfg(target_os = "macos")]
fn signing_team(root: &Path) -> Result<String> {
    let output = Command::new("/usr/bin/codesign")
        .args(["-dv", "--verbose=4"])
        .arg(root)
        .stdin(Stdio::null())
        .output()
        .map_err(|_| "Cannot inspect the macOS signature.")?;
    if !output.status.success() {
        return Err("Cannot verify the installed macOS identity.");
    }
    String::from_utf8_lossy(&output.stderr).lines().find_map(|l| l.strip_prefix("TeamIdentifier=")).filter(|s| s.len() == 10 && s.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())).map(str::to_owned).ok_or("In-app installation requires a Developer ID signed app. Use a manual update for this development build.")
}
pub(crate) fn validate_stage(staged: &Path, current: &Path, manifest: &Manifest) -> Result<()> {
    archive::validate_layout(staged, TARGET)?;
    #[cfg(target_os = "macos")]
    {
        let team = signing_team(current)?;
        if signing_team(staged)? != team {
            return Err("The update's Apple signing team differs from the installed app.");
        }
        let developer_id = format!(
            "anchor apple generic and certificate leaf[subject.OU] = \"{team}\" and certificate leaf[field.1.2.840.113635.100.6.1.13] exists"
        );
        for path in [staged.to_path_buf(), staged.join(companion_relative())] {
            let required = if path == staged {
                format!("{developer_id} and identifier \"com.openradview.app\"")
            } else {
                developer_id.clone()
            };
            command_ok(
                Command::new("/usr/bin/codesign")
                    .args([
                        "--verify",
                        "--strict",
                        "--all-architectures",
                        "-R",
                        &required,
                    ])
                    .arg(path),
                Duration::from_secs(30),
                "The staged macOS signature failed verification.",
            )?;
        }
        command_ok(
            Command::new("/usr/sbin/spctl")
                .args(["--assess", "--type", "execute"])
                .arg(staged),
            Duration::from_secs(45),
            "macOS did not accept the update. Check its notarization and install it manually if needed.",
        )?;
    }
    #[cfg(not(target_os = "macos"))]
    let _ = current;
    probe(&staged.join(executable_relative()), &manifest.version)?;
    probe(&staged.join(companion_relative()), &manifest.version)
}
/// Directory swap on one filesystem. Only Windows moves the running image
/// separately; Unix keeps the complete old tree sealed throughout the swap.
/// Keep the old tree until the new version has actually launched successfully.
pub(crate) fn swap(staged: &Path, destination: &Path, executable: &Path) -> Result<PathBuf> {
    swap_impl(staged, destination, executable, cfg!(target_os = "windows"))
}
pub(super) fn swap_impl(
    staged: &Path,
    destination: &Path,
    executable: &Path,
    move_image: bool,
) -> Result<PathBuf> {
    let parent = destination
        .parent()
        .ok_or("Cannot locate the installation directory.")?;
    let relative = executable
        .strip_prefix(destination)
        .map_err(|_| "The running app is outside its installation.")?;
    let backup = tempfile::Builder::new()
        .prefix(".openradview-update-backup-")
        .tempdir_in(parent)
        .map_err(|_| "Cannot create an update backup beside the app.")?
        .keep();
    let old = backup.join("previous");
    let aside = backup.join("running");
    if move_image && fs::rename(executable, &aside).is_err() {
        let _ = fs::remove_dir(&backup);
        return Err(
            "The running app could not be moved aside. Close other app instances and try again.",
        );
    }
    if fs::rename(destination, &old).is_err() {
        if move_image && fs::rename(&aside, executable).is_err() {
            return Err(
                "The update could not restore the executable. The original is preserved in the backup folder beside the app.",
            );
        }
        let _ = fs::remove_dir(&backup);
        return Err(
            "The app folder is in use or cannot be changed. Close other instances and connected MCP clients, then try again.",
        );
    }
    if fs::rename(staged, destination).is_err() {
        if fs::rename(&old, destination).is_err()
            || (move_image && fs::rename(&aside, executable).is_err())
        {
            return Err(
                "The update could not restore the app. The original is preserved in the backup folder beside the app.",
            );
        }
        let _ = fs::remove_dir(&backup);
        return Err("Installation failed; the previous app was restored.");
    }
    // This move can fail if an OS still holds the image. Keeping it separately
    // is safe; cleanup refuses an incomplete layout and preserves the backup.
    if move_image {
        let _ = fs::rename(&aside, old.join(relative));
    }
    Ok(backup)
}
pub(crate) fn cleanup(backup: &Path, current: &Path) -> Result<()> {
    if backup.parent() != current.parent()
        || !backup.file_name().is_some_and(|n| {
            n.to_string_lossy()
                .starts_with(".openradview-update-backup-")
        })
        || fs::symlink_metadata(backup)
            .map_err(|_| "Cannot inspect the old update backup.")?
            .file_type()
            .is_symlink()
    {
        return Err("The update backup location is invalid.");
    }
    let entries: Vec<_> = fs::read_dir(backup)
        .map_err(|_| "Cannot inspect the old update backup.")?
        .collect();
    if entries.len() != 1
        || entries[0]
            .as_ref()
            .map_err(|_| "Cannot inspect the old update backup.")?
            .file_name()
            != "previous"
    {
        return Err("The old backup contains unexpected files and was preserved.");
    }
    archive::validate_layout(&backup.join("previous"), TARGET)?;
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::fs::OpenOptionsExt;
        for file in [executable_relative(), companion_relative()] {
            let path = backup.join("previous").join(file);
            if path.exists() {
                fs::OpenOptions::new().read(true).share_mode(0).open(path).map_err(|_| "The previous app or companion is still running; its backup was preserved.")?;
            }
        }
    }
    fs::remove_dir_all(backup).map_err(|_| "The old app backup is still in use and was preserved.")
}
pub(crate) fn relaunch(executable: &Path) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        let bundle = executable
            .parent()
            .and_then(Path::parent)
            .and_then(Path::parent)
            .ok_or("Cannot locate the updated bundle.")?;
        if !Command::new("/usr/bin/open")
            .arg("-n")
            .arg(bundle)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(|_| "Cannot reopen the updated app.")?
            .success()
        {
            return Err("Cannot reopen the updated app.");
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        Command::new(executable)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| "Cannot reopen the updated app.")?;
    }
    Ok(())
}
