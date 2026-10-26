// SPDX-License-Identifier: GPL-3.0-or-later
//! Original sparse test data, not a patient or representative I/O benchmark.
//! Usage: generate_large_fixture OUTPUT.dcm [2..8191 frames, default8191].
#[path = "../tests/support/mod.rs"]
mod support;
use dicom_core::{DataElement, PrimitiveValue, Tag, VR};
use std::{
    fs::File,
    io::{Seek, SeekFrom, Write},
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let path =
        std::path::PathBuf::from(args.next().ok_or("Pass a new synthetic output .dcm path")?);
    let frames = args
        .next()
        .map(|s| {
            s.to_str()
                .ok_or("Invalid frame count")
                .and_then(|s| s.parse::<u32>().map_err(|_| "Invalid frame count"))
        })
        .transpose()?
        .unwrap_or(8191);
    if args.next().is_some() || !(2..=8191).contains(&frames) {
        return Err("Use 2..8191 frames".into());
    }
    let mut object =
        dicom_object::from_reader(std::io::Cursor::new(support::fixture(0., 1, false, false)))?;
    object.remove_element(Tag(0x7fe0, 0x0010));
    for tag in [Tag(0x0028, 0x0010), Tag(0x0028, 0x0011)] {
        object.put(DataElement::new(tag, VR::US, PrimitiveValue::from(512u16)));
    }
    object.put_str(Tag(0x0028, 0x0008), VR::IS, frames.to_string());
    object.put_str(Tag(0x0028, 0x1051), VR::DS, "400");
    object.put_str(Tag(0x0028, 0x1050), VR::DS, "40");
    let mut header = Vec::new();
    object.write_all(&mut header)?;
    let payload = 512u64 * 512 * 2 * u64::from(frames);
    header.extend_from_slice(&[0xe0, 0x7f, 0x10, 0, b'O', b'W', 0, 0]);
    header.extend_from_slice(&(payload as u32).to_le_bytes());
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = File::options().create_new(true).write(true).open(path)?;
    file.write_all(&header)?;
    file.set_len(header.len() as u64 + payload)?;
    let mut image = Vec::with_capacity(512 * 512 * 2);
    for y in 0..512 {
        for x in 0..512 {
            let dx = (x as f32 - 255.5) / 190.;
            let dy = (y as f32 - 255.5) / 170.;
            let r = dx * dx + dy * dy;
            let value: u16 = if r > 1. {
                0
            } else if r > 0.85 {
                400
            } else if ((dx - 0.3) * 3.).powi(2) + (dy * 4.).powi(2) < 1.
                || ((dx + 0.3) * 3.).powi(2) + (dy * 4.).powi(2) < 1.
            {
                10
            } else {
                70
            };
            image.extend_from_slice(&value.to_le_bytes());
        }
    }
    for index in [0, frames / 2, frames - 1] {
        file.seek(SeekFrom::Start(
            header.len() as u64 + u64::from(index) * 512 * 512 * 2,
        ))?;
        file.write_all(&image)?;
    }
    println!(
        "Generated {frames} synthetic frames, {:.2} logical GiB. Only first/middle/last contain a generated pattern; other frames are zero. No enhanced spatial geometry is supplied.",
        payload as f64 / (1024. * 1024. * 1024.)
    );
    Ok(())
}
