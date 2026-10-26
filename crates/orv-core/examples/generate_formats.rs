// SPDX-License-Identifier: GPL-3.0-or-later
//! Local synthetic UI fixtures; never reads patient files or uses the network.
#[path = "generate_formats/color.rs"]
mod color;
#[path = "../tests/support/mod.rs"]
mod support;
use dicom_core::{DataElement, PrimitiveValue, Tag, VR};
use std::{fs, io::Write, path::PathBuf};
fn write_new(path: PathBuf, bytes: &[u8]) -> std::io::Result<()> {
    fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)?
        .write_all(bytes)
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let folder = PathBuf::from(
        std::env::args_os()
            .nth(1)
            .ok_or("Pass a fresh synthetic output directory")?,
    );
    fs::create_dir_all(&folder)?;
    let mut rgb =
        dicom_object::from_reader(std::io::Cursor::new(support::fixture(0., 1, false, false)))?;
    rgb.put_str(
        Tag(0x0020, 0x000e),
        VR::UI,
        "1.2.826.0.1.3680043.10.543.901",
    );
    rgb.put_str(Tag(0x0008, 0x0060), VR::CS, "OT");
    rgb.put_str(Tag(0x0028, 0x0004), VR::CS, "RGB");
    rgb.put_str(Tag(0x0028, 0x0030), VR::DS, "1\\1");
    for (tag, n) in [
        (0x0002, 3u16),
        (0x0006, 0),
        (0x0010, 64),
        (0x0011, 64),
        (0x0100, 8),
        (0x0101, 8),
        (0x0102, 7),
        (0x0103, 0),
    ] {
        rgb.put(DataElement::new(
            Tag(0x0028, tag),
            VR::US,
            PrimitiveValue::from(n),
        ));
    }
    let pixels: Vec<u8> = (0..64 * 64)
        .flat_map(|i| {
            let x = i % 64;
            let y = i / 64;
            [x as u8 * 4, y as u8 * 4, if x < 32 { 255 } else { 32 }]
        })
        .collect();
    rgb.put(DataElement::new(
        Tag(0x7fe0, 0x0010),
        VR::OB,
        PrimitiveValue::U8(pixels.into()),
    ));
    let mut bytes = Vec::new();
    rgb.write_all(&mut bytes)?;
    write_new(folder.join("synthetic-rgb.dcm"), &bytes)?;
    let mut scalar =
        dicom_object::from_reader(std::io::Cursor::new(support::fixture(0., 1, false, false)))?;
    scalar.meta_mut().transfer_syntax = "1.2.840.10008.1.2.1.99".into();
    scalar.meta_mut().update_information_group_length();
    bytes.clear();
    scalar.write_all(&mut bytes)?;
    write_new(folder.join("synthetic-deflated.dcm"), &bytes)?;
    let mut nifti = vec![0u8; 352];
    nifti[..4].copy_from_slice(&348u32.to_le_bytes());
    for (at, n) in [
        (40, 3u16),
        (42, 32),
        (44, 32),
        (46, 16),
        (70, 16),
        (72, 32),
        (254, 1),
    ] {
        nifti[at..at + 2].copy_from_slice(&n.to_le_bytes());
    }
    for (at, n) in [
        (108, 352f32),
        (112, 1.),
        (124, 255.),
        (280, 2.),
        (300, 2.),
        (320, 3.),
    ] {
        nifti[at..at + 4].copy_from_slice(&n.to_le_bytes());
    }
    nifti[123] = 2;
    nifti[344..348].copy_from_slice(b"n+1\0");
    for z in 0..16 {
        for y in 0..32 {
            for x in 0..32 {
                let r = ((x as f32 - 15.5).powi(2)
                    + (y as f32 - 15.5).powi(2)
                    + ((z as f32 - 7.5) * 2.).powi(2))
                .sqrt();
                nifti
                    .extend_from_slice(&(if r < 12. { 200. - r * 8. } else { 0f32 }).to_le_bytes());
            }
        }
    }
    write_new(folder.join("synthetic-volume.nii"), &nifti)?;
    color::generate(&folder, &rgb)?;
    color::volume(&folder, &rgb)?;
    println!(
        "Wrote original synthetic image fixtures and a color volume; no patient data or network requests."
    );
    Ok(())
}
