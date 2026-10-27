// SPDX-License-Identifier: GPL-3.0-or-later
//! Run only against signed release archives; no installed app or study is read.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::{fs::File, io::Read, path::Path};
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 4 && !(args.len() == 5 && args[4] == "--live") {
        return Err(
            "Pass installed archive/envelope and update archive/envelope, optionally --live".into(),
        );
    }
    let envelope = |path: &std::ffi::OsStr| -> Result<Vec<u8>, std::io::Error> {
        let mut bytes = Vec::new();
        File::open(path)?
            .take(64 * 1024 + 1)
            .read_to_end(&mut bytes)?;
        Ok(bytes)
    };
    let old_envelope = envelope(&args[1])?;
    let new_envelope = envelope(&args[3])?;
    orv_updates::release_validation::verify_roundtrip(
        Path::new(&args[0]),
        &old_envelope,
        Path::new(&args[2]),
        &new_envelope,
    )?;
    if args.len() == 5 {
        orv_updates::release_validation::verify_live_download(&old_envelope, &new_envelope)?;
    }
    println!("Verified signed updater staging, integrity, swap and cleanup");
    Ok(())
}
