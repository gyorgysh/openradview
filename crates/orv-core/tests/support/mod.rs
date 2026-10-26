// SPDX-License-Identifier: GPL-3.0-or-later
//! Generated fixture. All labels and pixels are synthetic, with no real patient data.
use dicom_core::{DataElement, PrimitiveValue, Tag, VR};
use dicom_object::{InMemDicomObject, meta::FileMetaTableBuilder};

pub fn fixture(z: f64, instance: i32, monochrome1: bool, large: bool) -> Vec<u8> {
    let side: u16 = if large { 8192 } else { 2 };
    let obj = InMemDicomObject::from_element_iter([
        DataElement::new(Tag(0x0008, 0x0016), VR::UI, "1.2.840.10008.5.1.4.1.1.2"),
        DataElement::new(
            Tag(0x0008, 0x0018),
            VR::UI,
            format!("2.25.314159{}", instance.abs()),
        ),
        DataElement::new(Tag(0x0008, 0x0060), VR::CS, "CT"),
        DataElement::new(Tag(0x0010, 0x0010), VR::PN, "Synthetic^Only"),
        DataElement::new(Tag(0x0010, 0x0020), VR::LO, "SYNTHETIC-NO-PATIENT"),
        DataElement::new(Tag(0x0020, 0x000e), VR::UI, "2.25.314159265358979"),
        DataElement::new(Tag(0x0020, 0x0052), VR::UI, "2.25.3141592653589793238"),
        DataElement::new(Tag(0x0020, 0x0011), VR::IS, "7"),
        DataElement::new(Tag(0x0020, 0x0013), VR::IS, instance.to_string()),
        DataElement::new(Tag(0x0020, 0x0032), VR::DS, format!("0\\0\\{z}")),
        DataElement::new(Tag(0x0020, 0x0037), VR::DS, "1\\0\\0\\0\\1\\0"),
        DataElement::new(Tag(0x0028, 0x0030), VR::DS, "2\\1"),
        DataElement::new(Tag(0x0028, 0x0010), VR::US, PrimitiveValue::from(side)),
        DataElement::new(Tag(0x0028, 0x0011), VR::US, PrimitiveValue::from(side)),
        DataElement::new(Tag(0x0028, 0x0002), VR::US, PrimitiveValue::from(1u16)),
        DataElement::new(
            Tag(0x0028, 0x0004),
            VR::CS,
            if monochrome1 {
                "MONOCHROME1"
            } else {
                "MONOCHROME2"
            },
        ),
        DataElement::new(Tag(0x0028, 0x0100), VR::US, PrimitiveValue::from(16u16)),
        DataElement::new(Tag(0x0028, 0x0101), VR::US, PrimitiveValue::from(16u16)),
        DataElement::new(Tag(0x0028, 0x0102), VR::US, PrimitiveValue::from(15u16)),
        DataElement::new(Tag(0x0028, 0x0103), VR::US, PrimitiveValue::from(1u16)),
        DataElement::new(Tag(0x0028, 0x1050), VR::DS, "0"),
        DataElement::new(Tag(0x0028, 0x1051), VR::DS, "200"),
        DataElement::new(Tag(0x0028, 0x1052), VR::DS, "-100"),
        DataElement::new(Tag(0x0028, 0x1053), VR::DS, "2"),
        DataElement::new(
            Tag(0x7fe0, 0x0010),
            VR::OW,
            PrimitiveValue::U16(vec![0u16, 50, 100, 65535].into()),
        ),
    ]);
    let file = obj
        .with_meta(FileMetaTableBuilder::new().transfer_syntax("1.2.840.10008.1.2.1"))
        .unwrap();
    let mut bytes = Vec::new();
    file.write_all(&mut bytes).unwrap();
    bytes
}
