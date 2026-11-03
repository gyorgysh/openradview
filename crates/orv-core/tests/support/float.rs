// SPDX-License-Identifier: GPL-3.0-or-later
//! Original numeric Float Pixel Data phantom; no anatomy or patient attributes.
use dicom_core::{DataElement, PrimitiveValue, Tag, VR};
use dicom_object::{DefaultDicomObject, InMemDicomObject as Object, meta::FileMetaTableBuilder};

/// Two columns, one row per frame keeps frame boundaries off 2x2 alignment.
pub const COLUMNS: u16 = 3;
pub const ROWS: u16 = 1;

/// Frame `f` holds `[f - 1e30, -2.5 * (f + 1), 3e38]`.
pub fn values(frames: u32) -> Vec<f32> {
    (0..frames)
        .flat_map(|frame| {
            let f = frame as f32;
            [f - 1e30, -2.5 * (f + 1.), 3e38]
        })
        .collect()
}

pub fn fixture(z: f64, instance: u32, syntax: &str, frames: u32) -> DefaultDicomObject {
    let mut object = Object::new_empty();
    for (tag, vr, value) in [
        (Tag(0x0008, 0x0016), VR::UI, "1.2.840.10008.5.1.4.1.1.7"),
        (Tag(0x0008, 0x0060), VR::CS, "OT"),
        (Tag(0x0020, 0x000d), VR::UI, "2.25.626100"),
        (Tag(0x0020, 0x000e), VR::UI, "2.25.626101"),
        (Tag(0x0020, 0x0052), VR::UI, "2.25.626102"),
        (Tag(0x0020, 0x0011), VR::IS, "1"),
        (Tag(0x0020, 0x0037), VR::DS, "1\\0\\0\\0\\1\\0"),
        (Tag(0x0028, 0x0030), VR::DS, "1\\2"),
        (Tag(0x0028, 0x0004), VR::CS, "MONOCHROME2"),
    ] {
        object.put_str(tag, vr, value);
    }
    object.put_str(Tag(0x0008, 0x0018), VR::UI, format!("2.25.62611{instance}"));
    object.put_str(Tag(0x0020, 0x0013), VR::IS, instance.to_string());
    object.put_str(Tag(0x0020, 0x0032), VR::DS, format!("0\\0\\{z}"));
    if frames > 1 {
        object.put_str(Tag(0x0028, 0x0008), VR::IS, frames.to_string());
    }
    for (tag, value) in [
        (Tag(0x0028, 0x0002), 1u16),
        (Tag(0x0028, 0x0010), ROWS),
        (Tag(0x0028, 0x0011), COLUMNS),
        (Tag(0x0028, 0x0100), 32),
    ] {
        object.put(DataElement::new(tag, VR::US, PrimitiveValue::from(value)));
    }
    object.put(DataElement::new(
        Tag(0x7fe0, 0x0008),
        VR::OF,
        PrimitiveValue::F32(values(frames).into()),
    ));
    object
        .with_meta(FileMetaTableBuilder::new().transfer_syntax(syntax))
        .unwrap()
}
