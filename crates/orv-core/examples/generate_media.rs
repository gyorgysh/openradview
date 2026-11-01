// SPDX-License-Identifier: GPL-3.0-or-later
//! Usage: cargo run -p orv-core --example generate_media -- test-data/generated-media
#[path = "../tests/support/media.rs"]
mod media;
#[path = "../tests/support/mod.rs"]
mod support;
use dicom_core::{Tag, VR};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let output = std::path::PathBuf::from(
        std::env::args_os()
            .nth(1)
            .ok_or("Pass an output directory for generated media")?,
    );
    std::fs::create_dir_all(output.join("Images"))?;
    let (directory, _) = media::directory(&media::records(), &[4, 1, 3, 0, 2], true);
    std::fs::write(output.join("DICOMDIR"), directory)?;
    for instance in 1..=2 {
        let mut image = dicom_object::from_reader(std::io::Cursor::new(support::fixture(
            f64::from(instance) * 2.,
            instance,
            false,
            false,
        )))?;
        image.put_str(Tag(0x20, 0xd), VR::UI, media::STUDY);
        image.write_to_file(output.join(format!("Images/Img{instance}")))?;
    }
    println!("Wrote a generated DICOMDIR and two extensionless synthetic images.");
    Ok(())
}
