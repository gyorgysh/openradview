// SPDX-License-Identifier: GPL-3.0-or-later
//! Public-key-only release verification; never signing or credential access.
use std::{fs::File, io::Read};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 2 {
        return Err("Pass the envelope path and target triple".into());
    }
    let mut bytes = Vec::new();
    File::open(&args[0])?
        .take(64 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    let target = args[1].to_str().ok_or("Invalid target")?;
    let update = orv_updates::verify_release_envelope(&bytes, target)?;
    println!("Verified OpenRadView {} for {target}", update.version);
    Ok(())
}
