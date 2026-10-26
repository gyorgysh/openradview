// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use base64::{Engine, prelude::BASE64_STANDARD};
use ed25519_dalek::{Signer, SigningKey};
use std::{fs, io::Write, path::Path};

fn manifest() -> Manifest {
    let (archive, bundle) = protocol::names("1.2.3", TARGET);
    Manifest {
        schema_version: 1,
        repository: protocol::REPOSITORY.into(),
        version: "1.2.3".into(),
        target: TARGET.into(),
        archive,
        bundle,
        bytes: 3,
        sha256: "a".repeat(64),
    }
}

#[test]
fn release_target_contract_matches_client_payload_names_and_download_urls() {
    let catalog: serde_json::Value =
        serde_json::from_str(include_str!("../../../packaging/release-targets.json")).unwrap();
    for entry in catalog["targets"].as_array().unwrap() {
        let target = entry["target"].as_str().unwrap();
        assert!(protocol::supported_target(target));
        let (archive, folder) = protocol::names("1.2.3", target);
        assert_eq!(
            archive,
            entry["archive"]
                .as_str()
                .unwrap()
                .replace("{version}", "1.2.3")
        );
        assert_eq!(
            folder,
            entry["bundle"]
                .as_str()
                .unwrap()
                .replace("{version}", "1.2.3")
        );
        let mut release = manifest();
        release.target = target.into();
        release.archive = archive.clone();
        release.bundle = folder;
        assert_eq!(
            release.url(),
            format!("https://github.com/gyorgysh/openradview/releases/download/v1.2.3/{archive}")
        );
        assert_ne!(
            archive,
            entry["installer"]
                .as_str()
                .unwrap()
                .replace("{version}", "1.2.3")
        );
    }
}
fn envelope(manifest: &Manifest, key: &SigningKey) -> Vec<u8> {
    let raw = serde_json::to_vec(manifest).unwrap();
    serde_json::to_vec(&serde_json::json!({"manifest":BASE64_STANDARD.encode(&raw),"signature":BASE64_STANDARD.encode(key.sign(&raw).to_bytes())})).unwrap()
}
#[test]
fn signature_binds_all_release_fields_before_use() {
    // Deterministic test-only seed. Never used as an operational signing key.
    let key = SigningKey::from_bytes(&[7; 32]);
    let good = manifest();
    let bytes = envelope(&good, &key);
    assert_eq!(
        protocol::verify_envelope(&bytes, &key.verifying_key(), TARGET)
            .unwrap()
            .version,
        "1.2.3"
    );
    assert!(
        protocol::verify_envelope(
            &bytes,
            &SigningKey::from_bytes(&[8; 32]).verifying_key(),
            TARGET
        )
        .is_err()
    );
    for mutate in 0..8 {
        let mut bad = good.clone();
        match mutate {
            0 => bad.repository = "elsewhere/app".into(),
            1 => bad.target = "other-target".into(),
            2 => bad.archive = "../escape.zip".into(),
            3 => bad.bundle = "../../app".into(),
            4 => bad.bytes = protocol::MAX_ARCHIVE + 1,
            5 => bad.schema_version = 2,
            6 => bad.sha256 = "invalid".into(),
            _ => bad.version = "1.2.3-beta".into(),
        }
        assert!(
            protocol::verify_envelope(&envelope(&bad, &key), &key.verifying_key(), TARGET).is_err()
        );
    }
    let mut corrupted: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    corrupted["manifest"] = BASE64_STANDARD.encode(b"tampered").into();
    assert!(
        protocol::verify_envelope(
            &serde_json::to_vec(&corrupted).unwrap(),
            &key.verifying_key(),
            TARGET
        )
        .is_err()
    );
    assert!(
        verify_release_envelope(&bytes, TARGET).is_err(),
        "test signature must not match the production public key"
    );
    assert!(protocol::verify_envelope(&vec![b' '; 65537], &key.verifying_key(), TARGET).is_err());
}
#[test]
fn update_checks_cannot_turn_a_partial_or_unverified_download_into_an_install() {
    for (input, size, ok) in [
        (b"abc".as_slice(), 3, true),
        (b"ab", 3, false),
        (b"abcd", 3, false),
    ] {
        let mut output = Vec::new();
        let mut received = 0;
        assert_eq!(
            network::transfer(&mut &*input, &mut output, size, &mut |n| received = n).is_ok(),
            ok
        );
        assert!(output.len() <= size as usize);
        assert!(received <= size);
    }
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("archive.zip");
    fs::write(&path, b"abc").unwrap();
    let mut m = manifest();
    m.sha256 = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".into();
    assert!(m.verify_file(&path).is_ok());
    fs::write(&path, b"abd").unwrap();
    assert!(m.verify_file(&path).is_err());
    fs::write(&path, b"abcd").unwrap();
    assert!(m.verify_file(&path).is_err());
}
#[test]
fn preferences_preserve_opt_out_and_require_a_saved_choice() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("updates.json");
    let initial = Preferences::default();
    assert!(!initial.chosen && !initial.enabled);
    let prefs = Preferences {
        chosen: true,
        enabled: false,
        last_check: 123,
        ..Default::default()
    };
    preferences::save(&path, &prefs).unwrap();
    let read = preferences::load(&path);
    assert!(read.chosen);
    assert!(!read.enabled);
    assert_eq!(read.last_check, 123);
    preferences::save(
        &path,
        &Preferences {
            enabled: true,
            ..prefs
        },
    )
    .unwrap();
    assert!(preferences::load(&path).enabled);
    fs::write(&path, b"broken").unwrap();
    assert!(!preferences::load(&path).chosen);
    assert!(
        protocol::stable_version("0.15.0").unwrap() > protocol::stable_version("0.14.0").unwrap()
    );
    for v in ["01.2.3", "1.2", "1.2.3-beta", "1.2.3+metadata", "../../etc"] {
        assert!(protocol::stable_version(v).is_err());
    }
}
fn zip(path: &Path, names: &[&str]) {
    let mut zip = zip::ZipWriter::new(fs::File::create(path).unwrap());
    for name in names {
        zip.start_file(
            *name,
            zip::write::SimpleFileOptions::default().unix_permissions(0o755),
        )
        .unwrap();
        zip.write_all(b"synthetic executable").unwrap();
    }
    zip.finish().unwrap();
}
#[test]
fn zip_unpacks_regular_files_and_refuses_traversal_case_aliases_and_directory_bombs() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("app.zip");
    let out = folder.path().join("out");
    fs::create_dir(&out).unwrap();
    zip(&path, &["App/bin"]);
    archive::unpack(&path, &out).unwrap();
    assert_eq!(
        fs::read(out.join("App/bin")).unwrap(),
        b"synthetic executable"
    );
    for (i, names) in [
        ["../outside", "other"],
        ["App/Bin", "App/bin"],
        ["/absolute", "other"],
        ["App/CON.txt", "other"],
    ]
    .iter()
    .enumerate()
    {
        zip(&path, names);
        assert!(archive::unpack(&path, &folder.path().join(format!("bad{i}"))).is_err());
    }
    let mut bytes = fs::read(&path).unwrap();
    let at = bytes.len() - 22;
    bytes[at + 8..at + 12].copy_from_slice(&[0xff; 4]);
    fs::write(&path, bytes).unwrap();
    assert!(archive::unpack(&path, &folder.path().join("bomb")).is_err());
    assert!(!folder.path().join("outside").exists());
}
#[test]
fn tar_rejects_links_and_expansion_beyond_the_bound() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("app.tar.gz");
    let gzip = flate2::write::GzEncoder::new(
        fs::File::create(&path).unwrap(),
        flate2::Compression::fast(),
    );
    let mut tar = tar::Builder::new(gzip);
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(tar::EntryType::Symlink);
    header.set_size(0);
    header.set_mode(0o777);
    header.set_link_name("/etc/hosts").unwrap();
    header.set_cksum();
    tar.append_data(&mut header, "App/link", &b""[..]).unwrap();
    tar.into_inner().unwrap().finish().unwrap();
    assert!(archive::unpack(&path, &folder.path().join("out")).is_err());
    let mut file = tar::Header::new_gnu();
    file.set_path("App/huge").unwrap();
    file.set_size(protocol::MAX_EXPANDED + 1);
    file.set_mode(0o644);
    file.set_cksum();
    let mut gzip = flate2::write::GzEncoder::new(
        fs::File::create(&path).unwrap(),
        flate2::Compression::fast(),
    );
    gzip.write_all(file.as_bytes()).unwrap();
    gzip.write_all(&[0; 1024]).unwrap();
    gzip.finish().unwrap();
    assert!(archive::unpack(&path, &folder.path().join("huge")).is_err());
}
fn managed(root: &Path, bytes: &[u8]) {
    let exe = root.join(install::executable_relative());
    fs::create_dir_all(exe.parent().unwrap()).unwrap();
    fs::write(&exe, bytes).unwrap();
    let layout = archive::layout_path(root);
    fs::create_dir_all(layout.parent().unwrap()).unwrap();
    let names = [
        install::executable_relative().to_owned(),
        layout
            .strip_prefix(root)
            .unwrap()
            .to_str()
            .unwrap()
            .replace('\\', "/"),
    ];
    fs::write(
        layout,
        serde_json::to_vec(&archive::Layout {
            schema_version: 1,
            product: "OpenRadView".into(),
            target: TARGET.into(),
            files: names.into(),
        })
        .unwrap(),
    )
    .unwrap();
}
#[test]
fn managed_install_refuses_added_files_and_retains_a_complete_previous_version() {
    let folder = tempfile::tempdir().unwrap();
    let current = folder.path().join("app");
    let staged = folder.path().join("new");
    managed(&current, b"old");
    managed(&staged, b"new");
    archive::validate_layout(&current, TARGET).unwrap();
    let extra = current.join("my-important-file.txt");
    fs::write(&extra, b"user file").unwrap();
    assert!(archive::validate_layout(&current, TARGET).is_err());
    assert_eq!(fs::read(&extra).unwrap(), b"user file");
    fs::remove_file(extra).unwrap();
    let exe = current.join(install::executable_relative());
    let backup = install::swap(&staged, &current, &exe).unwrap();
    assert_eq!(fs::read(&exe).unwrap(), b"new");
    assert_eq!(
        fs::read(backup.join("previous").join(install::executable_relative())).unwrap(),
        b"old"
    );
    archive::validate_layout(&current, TARGET).unwrap();
    archive::validate_layout(&backup.join("previous"), TARGET).unwrap();
    install::cleanup(&backup, &current).unwrap();
    assert!(!backup.exists());
    assert_eq!(fs::read(exe).unwrap(), b"new");
}
#[test]
fn failed_swap_restores_the_old_tree_and_executable() {
    let folder = tempfile::tempdir().unwrap();
    let current = folder.path().join("app");
    managed(&current, b"old");
    let exe = current.join(install::executable_relative());
    assert!(install::swap(&folder.path().join("missing-stage"), &current, &exe).is_err());
    assert_eq!(fs::read(&exe).unwrap(), b"old");
    archive::validate_layout(&current, TARGET).unwrap();
}
#[test]
fn both_swap_strategies_keep_complete_backups_and_restore_failed_stages() {
    // Exercise filesystem sequencing on this host; this does not emulate
    // Windows executable locking or replace native Windows acceptance.
    for move_image in [false, true] {
        for succeeds in [false, true] {
            let folder = tempfile::tempdir().unwrap();
            let current = folder.path().join("app");
            let staged = folder.path().join("new");
            managed(&current, b"old");
            if succeeds {
                managed(&staged, b"new");
            }
            let exe = current.join(install::executable_relative());
            let result = install::swap_impl(&staged, &current, &exe, move_image);
            if succeeds {
                let backup = result.unwrap();
                assert_eq!(fs::read(&exe).unwrap(), b"new");
                archive::validate_layout(&backup.join("previous"), TARGET).unwrap();
                assert!(!backup.join("running").exists());
                install::cleanup(&backup, &current).unwrap();
                assert!(!backup.exists());
            } else {
                assert!(result.is_err());
                assert_eq!(fs::read(&exe).unwrap(), b"old");
                archive::validate_layout(&current, TARGET).unwrap();
                assert_eq!(fs::read_dir(folder.path()).unwrap().count(), 1);
            }
        }
    }
}
#[cfg(unix)]
#[test]
fn symlinked_files_and_parent_directories_are_never_managed_update_targets() {
    use std::os::unix::fs::symlink;
    let folder = tempfile::tempdir().unwrap();
    let root = folder.path().join("app");
    managed(&root, b"old");
    let exe = root.join(install::executable_relative());
    fs::remove_file(&exe).unwrap();
    let outside = folder.path().join("outside");
    fs::write(&outside, b"private").unwrap();
    symlink(&outside, &exe).unwrap();
    assert!(archive::validate_layout(&root, TARGET).is_err());
    assert_eq!(fs::read(&outside).unwrap(), b"private");
}
#[test]
fn download_redirects_are_limited_to_public_github_hosts() {
    for url in [
        "https://github.com/a",
        "https://release-assets.githubusercontent.com/a",
    ] {
        assert!(network::test_allowed(url));
    }
    for url in [
        "http://github.com/a",
        "https://github.com.evil.test/a",
        "https://user:pass@github.com/a",
        "https://github.com:444/a",
        "https://localhost/a",
    ] {
        assert!(!network::test_allowed(url));
    }
}
#[test]
fn background_checks_require_consent_daily_deadline_and_no_pending_restart() {
    let mut prefs = Preferences {
        chosen: false,
        enabled: true,
        last_check: 1,
        ..Default::default()
    };
    assert!(!automatic_due(&prefs, Status::Idle, 90000, false));
    prefs.chosen = true;
    prefs.enabled = false;
    assert!(!automatic_due(&prefs, Status::Idle, 90000, false));
    prefs.enabled = true;
    assert!(!automatic_due(&prefs, Status::Idle, 86400, false));
    assert!(automatic_due(&prefs, Status::Idle, 86401, false));
    assert!(!automatic_due(&prefs, Status::ReadyToRestart, 90000, false));
    assert!(!automatic_due(&prefs, Status::Downloading, 90000, false));
    prefs.last_check = 100000;
    assert!(!automatic_due(&prefs, Status::Idle, 90000, false));
}
#[test]
fn concurrent_install_lock_is_nonblocking_and_released_with_its_handle() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("update.lock");
    let first = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .read(true)
        .open(&path)
        .unwrap();
    let second = fs::OpenOptions::new()
        .write(true)
        .read(true)
        .open(&path)
        .unwrap();
    first.try_lock().unwrap();
    assert!(second.try_lock().is_err());
    drop(first);
    second.try_lock().unwrap();
}
#[test]
fn unsavable_preferences_honor_opt_out_and_never_enable_background_requests() {
    let folder = tempfile::tempdir().unwrap();
    let blocked = folder.path().join("file");
    fs::write(&blocked, b"not a directory").unwrap();
    let state = State {
        status: Status::Idle,
        detail: "",
        manifest: None,
        downloaded: 0,
        installable: false,
        package_managed: false,
        prefs: Preferences {
            chosen: true,
            enabled: true,
            ..Default::default()
        },
    };
    let mut updater = Updater {
        state: Arc::new(Mutex::new(state)),
        tasks: None,
        prefs_path: Some(blocked.join("updates.json")),
        executable: None,
        restart: false,
        lifecycle: Arc::new(lifecycle::Lifecycle::default()),
    };
    assert!(updater.set_enabled(false).is_err());
    assert!(!updater.enabled());
    assert!(!updater.preference_chosen());
    assert!(updater.set_enabled(true).is_err());
    assert!(!updater.enabled());
    let s = lock(&updater.state);
    assert!(!automatic_due(&s.prefs, s.status, 100000, false));
}
#[test]
fn quitting_first_cancels_installation_before_any_files_are_moved() {
    let lifecycle = lifecycle::Lifecycle::default();
    assert!(lifecycle.request_close());
    assert!(lifecycle.begin_commit().is_err());
    assert!(lifecycle.request_close());
}
#[test]
fn quitting_during_a_swap_waits_for_the_rename_or_rollback_to_finish() {
    let lifecycle = lifecycle::Lifecycle::default();
    let commit = lifecycle.begin_commit().unwrap();
    assert!(!lifecycle.request_close());
    drop(commit);
    assert!(lifecycle.request_close());
    assert!(lifecycle.begin_commit().is_err());
}
mod native_process;

