// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use std::fs;

#[test]
fn recognized_container_and_manager_locations_refuse_installation() {
    for path in [
        "/nix/store/hash-openradview",
        "/snap/openradview/12",
        "/var/lib/snapd/snap/openradview/12",
        "/var/lib/flatpak/app/com.openradview.app",
        "/opt/homebrew/Cellar/openradview/1.0/OpenRadView.app",
        "/custom/brew/Caskroom/openradview/1.0/OpenRadView.app",
    ] {
        assert!(policy(Path::new(path), |_| None).is_err(), "{path}");
    }
    let standalone = tempfile::tempdir().unwrap();
    for flag in ["FLATPAK_ID", "SNAP", "APPIMAGE"] {
        assert!(
            policy(standalone.path(), |k| (k == flag)
                .then(|| "container".into()))
            .is_err()
        );
    }
}

#[test]
fn exported_manager_variables_do_not_disable_unrelated_standalone_copies() {
    let folder = tempfile::tempdir().unwrap();
    let root = folder.path().join("OpenRadView.app");
    fs::create_dir(&root).unwrap();
    assert!(
        policy(&root, |key| match key {
            "SCOOP" | "SCOOP_GLOBAL" | "ChocolateyInstall" | "NIX_STORE_DIR" =>
                Some(folder.path().join("manager").into()),
            "HOMEBREW_PREFIX" => Some(folder.path().into()),
            _ => None,
        })
        .is_ok()
    );
    for key in [
        "SCOOP",
        "SCOOP_GLOBAL",
        "ChocolateyInstall",
        "NIX_STORE_DIR",
    ] {
        assert!(policy(&root, |k| (k == key).then(|| folder.path().into())).is_err());
    }
    assert!(policy(&folder.path().join("MyCellarPictures"), |_| None).is_ok());
    assert!(
        policy(
            &folder.path().join(".local/share/flatpak/app/org.example"),
            |key| { (key == "HOME").then(|| folder.path().into()) }
        )
        .is_err()
    );
}

#[test]
fn receipt_markers_recognize_custom_manager_paths_without_reading_their_contents() {
    for marker in [
        "scoop-install.json",
        "scoop-manifest.json",
        "INSTALL_RECEIPT.json",
        ".openradview-no-self-update",
    ] {
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path().join("OpenRadView");
        fs::create_dir(&root).unwrap();
        assert!(policy(&root, |_| None).is_ok());
        fs::write(root.join(marker), b"contents are not read").unwrap();
        assert!(policy(&root, |_| None).is_err(), "{marker}");
    }
    let folder = tempfile::tempdir().unwrap();
    let root = folder.path().join("apps/openradview/1.2.3/portable");
    fs::create_dir_all(&root).unwrap();
    let version = root.parent().unwrap();
    for receipt in ["install.json", "manifest.json"] {
        fs::write(version.join(receipt), b"legacy receipt").unwrap();
    }
    assert!(policy(&root, |_| None).is_err());
    let ordinary = folder.path().join("independent");
    fs::create_dir(&ordinary).unwrap();
    fs::write(ordinary.join("manifest.json"), b"ordinary app file").unwrap();
    assert!(policy(&ordinary, |_| None).is_ok());
}
