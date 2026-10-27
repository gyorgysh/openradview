// SPDX-License-Identifier: GPL-3.0-or-later
//! Native filesystem behavior with an executable that is actually running.
use super::*;
use std::{
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

struct Running(Child);
impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn updater_child_fixture() {
    let Some(folder) = std::env::var_os("ORV_TEST_UPDATE_CHILD") else {
        return;
    };
    let folder = std::path::PathBuf::from(folder);
    fs::write(folder.join("ready"), b"ready").unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    while !folder.join("stop").exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn running_executable_survives_failed_then_successful_swap_and_backup_cleanup() {
    let folder = tempfile::tempdir().unwrap();
    let current = folder.path().join("current");
    let staged = folder.path().join("incoming");
    managed(&current, b"old");
    managed(&staged, b"new payload");
    let executable = current.join(install::executable_relative());
    let original = std::env::current_exe().unwrap();
    fs::copy(&original, &executable).unwrap();
    #[cfg(not(target_os = "macos"))]
    add_inventory(&current, "1.0.0");
    let mut child = Running(
        Command::new(&executable)
            .args(["--exact", "tests::native_process::updater_child_fixture"])
            .env("ORV_TEST_UPDATE_CHILD", folder.path())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    while !folder.path().join("ready").exists() && Instant::now() < deadline {
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "fixture exited early"
        );
        thread::sleep(Duration::from_millis(10));
    }
    assert!(folder.path().join("ready").exists());
    #[cfg(not(target_os = "macos"))]
    archive::validate_installed_content(&current, TARGET, Some("1.0.0"), || false).unwrap();
    assert!(install::swap(&folder.path().join("missing"), &current, &executable).is_err());
    archive::validate_layout(&current, TARGET).unwrap();
    assert!(child.0.try_wait().unwrap().is_none());

    let backup = install::swap(&staged, &current, &executable).unwrap();
    assert_eq!(fs::read(&executable).unwrap(), b"new payload");
    let old_executable = backup.join("previous").join(install::executable_relative());
    assert_eq!(
        fs::metadata(&old_executable).unwrap().len(),
        fs::metadata(original).unwrap().len()
    );
    archive::validate_layout(&current, TARGET).unwrap();
    archive::validate_layout(&backup.join("previous"), TARGET).unwrap();
    assert!(child.0.try_wait().unwrap().is_none());
    #[cfg(windows)]
    {
        assert!(install::cleanup_impl(&backup, &current, |_| Ok(())).is_err());
        assert!(old_executable.exists());
        archive::validate_layout(&backup.join("previous"), TARGET).unwrap();
        archive::validate_installed_content(
            &backup.join("previous"),
            TARGET,
            Some("1.0.0"),
            || false,
        )
        .unwrap();
    }

    fs::write(folder.path().join("stop"), b"stop").unwrap();
    assert!(child.0.wait().unwrap().success());
    install::cleanup_impl(&backup, &current, |_| Ok(())).unwrap();
    assert!(!backup.exists());
    assert_eq!(fs::read(&executable).unwrap(), b"new payload");
}