#[test]
fn package_marker_disables_background_checks_but_preserves_manual_checks_and_preferences() {
    let folder = tempfile::tempdir().unwrap();
    let executable = folder.path().join("openradview");
    assert!(!package_install(&executable));
    fs::write(
        folder.path().join(".openradview-no-self-update"),
        "package managed\n",
    )
    .unwrap();
    assert!(package_install(&executable));
    for chosen in [false, true] {
        let prefs = Preferences {
            chosen,
            enabled: true,
            last_check: 0,
            ..Default::default()
        };
        assert!(!automatic_due(&prefs, Status::Idle, 100000, true));
        assert_eq!(automatic_due(&prefs, Status::Idle, 100000, false), chosen);
        let prefs_path = folder.path().join("updates.json");
        preferences::save(&prefs_path, &prefs).unwrap();
        let before = fs::read(&prefs_path).unwrap();
        let (send, receive) = mpsc::sync_channel(1);
        let mut updater = Updater {
            state: Arc::new(Mutex::new(State {
                status: Status::Idle,
                detail: "",
                manifest: None,
                downloaded: 0,
                installable: false,
                package_managed: package_install(&executable),
                prefs,
            })),
            tasks: Some(send),
            prefs_path: Some(prefs_path.clone()),
            executable: Some(executable.clone()),
            restart: false,
            lifecycle: Arc::new(lifecycle::Lifecycle::default()),
        };
        assert!(updater.package_managed());
        assert!(!updater.enabled());
        assert!(updater.set_enabled(true).is_err());
        assert_eq!(fs::read(&prefs_path).unwrap(), before);
        updater.check().unwrap();
        assert!(matches!(receive.try_recv(), Ok(Task::Check)));
    }
}
