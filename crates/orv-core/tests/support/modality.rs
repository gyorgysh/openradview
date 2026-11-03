// SPDX-License-Identifier: GPL-3.0-or-later
//! Original numeric modality-table phantom; no anatomy or patient attributes.
use dicom_core::{DataElement, DicomValue, Length, PrimitiveValue, Tag, VR};
use dicom_object::{DefaultDicomObject, InMemDicomObject as Object, meta::FileMetaTableBuilder};
pub fn sequence(tag: Tag, items: Vec<Object>) -> dicom_object::mem::InMemElement {
    DataElement::new(
        tag,
        VR::SQ,
        DicomValue::new_sequence(items, Length::UNDEFINED),
    )
}
pub fn fixture(
    z: f64,
    instance: u32,
    syntax: &str,
    signed: bool,
    stored: u16,
    lut_bits: u16,
    packed: bool,
) -> DefaultDicomObject {
    let mut object = Object::new_empty();
    for (tag, vr, value) in [
        (Tag(0x0008, 0x0016), VR::UI, "1.2.840.10008.5.1.4.1.1.7"),
        (Tag(0x0008, 0x0060), VR::CS, "OT"),
        (Tag(0x0020, 0x000d), VR::UI, "2.25.616100"),
        (Tag(0x0020, 0x000e), VR::UI, "2.25.616101"),
        (Tag(0x0020, 0x0052), VR::UI, "2.25.616102"),
        (Tag(0x0020, 0x0011), VR::IS, "1"),
        (Tag(0x0020, 0x0037), VR::DS, "1\\0\\0\\0\\1\\0"),
        (Tag(0x0028, 0x0030), VR::DS, "2\\1"),
        (Tag(0x0028, 0x0004), VR::CS, "MONOCHROME2"),
        (Tag(0x0028, 0x1050), VR::DS, "32768"),
        (Tag(0x0028, 0x1051), VR::DS, "65536"),
    ] {
        object.put_str(tag, vr, value);
    }
    object.put_str(Tag(0x0008, 0x0018), VR::UI, format!("2.25.61611{instance}"));
    object.put_str(Tag(0x0020, 0x0013), VR::IS, instance.to_string());
    object.put_str(Tag(0x0020, 0x0032), VR::DS, format!("0\\0\\{z}"));
    let allocated = if stored <= 8 {
        8
    } else if stored <= 16 {
        16
    } else {
        32
    };
    for (tag, value) in [
        (Tag(0x0028, 0x0002), 1u16),
        (Tag(0x0028, 0x0010), 2),
        (Tag(0x0028, 0x0011), 2),
        (Tag(0x0028, 0x0100), allocated),
        (Tag(0x0028, 0x0101), stored),
        (Tag(0x0028, 0x0102), stored - 1),
        (Tag(0x0028, 0x0103), u16::from(signed)),
    ] {
        object.put(DataElement::new(tag, VR::US, PrimitiveValue::from(value)));
    }
    let raw = if signed {
        [-2i64, -1, 0, 1]
    } else {
        [0, 1, 2, 3]
    }
    .map(|v| (v as u64 & ((1u64 << stored) - 1)) as u32);
    let pixels = match allocated {
        8 => PrimitiveValue::U8(raw.map(|v| v as u8).to_vec().into()),
        16 => PrimitiveValue::U16(raw.map(|v| v as u16).to_vec().into()),
        _ => PrimitiveValue::U16(
            raw.iter()
                .flat_map(|value| [*value as u16, (*value >> 16) as u16])
                .collect(),
        ),
    };
    object.put(DataElement::new(
        Tag(0x7fe0, 0x0010),
        if allocated == 8 { VR::OB } else { VR::OW },
        pixels,
    ));
    let descriptor = if signed {
        DataElement::new(
            Tag(0x0028, 0x3002),
            VR::SS,
            PrimitiveValue::I16(vec![4, -2, lut_bits as i16].into()),
        )
    } else {
        DataElement::new(
            Tag(0x0028, 0x3002),
            VR::US,
            PrimitiveValue::U16(vec![4, 0, lut_bits].into()),
        )
    };
    let values = if lut_bits == 8 {
        vec![1u16, 11, 101, 255]
    } else {
        vec![0, 100, 900, 65535]
    };
    object.put(sequence(
        Tag(0x0028, 0x3000),
        vec![Object::from_element_iter([
            descriptor,
            DataElement::new(Tag(0x0028, 0x3004), VR::LO, "HU"),
            DataElement::new(Tag(0x0028, 0x3003), VR::LO, "Generated private explanation"),
            DataElement::new(
                Tag(0x0028, 0x3006),
                VR::OW,
                if packed {
                    PrimitiveValue::U16(
                        values
                            .as_chunks::<2>()
                            .0
                            .iter()
                            .map(|pair| pair[0] | (pair[1] << 8))
                            .collect(),
                    )
                } else {
                    PrimitiveValue::U16(values.into())
                },
            ),
        ])],
    ));
    object
        .with_meta(FileMetaTableBuilder::new().transfer_syntax(syntax))
        .unwrap()
}
