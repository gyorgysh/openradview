// SPDX-License-Identifier: GPL-3.0-or-later
//! Generated boxes for the reader's documented subset; no anatomy or real source data.
use dicom_core::{DataElement, DicomValue, Length, PrimitiveValue, Tag, VR};
use dicom_object::InMemDicomObject as Object;
use std::path::Path;
fn seq(tag: Tag, items: Vec<Object>) -> dicom_object::mem::InMemElement {
    DataElement::new(
        tag,
        VR::SQ,
        DicomValue::new_sequence(items, Length::UNDEFINED),
    )
}
fn us(tag: Tag, value: u16) -> dicom_object::mem::InMemElement {
    DataElement::new(tag, VR::US, PrimitiveValue::from(value))
}
pub fn write(output: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let mut object = dicom_object::from_reader(std::io::Cursor::new(super::support::fixture(
        2., 701, false, false,
    )))?;
    let class = "1.2.840.10008.5.1.4.1.1.66.4";
    object.put_str(Tag(8, 0x16), VR::UI, class);
    object.put_str(Tag(8, 0x18), VR::UI, "2.25.202611010701");
    object.update_meta(|meta| {
        meta.media_storage_sop_class_uid = class.into();
        meta.media_storage_sop_instance_uid = "2.25.202611010701".into();
    });
    object.put_str(Tag(8, 0x60), VR::CS, "SEG");
    object.put_str(Tag(0x20, 0xd), VR::UI, super::media::STUDY);
    object.put_str(Tag(0x20, 0xe), VR::UI, "2.25.202611010702");
    object.put_str(Tag(0x62, 1), VR::CS, "BINARY");
    object.put_str(Tag(0x62, 0x13), VR::CS, "YES");
    object.put_str(Tag(0x28, 8), VR::IS, "7");
    object.put_str(Tag(0x28, 4), VR::CS, "MONOCHROME2");
    for (tag, value) in [
        (0x10, 12),
        (0x11, 16),
        (2, 1),
        (0x100, 1),
        (0x101, 1),
        (0x102, 0),
        (0x103, 0),
    ] {
        object.put(us(Tag(0x28, tag), value));
    }
    object.put(seq(
        Tag(0x62, 2),
        [9, u16::MAX, 2]
            .map(|number| {
                Object::from_element_iter([
                    us(Tag(0x62, 4), number),
                    DataElement::new(Tag(0x62, 5), VR::LO, format!("Generated box {number}")),
                    DataElement::new(Tag(0x62, 8), VR::CS, "MANUAL"),
                ])
            })
            .into(),
    ));
    object.put(seq(
        Tag(0x5200, 0x9229),
        vec![Object::from_element_iter([
            seq(
                Tag(0x20, 0x9116),
                vec![Object::from_element_iter([DataElement::new(
                    Tag(0x20, 0x37),
                    VR::DS,
                    "1\\0\\0\\0\\1\\0",
                )])],
            ),
            seq(
                Tag(0x28, 0x9110),
                vec![Object::from_element_iter([DataElement::new(
                    Tag(0x28, 0x30),
                    VR::DS,
                    "2\\1",
                )])],
            ),
        ])],
    ));
    let frames = [
        (2, 4),
        (u16::MAX, 2),
        (9, 3),
        (u16::MAX, 4),
        (2, 3),
        (9, 4),
        (u16::MAX, 3),
    ];
    let mut groups = Vec::new();
    let mut pixels = vec![0u8; 7 * 192 / 8];
    for (f, (number, instance)) in frames.into_iter().enumerate() {
        let source = Object::from_element_iter([
            DataElement::new(Tag(8, 0x1150), VR::UI, super::media::IMAGE_CLASS),
            DataElement::new(Tag(8, 0x1155), VR::UI, format!("2.25.314159{instance}")),
            DataElement::new(Tag(0x28, 0x135a), VR::CS, "YES"),
        ]);
        groups.push(Object::from_element_iter([
            seq(
                Tag(0x62, 0xa),
                vec![Object::from_element_iter([us(Tag(0x62, 0xb), number)])],
            ),
            seq(
                Tag(0x20, 0x9113),
                vec![Object::from_element_iter([DataElement::new(
                    Tag(0x20, 0x32),
                    VR::DS,
                    format!("0\\0\\{}", instance * 2),
                )])],
            ),
            seq(
                Tag(8, 0x9124),
                vec![Object::from_element_iter([seq(
                    Tag(8, 0x2112),
                    vec![source],
                )])],
            ),
        ]));
        for y in 0..12 {
            for x in 0..16 {
                let inside = match number {
                    9 => (11..=12).contains(&x) && (1..=2).contains(&y),
                    u16::MAX => (4..=8).contains(&x) && (4..=7).contains(&y),
                    _ => (7..=10).contains(&x) && (6..=8).contains(&y),
                };
                if inside {
                    let bit = f * 192 + y * 16 + x;
                    pixels[bit / 8] |= 1 << (bit % 8);
                }
            }
        }
    }
    object.put(seq(Tag(0x5200, 0x9230), groups.clone()));
    object.put(DataElement::new(
        Tag(0x7fe0, 0x10),
        VR::OB,
        PrimitiveValue::U8(pixels.into()),
    ));
    object.write_to_file(output.join("supplied-masks.dcm"))?;
    let mut derivation = groups[0]
        .element(Tag(8, 0x9124))?
        .value()
        .items()
        .ok_or("Generated derivation missing")?
        .to_vec();
    let mut sources = derivation[0]
        .element(Tag(8, 0x2112))?
        .value()
        .items()
        .ok_or("Generated source missing")?
        .to_vec();
    sources[0].put_str(Tag(8, 0x1155), VR::UI, "2.25.999999999");
    derivation[0].put(seq(Tag(8, 0x2112), sources));
    groups[0].put(seq(Tag(8, 0x9124), derivation));
    object.put(seq(Tag(0x5200, 0x9230), groups));
    object.write_to_file(output.join("wrong-reference-masks.dcm"))?;
    Ok(())
}
