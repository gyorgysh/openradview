// SPDX-License-Identifier: GPL-3.0-or-later
//! Explicit release checks on signed application archives in a disposable folder.
use crate::{archive, install, protocol, unpack_bundle};
use std::{
    fs,
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
};

/// Exercise the updater's real unpack, signature, health, swap and cleanup paths.
/// Both inputs must have valid production-signed manifests. No installed app is touched.
pub fn verify_roundtrip(
    installed_archive: &Path,
    installed_envelope: &[u8],
    update_archive: &Path,
    update_envelope: &[u8],
) -> protocol::Result<()> {
    let key = protocol::public_key()?;
    let old = protocol::verify_envelope(installed_envelope, &key, protocol::TARGET)?;
    let new = protocol::verify_envelope(update_envelope, &key, protocol::TARGET)?;
    if protocol::stable_version(&old.version)? > protocol::stable_version(&new.version)? {
        return Err("The release check cannot install an older version.");
    }
    old.verify_file(installed_archive)?;
    new.verify_file(update_archive)?;

    let scratch = tempfile::tempdir().map_err(|_| "Cannot create release check folder.")?;
    let corrupted = scratch.path().join("corrupted-update");
    fs::copy(update_archive, &corrupted).map_err(|_| "Cannot copy release check archive.")?;
    let mut file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&corrupted)
        .map_err(|_| "Cannot modify release check archive.")?;
    let offset = new.bytes / 2;
    file.seek(SeekFrom::Start(offset))
        .map_err(|_| "Cannot inspect release check archive.")?;
    let mut byte = [0u8; 1];
    file.read_exact(&mut byte)
        .map_err(|_| "Cannot inspect release check archive.")?;
    byte[0] ^= 1;
    file.seek(SeekFrom::Start(offset))
        .map_err(|_| "Cannot inspect release check archive.")?;
    file.write_all(&byte)
        .map_err(|_| "Cannot modify release check archive.")?;
    drop(file);
    if new.verify_file(&corrupted).is_ok() {
        return Err("The release check accepted a modified archive.");
    }
    fs::remove_file(corrupted).map_err(|_| "Cannot remove release check archive.")?;
    let current = unpack_bundle(installed_archive, &scratch.path().join("installed"), &old)?;
    let executable = current.join(install::executable_relative());
    if install::root(&executable)? != current {
        return Err("The release check installation is not managed.");
    }
    install::current_trust(&current, Some(&old.version), || false)?;
    let extra = current.join("synthetic-user-file");
    fs::write(&extra, b"preserve").map_err(|_| "Cannot create release check file.")?;
    if install::current_trust(&current, Some(&old.version), || false).is_ok() {
        return Err("The release check accepted an extra installed file.");
    }
    fs::remove_file(extra).map_err(|_| "Cannot remove release check file.")?;
    #[cfg(target_os = "macos")]
    let current_file = current.join("Contents/Info.plist");
    #[cfg(not(target_os = "macos"))]
    let current_file = current.join("PRIVACY.md");
    let current_bytes =
        fs::read(&current_file).map_err(|_| "Cannot inspect release check file.")?;
    let mut changed = current_bytes.clone();
    changed.push(b'\n');
    fs::write(&current_file, changed).map_err(|_| "Cannot modify release check file.")?;
    if install::current_trust(&current, Some(&old.version), || false).is_ok() {
        return Err("The release check accepted a modified installed file.");
    }
    fs::write(&current_file, current_bytes).map_err(|_| "Cannot restore release check file.")?;
    install::current_trust(&current, Some(&old.version), || false)?;
    install::probe(&executable, &old.version)?;
    install::probe(&current.join(install::companion_relative()), &old.version)?;

    let parent = current.parent().ok_or("Invalid release check folder.")?;
    let staging = tempfile::Builder::new()
        .prefix(".openradview-update-stage-")
        .tempdir_in(parent)
        .map_err(|_| "Cannot create release check staging folder.")?;
    let staged = unpack_bundle(update_archive, &staging.path().join("unpacked"), &new)?;

    // Check the actual signed binaries reject wrong identities as well as
    // accepting their real team; this must not become an integrity-only check.
    #[cfg(target_os = "macos")]
    {
        let team = install::signing_team(&current)?;
        let other_team = if team == "AAAAAAAAAA" {
            "BBBBBBBBBB"
        } else {
            "AAAAAAAAAA"
        };
        if install::verify_signature(&staged, other_team, Some("com.openradview.app")).is_ok()
            || install::verify_signature(
                &staged.join(install::companion_relative()),
                other_team,
                None,
            )
            .is_ok()
            || install::verify_signature(&staged, &team, Some("invalid.example")).is_ok()
        {
            return Err("The release check accepted an incorrect signing identity.");
        }

        let info = staged.join("Contents/Info.plist");
        let original = fs::read(&info).map_err(|_| "Cannot inspect release check metadata.")?;
        let mut changed = original.clone();
        changed.push(b'\n');
        fs::write(&info, changed).map_err(|_| "Cannot modify release check metadata.")?;
        if install::verify_signature(&staged, &team, Some("com.openradview.app")).is_ok() {
            return Err("The release check accepted modified application metadata.");
        }
        fs::write(&info, original).map_err(|_| "Cannot restore release check metadata.")?;
    }
    install::validate_stage(&staged, &current, &new)?;

    install::current_trust(&current, Some(&old.version), || false)?;
    let backup = install::swap(&staged, &current, &executable)?;
    install::probe(&executable, &new.version)?;
    install::probe(&current.join(install::companion_relative()), &new.version)?;
    let previous = backup.join("previous");
    #[cfg(not(target_os = "macos"))]
    {
        let current_file = current.join("PRIVACY.md");
        let current_bytes =
            fs::read(&current_file).map_err(|_| "Cannot inspect release check installation.")?;
        let mut changed = current_bytes.clone();
        changed.push(b'\n');
        fs::write(&current_file, changed)
            .map_err(|_| "Cannot modify release check installation.")?;
        if install::cleanup(&backup, &current, || false).is_ok() || !backup.is_dir() {
            return Err("The release check removed a backup with a modified installation.");
        }
        fs::write(&current_file, current_bytes)
            .map_err(|_| "Cannot restore release check installation.")?;
        let file = previous.join("PRIVACY.md");
        let original = fs::read(&file).map_err(|_| "Cannot inspect release check backup.")?;
        let mut changed = original.clone();
        changed.push(b'\n');
        fs::write(&file, changed).map_err(|_| "Cannot modify release check backup.")?;
        if install::current_trust(&previous, None, || false).is_ok()
            || install::cleanup(&backup, &current, || false).is_ok()
            || !backup.is_dir()
        {
            return Err("The release check removed a modified backup.");
        }
        fs::write(&file, original).map_err(|_| "Cannot restore release check backup.")?;
    }
    install::probe(&previous.join(install::executable_relative()), &old.version)?;
    install::probe(&previous.join(install::companion_relative()), &old.version)?;
    install::cleanup(&backup, &current, || false)?;
    archive::validate_layout(&current, protocol::TARGET)
}

/// Exercise the real GitHub feed, redirect policy and signed download without
/// opening a study or changing any installed app.
pub fn verify_live_download(
    installed_envelope: &[u8],
    update_envelope: &[u8],
) -> protocol::Result<()> {
    let key = protocol::public_key()?;
    let old = protocol::verify_envelope(installed_envelope, &key, protocol::TARGET)?;
    let update = protocol::verify_envelope(update_envelope, &key, protocol::TARGET)?;
    let latest = crate::check(&old.version)?
        .ok_or("The live feed did not offer an update for the checked version.")?;
    if protocol::stable_version(&latest.version)? < protocol::stable_version(&update.version)? {
        return Err("The live feed is older than the checked update.");
    }
    let scratch = tempfile::tempdir().map_err(|_| "Cannot create release check folder.")?;
    crate::network::download(
        &old.version,
        &update,
        &scratch.path().join(&update.archive),
        |_| {},
    )
}
