// SPDX-License-Identifier: GPL-3.0-or-later
//! Usage: cargo run -p orv-core --example generate_fixture -- test-data/synthetic
#[path = "../tests/support/mod.rs"]
mod support;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os()
        .nth(1)
        .ok_or("Pass an output directory for synthetic files")?;
    let path = std::path::PathBuf::from(path);
    std::fs::create_dir_all(&path)?;
    for i in 1..=3 {
        std::fs::write(
            path.join(format!("synthetic-{i}.dcm")),
            support::fixture(i as f64 * 2.0, i, false, false),
        )?;
    }
    println!("Wrote 3 generated DICOM images (synthetic labels and pixels only).");
    Ok(())
}
