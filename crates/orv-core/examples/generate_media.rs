// SPDX-License-Identifier: GPL-3.0-or-later
//! Usage: cargo run -p orv-core --example generate_media -- test-data/generated-media
//! Add --label-phantom for two disconnected boxes in a 16×12×5 source grid.
//! Add --segmentation-phantom for those images and overlapping supplied-mask fixtures.
#[path = "../tests/support/generated_segments.rs"]
mod generated_segments;
#[path = "../tests/support/media.rs"]
mod media;
#[path = "../tests/support/mod.rs"]
mod support;
use dicom_core::{DataElement, PrimitiveValue, Tag, VR};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = std::env::args_os().skip(1);
    let output = std::path::PathBuf::from(
        arguments
            .next()
            .ok_or("Pass an output directory for generated media")?,
    );
    let (phantom, segments) = match arguments.next() {
        None => (false, false),
        Some(flag) if flag == "--label-phantom" => (true, false),
        Some(flag) if flag == "--segmentation-phantom" => (true, true),
        Some(_) => return Err("Unknown option".into()),
    };
    if arguments.next().is_some() {
        return Err("Too many arguments".into());
    }
    std::fs::create_dir_all(output.join("Images"))?;
    let count = if phantom { 5 } else { 2 };
    let mut records = media::records();
    for instance in 3..=count {
        let mut record = records[4].clone();
        record.file = Some(format!("IMAGES\\IMG{instance}"));
        record.instance = format!("2.25.314159{instance}");
        records.push(record);
    }
    for index in 3..records.len() {
        records[index].next = (index + 1 < records.len()).then_some(index + 1);
    }
    let order = if phantom {
        (0..records.len()).rev().collect::<Vec<_>>()
    } else {
        vec![4, 1, 3, 0, 2]
    };
    let (directory, _) = media::directory(&records, &order, true);
    std::fs::write(output.join("DICOMDIR"), directory)?;
    for instance in 1..=count {
        let mut image = dicom_object::from_reader(std::io::Cursor::new(support::fixture(
            f64::from(instance) * 2.,
            instance,
            false,
            false,
        )))?;
        image.put_str(Tag(0x20, 0xd), VR::UI, media::STUDY);
        if phantom {
            let z = instance - 1;
            let pixels: Vec<u16> = (0..12)
                .flat_map(|y| {
                    (0..16).map(move |x| {
                        if (4..=8).contains(&x) && (4..=7).contains(&y) && (1..=3).contains(&z) {
                            100
                        } else if (11..=12).contains(&x)
                            && (1..=2).contains(&y)
                            && (2..=3).contains(&z)
                        {
                            75
                        } else {
                            0
                        }
                    })
                })
                .collect();
            image.put(DataElement::new(
                Tag(0x28, 0x10),
                VR::US,
                PrimitiveValue::from(12u16),
            ));
            image.put(DataElement::new(
                Tag(0x28, 0x11),
                VR::US,
                PrimitiveValue::from(16u16),
            ));
            image.put(DataElement::new(
                Tag(0x7fe0, 0x10),
                VR::OW,
                PrimitiveValue::U16(pixels.into()),
            ));
        }
        image.write_to_file(output.join(format!("Images/Img{instance}")))?;
    }
    println!("Wrote a generated DICOMDIR and {count} extensionless synthetic images.");
    if phantom {
        println!(
            "Generated boxes at source values 100 and 50 occupy 60 and 8 voxels (240 and 32 mm³). No anatomy is represented."
        );
    }
    if segments {
        generated_segments::write(&output)?;
        println!(
            "Generated read-only masks: IDs 9/65535/2 contain 8/60/24 cells (32/240/96 mm³), with 8 overlap cells. The wrong-reference fixture must be refused."
        );
    }
    Ok(())
}
