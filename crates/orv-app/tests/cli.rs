// SPDX-License-Identifier: GPL-3.0-or-later
use std::{
    process::{Command, Stdio},
    time::{Duration, Instant},
};

fn invoke(args: &[&str]) -> std::process::Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_openradview"))
        .args(args)
        .env("WGPU_BACKEND", "invalid-backend-must-not-be-read")
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if child.try_wait().unwrap().is_some() {
            return child.wait_with_output().unwrap();
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("CLI action tried to start the GUI or failed to exit");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn help_version_and_health_probe_exit_before_graphics_initialization() {
    for flag in ["--help", "-h"] {
        let output = invoke(&[flag]);
        assert!(output.status.success(), "{output:?}");
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(text.contains("Usage: openradview"));
        assert!(text.contains("WGPU_BACKEND"));
        assert!(output.stderr.is_empty());
    }
    let output = invoke(&["--version"]);
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim(),
        format!("OpenRadView {}", env!("CARGO_PKG_VERSION"))
    );
    assert!(
        invoke(&["--update-health-check", env!("CARGO_PKG_VERSION")])
            .status
            .success()
    );
    assert!(!invoke(&["--update-health-check", "0.0.0"]).status.success());
    assert_eq!(invoke(&["--unknown"]).status.code(), Some(2));
}
