// SPDX-License-Identifier: GPL-3.0-or-later
//! Original numeric acquisition and cine phantoms, with no patient attributes.
use dicom_core::{DataElement, DicomValue, Length, PrimitiveValue, Tag, VR};
use dicom_object::{DefaultDicomObject, InMemDicomObject as Object, meta::FileMetaTableBuilder};
pub fn sequence(tag: Tag, items: Vec<Object>) -> dicom_object::mem::InMemElement {
    DataElement::new(
        tag,
        VR::SQ,
        DicomValue::new_sequence(items, Length::UNDEFINED),
    )
}
fn base(class: &str, frames: u32) -> DefaultDicomObject {
    let mut object = Object::new_empty();
    for (tag, vr, value) in [
        (Tag(0x0008, 0x0016), VR::UI, class),
        (Tag(0x0008, 0x0018), VR::UI, "2.25.5150001"),
        (Tag(0x0008, 0x0060), VR::CS, "MR"),
        (Tag(0x0020, 0x000d), VR::UI, "2.25.515000"),
        (Tag(0x0020, 0x000e), VR::UI, "2.25.515010"),
        (Tag(0x0020, 0x0052), VR::UI, "2.25.515099"),
        (Tag(0x0028, 0x0004), VR::CS, "MONOCHROME2"),
        (Tag(0x0020, 0x0011), VR::IS, "1"),
        (Tag(0x0020, 0x0013), VR::IS, "1"),
        (Tag(0x0028, 0x1052), VR::DS, "0"),
        (Tag(0x0028, 0x1053), VR::DS, "1"),
        (Tag(0x0028, 0x1050), VR::DS, "500"),
        (Tag(0x0028, 0x1051), VR::DS, "1000"),
    ] {
        object.put_str(tag, vr, value);
    }
    for (tag, value) in [
        (Tag(0x0028, 0x0002), 1u16),
        (Tag(0x0028, 0x0010), 12),
        (Tag(0x0028, 0x0011), 16),
        (Tag(0x0028, 0x0100), 16),
        (Tag(0x0028, 0x0101), 16),
        (Tag(0x0028, 0x0102), 15),
        (Tag(0x0028, 0x0103), 0),
    ] {
        object.put(DataElement::new(tag, VR::US, PrimitiveValue::from(value)));
    }
    object.put_str(Tag(0x0028, 0x0008), VR::IS, frames.to_string());
    object
        .with_meta(FileMetaTableBuilder::new().transfer_syntax("1.2.840.10008.1.2.1"))
        .unwrap()
}
pub fn enhanced() -> DefaultDicomObject {
    let mut object = base("1.2.840.10008.5.1.4.1.1.4.1", 12);
    object.put(sequence(
        Tag(0x5200, 0x9229),
        vec![Object::from_element_iter([
            sequence(
                Tag(0x0020, 0x9116),
                vec![Object::from_element_iter([DataElement::new(
                    Tag(0x0020, 0x0037),
                    VR::DS,
                    "1\\0\\0\\0\\1\\0",
                )])],
            ),
            sequence(
                Tag(0x0028, 0x9110),
                vec![Object::from_element_iter([DataElement::new(
                    Tag(0x0028, 0x0030),
                    VR::DS,
                    "2\\1",
                )])],
            ),
        ])],
    ));
    object.put(sequence(
        Tag(0x0020, 0x9221),
        vec![Object::from_element_iter([DataElement::new(
            Tag(0x0020, 0x9164),
            VR::UI,
            "2.25.515001",
        )])],
    ));
    object.put(sequence(
        Tag(0x0020, 0x9222),
        [
            (Tag(0x0020, 0x9128), Tag(0x0020, 0x9111)),
            (Tag(0x0018, 0x9082), Tag(0x0018, 0x9114)),
            (Tag(0x0020, 0x9057), Tag(0x0020, 0x9111)),
        ]
        .map(|(tag, group)| {
            Object::from_element_iter([
                DataElement::new(Tag(0x0020, 0x9164), VR::UI, "2.25.515001"),
                DataElement::new(
                    Tag(0x0020, 0x9165),
                    VR::AT,
                    PrimitiveValue::Tags(vec![tag].into()),
                ),
                DataElement::new(
                    Tag(0x0020, 0x9167),
                    VR::AT,
                    PrimitiveValue::Tags(vec![group].into()),
                ),
            ])
        })
        .into(),
    ));
    let mut groups = Vec::new();
    let mut pixels = Vec::new();
    for logical in [11u32, 0, 5, 8, 3, 10, 1, 7, 4, 9, 2, 6] {
        let time = logical / 6;
        let echo = logical / 3 % 2;
        let z = logical % 3;
        let actual_time = [2u32, 7][time as usize];
        let actual_echo = [4.5f64, 9.25][echo as usize];
        groups.push(Object::from_element_iter([
            sequence(
                Tag(0x0020, 0x9111),
                vec![Object::from_element_iter([
                    DataElement::new(
                        Tag(0x0020, 0x9157),
                        VR::UL,
                        PrimitiveValue::U32(vec![time + 1, echo + 1, z + 1].into()),
                    ),
                    DataElement::new(
                        Tag(0x0020, 0x9128),
                        VR::UL,
                        PrimitiveValue::from(actual_time),
                    ),
                    DataElement::new(Tag(0x0020, 0x9057), VR::UL, PrimitiveValue::from(z + 1)),
                ])],
            ),
            sequence(
                Tag(0x0018, 0x9114),
                vec![Object::from_element_iter([DataElement::new(
                    Tag(0x0018, 0x9082),
                    VR::FD,
                    PrimitiveValue::from(actual_echo),
                )])],
            ),
            sequence(
                Tag(0x0020, 0x9113),
                vec![Object::from_element_iter([DataElement::new(
                    Tag(0x0020, 0x0032),
                    VR::DS,
                    format!("0\\0\\{}", z * 3),
                )])],
            ),
        ]));
        pixels.extend((0..192).map(|i| (actual_time * 100 + (echo + 1) * 10 + z + i) as u16));
    }
    object.put(sequence(Tag(0x5200, 0x9230), groups));
    object.put(DataElement::new(
        Tag(0x7fe0, 0x0010),
        VR::OW,
        PrimitiveValue::U16(pixels.into()),
    ));
    object
}
pub fn cine(variable: bool) -> DefaultDicomObject {
    let mut object = base("1.2.840.10008.5.1.4.1.1.3.1", 5);
    object.put_str(Tag(0x0008, 0x0060), VR::CS, "US");
    let uid = if variable {
        "2.25.5150002"
    } else {
        "2.25.5150003"
    };
    object.put_str(Tag(0x0008, 0x0018), VR::UI, uid);
    object.update_meta(|meta| meta.media_storage_sop_instance_uid = uid.into());
    object.put_str(
        Tag(0x0020, 0x000e),
        VR::UI,
        if variable {
            "2.25.515020"
        } else {
            "2.25.515021"
        },
    );
    let pointer = if variable {
        Tag(0x0018, 0x1065)
    } else {
        Tag(0x0018, 0x1063)
    };
    object.put(DataElement::new(
        Tag(0x0028, 0x0009),
        VR::AT,
        PrimitiveValue::Tags(vec![pointer].into()),
    ));
    object.put_str(
        pointer,
        VR::DS,
        if variable { "0\\40\\120\\60\\80" } else { "50" },
    );
    object.put_str(Tag(0x0018, 0x1066), VR::DS, "7.5");
    object.put_str(Tag(0x0008, 0x2142), VR::IS, "2");
    object.put_str(Tag(0x0008, 0x2143), VR::IS, "4");
    let pixels: Vec<u16> = (0..5)
        .flat_map(|frame| (0..192).map(move |i| frame * 100 + i))
        .collect();
    object.put(DataElement::new(
        Tag(0x7fe0, 0x0010),
        VR::OW,
        PrimitiveValue::U16(pixels.into()),
    ));
    object
}
