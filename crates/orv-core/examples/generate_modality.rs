// SPDX-License-Identifier: GPL-3.0-or-later
//! Original scalar lookup phantoms, without anatomy or patient attributes.
#[path = "../tests/support/modality.rs"]
mod phantom;
use dicom_core::{DataElement, PrimitiveValue, Tag, VR};
use dicom_object::DefaultDicomObject;
use std::{fs, io::Write, path::Path};

fn write(
    folder: &Path,
    name: &str,
    mut object: DefaultDicomObject,
    family: u32,
    instance: u32,
) -> Result<(), Box<dyn std::error::Error>> {
    let uid = format!("2.25.6162{family}{instance}");
    object.put_str(Tag(0x0008, 0x0018), VR::UI, &uid);
    object.update_meta(|meta| meta.media_storage_sop_instance_uid = uid);
    object.put_str(Tag(0x0020, 0x000e), VR::UI, format!("2.25.61620{family}"));
    object.put_str(Tag(0x0020, 0x0011), VR::IS, family.to_string());
    let mut bytes = Vec::new();
    object.write_all(&mut bytes)?;
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(folder.join(name))?
        .write_all(&bytes)?;
    Ok(())
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let folder = args
        .next()
        .ok_or("Pass a fresh modality fixture directory")?;
    if args.next().is_some() {
        return Err("Pass only the output directory".into());
    }
    let folder = Path::new(&folder);
    fs::create_dir_all(folder)?;
    for z in 0..3 {
        write(
            folder,
            &format!("mapped-stack-{}.dcm", z + 1),
            phantom::fixture(
                f64::from(z) * 2.,
                z + 1,
                "1.2.840.10008.1.2.1",
                true,
                12,
                16,
                false,
            ),
            1,
            z + 1,
        )?;
    }
    let mut small = phantom::fixture(0., 1, "1.2.840.10008.1.2.1", false, 8, 8, true);
    small.put_str(Tag(0x0028, 0x1050), VR::DS, "128");
    small.put_str(Tag(0x0028, 0x1051), VR::DS, "256");
    write(folder, "mapped-8bit.dcm", small.clone(), 2, 1)?;
    write(
        folder,
        "mapped-big-32bit.dcm",
        phantom::fixture(0., 1, "1.2.840.10008.1.2.2", true, 32, 16, false),
        3,
        1,
    )?;
    let mut padded = phantom::fixture(0., 1, "1.2.840.10008.1.2.1", true, 12, 16, false);
    padded.put(DataElement::new(
        Tag(0x0028, 0x0120),
        VR::SS,
        PrimitiveValue::from(-1i16),
    ));
    write(folder, "mapped-padding.dcm", padded, 4, 1)?;
    for (name, unit, family) in [
        ("mapped-density.dcm", "OD", 5),
        ("mapped-unknown.dcm", "CUSTOM_UNITS", 6),
    ] {
        let mut object = small.clone();
        object.update_value(Tag(0x0028, 0x3000), |value| {
            value.items_mut().unwrap()[0].put_str(Tag(0x0028, 0x3004), VR::LO, unit);
        });
        write(folder, name, object, family, 1)?;
    }
    let mut conflict = small;
    conflict.put_str(Tag(0x0028, 0x1053), VR::DS, "1");
    write(folder, "invalid-conflict.dcm", conflict, 7, 1)?;
    println!(
        "Wrote original numeric modality-table inputs, a spatial stack, padding, units and an explicit refusal case; no patient data."
    );
    Ok(())
}
