// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
fn registry() -> (tempfile::TempDir, Registry) {
    let dir = tempfile::tempdir().unwrap();
    let registry = Registry {
        folder: dir.path().join("instances"),
    };
    (dir, registry)
}
#[test]
fn live_lock_stale_records_and_multiple_instances_are_distinct() {
    let (_dir, registry) = registry();
    assert!(registry.list().unwrap().is_empty());
    let one = Instance::new(14567).unwrap();
    let two = Instance::new(24567).unwrap();
    let lease = registry.publish(one.clone()).unwrap();
    let second = registry.publish(two.clone()).unwrap();
    assert_eq!(registry.list().unwrap().len(), 2);
    assert!(registry.list().unwrap().contains(&one));
    // Model an abrupt exit: record survives but the OS releases its lock.
    let stale_path = second.path.clone();
    let bytes = second.bytes.clone();
    drop(second);
    fs::write(&stale_path, bytes).unwrap();
    File::create(stale_path.with_extension("lock")).unwrap();
    assert_eq!(registry.list().unwrap(), [one]);
    drop(lease);
    assert!(registry.list().unwrap().is_empty());
}
#[test]
fn invalid_or_oversized_live_descriptors_are_ignored() {
    let (_dir, registry) = registry();
    let lease = registry.publish(Instance::new(14567).unwrap()).unwrap();
    fs::write(&lease.path, vec![b'x'; 4097]).unwrap();
    assert!(registry.list().unwrap().is_empty());
    let mut invalid: Instance = serde_json::from_slice(&lease.bytes).unwrap();
    invalid.port = 0;
    fs::write(&lease.path, serde_json::to_vec(&invalid).unwrap()).unwrap();
    assert!(registry.list().unwrap().is_empty());
}
#[cfg(unix)]
#[test]
fn discovery_rejects_public_folders_and_symlink_or_public_records() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let (_dir, registry) = registry();
    let lease = registry.publish(Instance::new(14567).unwrap()).unwrap();
    fs::set_permissions(&registry.folder, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(registry.list().is_err());
    fs::set_permissions(&registry.folder, fs::Permissions::from_mode(0o700)).unwrap();
    fs::set_permissions(&lease.path, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(registry.list().unwrap().is_empty());
    fs::remove_file(&lease.path).unwrap();
    symlink(&lease.lock_path, &lease.path).unwrap();
    assert!(registry.list().unwrap().is_empty());
}
