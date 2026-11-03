// SPDX-License-Identifier: GPL-3.0-or-later
//! Original Float Pixel Data phantoms, without anatomy or patient attributes.
#[path = "../tests/support/float.rs"]
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
    let uid = format!("2.25.6262{family}{instance}");
    object.put_str(Tag(0x0008, 0x0018), VR::UI, &uid);
    object.update_meta(|meta| meta.media_storage_sop_instance_uid = uid);
    object.put_str(Tag(0x0020, 0x000e), VR::UI, format!("2.25.62620{family}"));
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
fn ramp(object: &mut DefaultDicomObject, size: u16, values: Vec<f32>) {
    for tag in [Tag(0x0028, 0x0010), Tag(0x0028, 0x0011)] {
        object.put(DataElement::new(tag, VR::US, PrimitiveValue::from(size)));
    }
    object.put(DataElement::new(
        Tag(0x7fe0, 0x0008),
        VR::OF,
        PrimitiveValue::F32(values.into()),
    ));
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let folder = args.next().ok_or("Pass a fresh float fixture directory")?;
    if args.next().is_some() {
        return Err("Pass only the output directory".into());
    }
    let folder = Path::new(&folder);
    fs::create_dir_all(folder)?;
    // A 16x16 stack whose values rise along x by 0.125 and along z by 100.
    for z in 0..3u32 {
        let mut object = phantom::fixture(f64::from(z) * 2., z + 1, "1.2.840.10008.1.2.1", 1);
        let values = (0..256)
            .map(|i| (i % 16) as f32 * 0.125 + z as f32 * 100.)
            .collect();
        ramp(&mut object, 16, values);
        write(
            folder,
            &format!("float-stack-{}.dcm", z + 1),
            object,
            1,
            z + 1,
        )?;
    }
    let mut big = phantom::fixture(0., 1, "1.2.840.10008.1.2.2", 1);
    ramp(
        &mut big,
        4,
        (0..16).map(|i| if i < 8 { -1e30 } else { 3e38 }).collect(),
    );
    write(folder, "float-big-endian-extremes.dcm", big, 2, 1)?;
    let mut missing = phantom::fixture(0., 1, "1.2.840.10008.1.2.1", 1);
    let mut values: Vec<f32> = (0..16).map(|i| i as f32).collect();
    values[5] = f32::NAN;
    values[6] = f32::INFINITY;
    values[9] = -999.;
    ramp(&mut missing, 4, values);
    missing.put(DataElement::new(
        Tag(0x0028, 0x0122),
        VR::FL,
        PrimitiveValue::from(-999f32),
    ));
    write(folder, "float-missing.dcm", missing, 3, 1)?;
    let mut window = phantom::fixture(0., 1, "1.2.840.10008.1.2.1", 1);
    ramp(&mut window, 4, (0..16).map(|i| i as f32 / 16.).collect());
    window.put_str(Tag(0x0028, 0x1050), VR::DS, "0.5");
    window.put_str(Tag(0x0028, 0x1051), VR::DS, "0.5");
    window.put_str(Tag(0x0028, 0x1056), VR::CS, "LINEAR_EXACT");
    write(folder, "float-fractional-window.dcm", window, 4, 1)?;
    let mut refused = phantom::fixture(0., 1, "1.2.840.10008.1.2.1", 1);
    refused.put_str(Tag(0x0028, 0x1053), VR::DS, "2");
    refused.put_str(Tag(0x0028, 0x1052), VR::DS, "0");
    write(folder, "invalid-float-rescale.dcm", refused, 5, 1)?;
    println!(
        "Wrote original Float Pixel Data inputs: a stack, byte-order extremes, missing samples, a fractional window and a refusal case; no patient data."
    );
    Ok(())
}
