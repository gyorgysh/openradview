// SPDX-License-Identifier: GPL-3.0-or-later
//! Original real-world value mapping phantoms, without anatomy or patient attributes.
#[path = "../tests/support/real_world.rs"]
mod phantom;
use dicom_core::{DataElement, PrimitiveValue, Tag, VR};
use dicom_object::DefaultDicomObject;
use std::{fs, io::Write, path::Path};

fn write(
    folder: &Path,
    name: &str,
    mut object: DefaultDicomObject,
    family: u32,
) -> Result<(), Box<dyn std::error::Error>> {
    let uid = format!("2.25.6363{family}");
    object.put_str(Tag(0x0008, 0x0018), VR::UI, &uid);
    object.update_meta(|meta| meta.media_storage_sop_instance_uid = uid);
    object.put_str(Tag(0x0020, 0x000e), VR::UI, format!("2.25.63630{family}"));
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
        .ok_or("Pass a fresh mapping fixture directory")?;
    if args.next().is_some() {
        return Err("Pass only the output directory".into());
    }
    let folder = Path::new(&folder);
    fs::create_dir_all(folder)?;
    let syntax = "1.2.840.10008.1.2.1";
    write(
        folder,
        "mapped-integer-counts.dcm",
        phantom::integer(syntax, phantom::item(0.5, -10., Some((0, 255)), None)),
        1,
    )?;
    write(
        folder,
        "mapped-float-range.dcm",
        phantom::float(syntax, phantom::item(2., 1., None, Some((-10., 10.)))),
        2,
    )?;
    let mut custom = phantom::item(0.5, -10., Some((0, 255)), None);
    custom.put(phantom::units("99LOCAL", "GENERATED_UNIT"));
    write(
        folder,
        "mapped-custom-unit.dcm",
        phantom::integer(syntax, custom),
        3,
    )?;
    let mut alternatives = phantom::integer(syntax, phantom::item(1., 0., Some((0, 255)), None));
    alternatives.put(phantom::sequence(
        Tag(0x0040, 0x9096),
        vec![
            phantom::item(1., 0., Some((0, 255)), None),
            phantom::item(0.5, 0., Some((0, 255)), None),
        ],
    ));
    write(folder, "invalid-alternative-mappings.dcm", alternatives, 4)?;
    // A 16x16 ramp with a supplied overlay outline and a circular shutter.
    let mut layered = phantom::integer(syntax, phantom::item(1., 0., Some((0, 255)), None));
    layered.remove_element(Tag(0x0040, 0x9096));
    for tag in [Tag(0x0028, 0x0010), Tag(0x0028, 0x0011)] {
        layered.put(DataElement::new(tag, VR::US, PrimitiveValue::from(16u16)));
    }
    layered.put(DataElement::new(
        Tag(0x7fe0, 0x0010),
        VR::OB,
        PrimitiveValue::U8((0..256).map(|v| v as u8).collect()),
    ));
    for (element, value) in [(0x0010, 8u16), (0x0011, 8), (0x0100, 1), (0x0102, 0)] {
        layered.put(DataElement::new(
            Tag(0x6000, element),
            VR::US,
            PrimitiveValue::from(value),
        ));
    }
    layered.put_str(Tag(0x6000, 0x0040), VR::CS, "G");
    layered.put(DataElement::new(
        Tag(0x6000, 0x0050),
        VR::SS,
        PrimitiveValue::I16(vec![5, 5].into()),
    ));
    // An 8x8 square outline: full first/last rows, edge columns between.
    let bits: Vec<u8> = (0..8)
        .map(|row| if row == 0 || row == 7 { 0xff } else { 0x81 })
        .collect();
    layered.put(DataElement::new(
        Tag(0x6000, 0x3000),
        VR::OB,
        PrimitiveValue::U8(bits.into()),
    ));
    layered.put_str(Tag(0x0018, 0x1600), VR::CS, "CIRCULAR");
    layered.put_str(Tag(0x0018, 0x1610), VR::IS, "8\\8");
    layered.put_str(Tag(0x0018, 0x1612), VR::IS, "7");
    layered.put(DataElement::new(
        Tag(0x0018, 0x1622),
        VR::US,
        PrimitiveValue::from(16384u16),
    ));
    write(folder, "layers-overlay-shutter.dcm", layered, 5)?;
    // A presentation state for the integer series: inverse, 90° and a flip.
    let mut state = dicom_object::InMemDicomObject::new_empty();
    state.put_str(Tag(0x0008, 0x0016), VR::UI, "1.2.840.10008.5.1.4.1.1.11.1");
    state.put_str(Tag(0x0008, 0x0018), VR::UI, "2.25.63639");
    let mut image = dicom_object::InMemDicomObject::new_empty();
    image.put_str(Tag(0x0008, 0x1150), VR::UI, "1.2.840.10008.5.1.4.1.1.7");
    image.put_str(Tag(0x0008, 0x1155), VR::UI, "2.25.63631");
    let mut series = dicom_object::InMemDicomObject::new_empty();
    series.put_str(Tag(0x0020, 0x000e), VR::UI, "2.25.636301");
    series.put(phantom::sequence(Tag(0x0008, 0x1140), vec![image]));
    state.put(phantom::sequence(Tag(0x0008, 0x1115), vec![series]));
    state.put_str(Tag(0x2050, 0x0020), VR::CS, "INVERSE");
    let mut voi = dicom_object::InMemDicomObject::new_empty();
    voi.put_str(Tag(0x0028, 0x1050), VR::DS, "0");
    voi.put_str(Tag(0x0028, 0x1051), VR::DS, "40");
    state.put(phantom::sequence(Tag(0x0028, 0x3110), vec![voi]));
    let mut area = dicom_object::InMemDicomObject::new_empty();
    area.put(DataElement::new(
        Tag(0x0070, 0x0052),
        VR::SL,
        PrimitiveValue::I32(vec![1, 1].into()),
    ));
    area.put(DataElement::new(
        Tag(0x0070, 0x0053),
        VR::SL,
        PrimitiveValue::I32(vec![2, 2].into()),
    ));
    area.put_str(Tag(0x0070, 0x0100), VR::CS, "SCALE TO FIT");
    state.put(phantom::sequence(Tag(0x0070, 0x005a), vec![area]));
    state.put(DataElement::new(
        Tag(0x0070, 0x0042),
        VR::US,
        PrimitiveValue::from(90u16),
    ));
    state.put_str(Tag(0x0070, 0x0041), VR::CS, "Y");
    let state = state.with_meta(
        dicom_object::meta::FileMetaTableBuilder::new()
            .transfer_syntax("1.2.840.10008.1.2.1")
            .media_storage_sop_class_uid("1.2.840.10008.5.1.4.1.1.11.1")
            .media_storage_sop_instance_uid("2.25.63639"),
    )?;
    let mut bytes = Vec::new();
    state.write_all(&mut bytes)?;
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(folder.join("state-integer-counts.pre"))?
        .write_all(&bytes)?;
    println!(
        "Wrote original real-world mapping inputs: integer counts, a float range, a private unit, an alternative-mapping refusal, overlay/shutter layers and a presentation state; no patient data."
    );
    Ok(())
}
