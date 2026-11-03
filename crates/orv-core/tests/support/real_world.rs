// SPDX-License-Identifier: GPL-3.0-or-later
//! Original numeric real-world mapping phantoms; no anatomy or patient attributes.
use dicom_core::{DataElement, DicomValue, Length, PrimitiveValue, Tag, VR};
use dicom_object::{DefaultDicomObject, InMemDicomObject as Object, meta::FileMetaTableBuilder};

pub fn sequence(tag: Tag, items: Vec<Object>) -> dicom_object::mem::InMemElement {
    DataElement::new(
        tag,
        VR::SQ,
        DicomValue::new_sequence(items, Length::UNDEFINED),
    )
}

pub fn units(scheme: &str, value: &str) -> dicom_object::mem::InMemElement {
    let mut code = Object::new_empty();
    code.put_str(Tag(0x0008, 0x0100), VR::SH, value);
    code.put_str(Tag(0x0008, 0x0102), VR::SH, scheme);
    code.put_str(Tag(0x0008, 0x0104), VR::LO, "Generated unit meaning");
    sequence(Tag(0x0040, 0x08ea), vec![code])
}

/// One linear item. Integer `range` uses the stored-value attributes; otherwise
/// the double-float range attributes are used.
pub fn item(
    slope: f64,
    intercept: f64,
    range: Option<(i16, i16)>,
    double: Option<(f64, f64)>,
) -> Object {
    let mut item = Object::new_empty();
    item.put(DataElement::new(
        Tag(0x0040, 0x9225),
        VR::FD,
        PrimitiveValue::from(slope),
    ));
    item.put(DataElement::new(
        Tag(0x0040, 0x9224),
        VR::FD,
        PrimitiveValue::from(intercept),
    ));
    if let Some((first, last)) = range {
        item.put(DataElement::new(
            Tag(0x0040, 0x9216),
            VR::SS,
            PrimitiveValue::from(first),
        ));
        item.put(DataElement::new(
            Tag(0x0040, 0x9211),
            VR::SS,
            PrimitiveValue::from(last),
        ));
    }
    if let Some((first, last)) = double {
        item.put(DataElement::new(
            Tag(0x0040, 0x9214),
            VR::FD,
            PrimitiveValue::from(first),
        ));
        item.put(DataElement::new(
            Tag(0x0040, 0x9213),
            VR::FD,
            PrimitiveValue::from(last),
        ));
    }
    item.put_str(Tag(0x0040, 0x9210), VR::SH, "GENERATED_LABEL");
    item.put_str(Tag(0x0028, 0x3003), VR::LO, "Generated private explanation");
    item.put(units("UCUM", "{counts}"));
    item
}

/// A 2x2 image of stored codes `[0, 1, 2, 255]` (8-bit unsigned) with one item.
pub fn integer(syntax: &str, mapping: Object) -> DefaultDicomObject {
    let mut object = base(syntax);
    for (tag, value) in [
        (Tag(0x0028, 0x0100), 8u16),
        (Tag(0x0028, 0x0101), 8),
        (Tag(0x0028, 0x0102), 7),
        (Tag(0x0028, 0x0103), 0),
    ] {
        object.put(DataElement::new(tag, VR::US, PrimitiveValue::from(value)));
    }
    object.put(DataElement::new(
        Tag(0x7fe0, 0x0010),
        VR::OB,
        PrimitiveValue::U8(vec![0, 1, 2, 255].into()),
    ));
    object.put(sequence(Tag(0x0040, 0x9096), vec![mapping]));
    object
}

/// A 2x2 float image of stored values `[-20, -1.5, 0.25, 30]` with one item.
pub fn float(syntax: &str, mapping: Object) -> DefaultDicomObject {
    let mut object = base(syntax);
    object.put(DataElement::new(
        Tag(0x0028, 0x0100),
        VR::US,
        PrimitiveValue::from(32u16),
    ));
    object.put(sequence(Tag(0x0040, 0x9096), vec![mapping]));
    object.put(DataElement::new(
        Tag(0x7fe0, 0x0008),
        VR::OF,
        PrimitiveValue::F32(vec![-20., -1.5, 0.25, 30.].into()),
    ));
    object
}

fn base(syntax: &str) -> DefaultDicomObject {
    let mut object = Object::new_empty();
    for (tag, vr, value) in [
        (Tag(0x0008, 0x0016), VR::UI, "1.2.840.10008.5.1.4.1.1.7"),
        (Tag(0x0008, 0x0018), VR::UI, "2.25.636111"),
        (Tag(0x0008, 0x0060), VR::CS, "OT"),
        (Tag(0x0020, 0x000d), VR::UI, "2.25.636100"),
        (Tag(0x0020, 0x000e), VR::UI, "2.25.636101"),
        (Tag(0x0020, 0x0052), VR::UI, "2.25.636102"),
        (Tag(0x0020, 0x0011), VR::IS, "1"),
        (Tag(0x0020, 0x0013), VR::IS, "1"),
        (Tag(0x0020, 0x0032), VR::DS, "0\\0\\0"),
        (Tag(0x0020, 0x0037), VR::DS, "1\\0\\0\\0\\1\\0"),
        (Tag(0x0028, 0x0030), VR::DS, "1\\1"),
        (Tag(0x0028, 0x0004), VR::CS, "MONOCHROME2"),
    ] {
        object.put_str(tag, vr, value);
    }
    for (tag, value) in [
        (Tag(0x0028, 0x0002), 1u16),
        (Tag(0x0028, 0x0010), 2),
        (Tag(0x0028, 0x0011), 2),
    ] {
        object.put(DataElement::new(tag, VR::US, PrimitiveValue::from(value)));
    }
    object
        .with_meta(FileMetaTableBuilder::new().transfer_syntax(syntax))
        .unwrap()
}
