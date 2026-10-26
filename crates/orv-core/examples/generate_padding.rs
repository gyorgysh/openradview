// SPDX-License-Identifier: GPL-3.0-or-later
//! Original numeric cylinder with a declared padding border; no patient input.
//! cargo run -p orv-core --example generate_padding -- test-data/padding
#[path = "../tests/support/mod.rs"]
mod support;
use dicom_core::{DataElement, PrimitiveValue, Tag, VR};
use std::io::Cursor;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os()
        .nth(1)
        .ok_or("Pass a fresh output directory")?;
    let path = std::path::PathBuf::from(path);
    std::fs::create_dir(&path)?;
    for z in 0..16 {
        let mut object = dicom_object::from_reader(Cursor::new(support::fixture(
            z as f64 * 2.,
            z + 1,
            false,
            false,
        )))?;
        for tag in [0x0010, 0x0011] {
            object.put(DataElement::new(
                Tag(0x0028, tag),
                VR::US,
                PrimitiveValue::from(64u16),
            ));
        }
        object.put(DataElement::new(
            Tag(0x0028, 0x0120),
            VR::SS,
            PrimitiveValue::from(i16::MIN),
        ));
        object.put_str(Tag(0x0028, 0x1052), VR::DS, "0");
        object.put_str(Tag(0x0028, 0x1053), VR::DS, "1");
        for tag in [0x1050, 0x1051] {
            object.remove_element(Tag(0x0028, tag));
        }
        let values: Vec<u16> = (0..64)
            .flat_map(|y: i32| {
                (0..64).map(move |x: i32| {
                    if (x - 32).pow(2) + (y - 32).pow(2) < 22i32.pow(2) {
                        (x * 8 + y * 3 + z * 10) as u16
                    } else {
                        i16::MIN as u16
                    }
                })
            })
            .collect();
        object.put(DataElement::new(
            Tag(0x7fe0, 0x0010),
            VR::OW,
            PrimitiveValue::U16(values.into()),
        ));
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(path.join(format!("synthetic-{z:02}.dcm")))?;
        object.write_all(&mut file)?;
    }
    println!("Wrote 16 original synthetic images with declared padding.");
    Ok(())
}
