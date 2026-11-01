// SPDX-License-Identifier: GPL-3.0-or-later
//! Original numeric acquisition fixtures; no anatomy, patient data or network access.
#[path = "../tests/support/nifti_axes.rs"]
mod phantom;
use std::{fs, io::Write, path::PathBuf};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = std::env::args_os().skip(1);
    let folder = PathBuf::from(
        arguments
            .next()
            .ok_or("Pass a fresh acquisition fixture directory")?,
    );
    if arguments.next().is_some() {
        return Err("Pass only the output directory".into());
    }
    fs::create_dir_all(&folder)?;
    for (name, version, complex, units, step, offset) in [
        ("timed-complex.nii", 2, true, 18, 250., -500.),
        ("frequency.nii", 1, false, 34, 5., -10.),
        ("unknown-time.nii", 2, false, 2, 0., 0.),
        ("invalid-time.nii", 2, false, 18, -1., 0.),
    ] {
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(folder.join(name))?
            .write_all(&phantom::fixture(
                version, false, complex, units, step, offset,
            ))?;
    }
    println!(
        "Wrote original numeric time/component/complex/higher-axis fixtures; no anatomy or patient data."
    );
    Ok(())
}
