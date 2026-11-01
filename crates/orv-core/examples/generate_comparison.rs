// SPDX-License-Identifier: GPL-3.0-or-later
//! Four generated five-image series: parallel grids, a crossing plane and an unrelated reference.
#[path = "../tests/support/mod.rs"]
mod support;
use dicom_core::{DataElement, PrimitiveValue, Tag, VR};
use std::{
    fs,
    io::{Cursor, Write},
    path::PathBuf,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = std::env::args_os().skip(1);
    let folder = PathBuf::from(
        arguments
            .next()
            .ok_or("Pass a fresh output directory for generated comparison images")?,
    );
    if arguments.next().is_some() {
        return Err("Pass only the output directory".into());
    }
    fs::create_dir_all(&folder)?;
    for series in 1u16..=4 {
        let (width, height) = if series == 2 {
            (20u16, 10u16)
        } else {
            (16, 12)
        };
        for frame in 1u16..=5 {
            let mut image = dicom_object::from_reader(Cursor::new(support::fixture(
                f64::from(frame) * 2.,
                i32::from(series * 100 + frame),
                false,
                false,
            )))?;
            image.put_str(Tag(0x0020, 0x000d), VR::UI, "2.25.2718281828459045230");
            image.put_str(
                Tag(0x0020, 0x000e),
                VR::UI,
                format!("2.25.271828182845904523{series}"),
            );
            image.put_str(
                Tag(0x0020, 0x0052),
                VR::UI,
                if series == 4 {
                    "2.25.2718281828459045236"
                } else {
                    "2.25.2718281828459045235"
                },
            );
            image.put_str(Tag(0x0020, 0x0011), VR::IS, series.to_string());
            image.put_str(Tag(0x0020, 0x0013), VR::IS, frame.to_string());
            let position = match series {
                2 => format!("-0.25\\-0.5\\{}", frame * 2),
                3 => format!("0\\{}\\0", frame * 2),
                _ => format!("0\\0\\{}", frame * 2),
            };
            image.put_str(Tag(0x0020, 0x0032), VR::DS, position);
            image.put_str(
                Tag(0x0020, 0x0037),
                VR::DS,
                if series == 3 {
                    "1\\0\\0\\0\\0\\1"
                } else {
                    "1\\0\\0\\0\\1\\0"
                },
            );
            image.put_str(
                Tag(0x0028, 0x0030),
                VR::DS,
                if series == 2 { "1\\0.5" } else { "2\\1" },
            );
            image.put_str(Tag(0x0028, 0x1050), VR::DS, "2500");
            image.put_str(Tag(0x0028, 0x1051), VR::DS, "5000");
            image.put_str(Tag(0x0028, 0x1052), VR::DS, "0");
            image.put_str(Tag(0x0028, 0x1053), VR::DS, "1");
            image.put_str(Tag(0x0028, 0x1054), VR::LO, "HU");
            for (tag, value) in [(Tag(0x0028, 0x0010), height), (Tag(0x0028, 0x0011), width)] {
                image.put(DataElement::new(tag, VR::US, PrimitiveValue::from(value)));
            }
            let pixels: Vec<u16> = (0..height)
                .flat_map(|row| {
                    (0..width).map(move |column| series * 1000 + frame * 100 + row * width + column)
                })
                .collect();
            image.put(DataElement::new(
                Tag(0x7fe0, 0x0010),
                VR::OW,
                PrimitiveValue::U16(pixels.into()),
            ));
            let mut bytes = Vec::new();
            image.write_all(&mut bytes)?;
            fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(folder.join(format!("series-{series}-image-{frame}.dcm")))?
                .write_all(&bytes)?;
        }
    }
    println!(
        "Wrote four generated five-image series. Series 1/2 are parallel grids, 3 crosses them, and 4 has an unrelated reference. No anatomy is represented."
    );
    Ok(())
}
