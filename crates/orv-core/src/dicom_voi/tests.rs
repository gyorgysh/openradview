// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
fn object() -> DefaultDicomObject {
    dicom_object::from_reader(std::io::Cursor::new(crate::test_support::fixture(
        0., 0, false, false,
    )))
    .unwrap()
}
fn decode(object: &DefaultDicomObject) -> Arc<crate::study::Frame> {
    let mut bytes = Vec::new();
    object.write_all(&mut bytes).unwrap();
    crate::load::decode(&bytes).unwrap().frames.remove(0)
}
fn sequence(tag: Tag, items: Vec<Object>) -> dicom_object::mem::InMemElement {
    DataElement::new(
        tag,
        VR::SQ,
        DicomValue::new_sequence(items, dicom_core::Length::UNDEFINED),
    )
}
fn table(first: i16, bits: u16, values: PrimitiveValue) -> dicom_object::mem::InMemElement {
    sequence(
        TABLES,
        vec![Object::from_element_iter([
            DataElement::new(
                Tag(0x0028, 0x3002),
                VR::SS,
                PrimitiveValue::I16(vec![4, first, bits as i16].into()),
            ),
            DataElement::new(Tag(0x0028, 0x3006), VR::OW, values),
        ])],
    )
}
#[test]
fn fractional_source_windows_preserve_scaling_and_alternative_pairs() {
    let mut object = object();
    object.put(DataElement::new(
        Tag(0x7fe0, 0x0010),
        VR::OW,
        PrimitiveValue::U16(vec![0, 1, 2, 3].into()),
    ));
    object.put_str(Tag(0x0028, 0x1052), VR::DS, "0");
    object.put_str(Tag(0x0028, 0x1053), VR::DS, "0.25");
    object.put_str(Tag(0x0028, 0x1054), VR::LO, "HU");
    object.put_str(Tag(0x0028, 0x1050), VR::DS, "0.5\\0.25");
    object.put_str(Tag(0x0028, 0x1051), VR::DS, "1\\0.5");
    object.put_str(Tag(0x0028, 0x1056), VR::CS, "LINEAR_EXACT");
    let frame = decode(&object);
    assert_eq!(&*frame.pixels, &[0., 0.25, 0.5, 0.75]);
    let scalar = frame.scalar.as_ref().unwrap();
    assert_eq!(scalar.info().unit, Unit::Hounsfield);
    assert_eq!(scalar.info().stored_bits, 16);
    assert_eq!(scalar.choices().windows().len(), 2);
    for (&value, expected) in frame.pixels.iter().zip([0., 0.25, 0.5, 0.75]) {
        assert_eq!(frame.normalize(frame.default_window, value), Some(expected));
    }
    object.put_str(Tag(0x0028, 0x1056), VR::CS, "SIGMOID");
    let frame = decode(&object);
    for (&value, expected) in frame
        .pixels
        .iter()
        .zip([0.11920292, 0.26894143, 0.5, 0.7310586])
    {
        assert!(
            (frame.normalize(frame.default_window, value).unwrap() - expected).abs() < 0.000001
        );
    }
    object.put_str(Tag(0x0028, 0x0004), VR::CS, "MONOCHROME1");
    let inverted = decode(&object);
    assert_eq!(frame.pixels, inverted.pixels);
    assert!(inverted.monochrome1);
}
#[test]
fn signed_lut_input_follows_rescale_domain_and_packed_or_word_padded_data() {
    for syntax in [
        "1.2.840.10008.1.2.1",
        "1.2.840.10008.1.2.2",
        "1.2.840.10008.1.2",
    ] {
        for packed in [false, true] {
            let mut object = object();
            object.update_meta(|meta| meta.transfer_syntax = syntax.into());
            object.put(DataElement::new(
                Tag(0x0028, 0x0103),
                VR::US,
                PrimitiveValue::from(0u16),
            ));
            object.put(DataElement::new(
                Tag(0x7fe0, 0x0010),
                VR::OW,
                PrimitiveValue::U16(vec![0, 1, 2, 3].into()),
            ));
            object.put_str(Tag(0x0028, 0x1052), VR::DS, "-2");
            object.put_str(Tag(0x0028, 0x1053), VR::DS, "1");
            object.remove_element(Tag(0x0028, 0x1050));
            object.remove_element(Tag(0x0028, 0x1051));
            object.put(table(
                -2,
                8,
                if packed {
                    PrimitiveValue::U8(vec![0, 64, 192, 255].into())
                } else {
                    PrimitiveValue::U16(vec![0, 64, 192, 255].into())
                },
            ));
            let frame = decode(&object);
            assert_eq!(&*frame.pixels, &[-2., -1., 0., 1.]);
            assert_eq!(frame.default_window, WindowLevel::lookup(0));
            assert_eq!(
                frame.scalar.as_ref().unwrap().choices().luts()[0].first(),
                -2
            );
            for (&value, expected) in frame.pixels.iter().zip([0., 64. / 255., 192. / 255., 1.]) {
                assert_eq!(
                    frame.normalize(frame.default_window, value),
                    Some(expected),
                    "{syntax} packed={packed}"
                );
            }
        }
    }
}
#[test]
fn malformed_unknown_or_inexact_presentation_is_explicitly_refused() {
    for variant in 0..6 {
        let mut object = object();
        match variant {
            0 => {
                object.put_str(Tag(0x0028, 0x1056), VR::CS, "UNSUPPORTED");
            }
            1 => {
                object.put_str(Tag(0x0028, 0x1050), VR::DS, "1\\2");
            }
            2 => {
                object.put_str(Tag(0x0028, 0x1051), VR::DS, "0.5");
            }
            3 => {
                object.put(table(-2, 12, PrimitiveValue::U16(vec![0, 1, 2, 3].into())));
            }
            4 => {
                object.put(table(-2, 8, PrimitiveValue::U16(vec![0, 256, 0, 0].into())));
            }
            _ => {
                object.put(table(-2, 8, PrimitiveValue::U16(vec![0, 1, 2, 3].into())));
                object.put_str(Tag(0x0028, 0x1053), VR::DS, "0.5");
            }
        }
        assert!(
            matches!(read(&object, 1, &|| false), Err(LoadError::Unsupported)),
            "variant {variant}"
        );
    }
    assert!(matches!(
        read(&object(), 1, &|| true),
        Err(LoadError::Cancelled)
    ));
}
#[test]
fn shared_and_per_frame_voi_macros_cannot_override_each_other() {
    let mut object = object();
    let voi = sequence(
        VOI,
        vec![Object::from_element_iter([
            DataElement::new(Tag(0x0028, 0x1050), VR::DS, "0.25"),
            DataElement::new(Tag(0x0028, 0x1051), VR::DS, "0.5"),
            DataElement::new(Tag(0x0028, 0x1056), VR::CS, "LINEAR_EXACT"),
        ])],
    );
    object.put(sequence(
        SHARED,
        vec![Object::from_element_iter([voi.clone()])],
    ));
    let shared = read(&object, 1, &|| false).unwrap();
    assert_eq!(
        shared
            .get(0)
            .unwrap()
            .choices()
            .original()
            .unwrap()
            .function,
        VoiFunction::LinearExact
    );
    object.put(sequence(PER_FRAME, vec![Object::from_element_iter([voi])]));
    assert!(matches!(
        read(&object, 1, &|| false),
        Err(LoadError::Unsupported)
    ));
}

#[test]
fn units_require_declarations_and_conflicts_are_not_silently_relabelled() {
    let mut object = object();
    object.put_str(Tag(0x0008, 0x0060), VR::CS, "CT");
    object.remove_element(Tag(0x0028, 0x1054));
    assert_eq!(
        read(&object, 1, &|| false)
            .unwrap()
            .get(0)
            .unwrap()
            .info()
            .unit,
        Unit::Unknown
    );
    object.put_str(Tag(0x0028, 0x1054), VR::LO, "HU");
    for other in ["BQML", "GML"] {
        object.put_str(Tag(0x0054, 0x1001), VR::CS, other);
        assert!(matches!(
            read(&object, 1, &|| false),
            Err(LoadError::Unsupported)
        ));
    }
    object.put_str(Tag(0x0028, 0x1054), VR::LO, "US");
    object.put_str(Tag(0x0054, 0x1001), VR::CS, "BQML");
    assert_eq!(
        read(&object, 1, &|| false)
            .unwrap()
            .get(0)
            .unwrap()
            .info()
            .unit,
        Unit::BecquerelsPerMillilitre
    );
}

#[test]
fn implicit_lut_descriptor_signedness_follows_each_frames_rescaled_domain() {
    let mut object = object();
    object.update_meta(|meta| meta.transfer_syntax = "1.2.840.10008.1.2".into());
    object.put(DataElement::new(
        Tag(0x0028, 0x0103),
        VR::US,
        PrimitiveValue::from(0u16),
    ));
    object.put(sequence(
        TABLES,
        vec![Object::from_element_iter([
            DataElement::new(
                Tag(0x0028, 0x3002),
                VR::US,
                PrimitiveValue::U16(vec![2, 65534, 8].into()),
            ),
            DataElement::new(
                Tag(0x0028, 0x3006),
                VR::OW,
                PrimitiveValue::U16(vec![10, 255].into()),
            ),
        ])],
    ));
    let frames = ["0", "-65536"].map(|intercept| {
        Object::from_element_iter([sequence(
            TRANSFORM,
            vec![Object::from_element_iter([
                DataElement::new(Tag(0x0028, 0x1053), VR::DS, "1"),
                DataElement::new(Tag(0x0028, 0x1052), VR::DS, intercept),
            ])],
        )])
    });
    object.put(sequence(PER_FRAME, frames.into()));
    let data = read(&object, 2, &|| false).unwrap();
    let a = data.get(0).unwrap();
    let b = data.get(1).unwrap();
    assert_eq!(a.choices().luts()[0].first(), 65534);
    assert_eq!(b.choices().luts()[0].first(), -2);
    assert_eq!(
        a.normalize(WindowLevel::lookup(0), 65534.),
        Some(10. / 255.)
    );
    assert_eq!(b.normalize(WindowLevel::lookup(0), -2.), Some(10. / 255.));
    assert!(!a.compatible(b));
}

#[test]
fn full_size_tables_deduplicate_and_unique_metadata_is_bounded_during_parsing() {
    let mut object = object();
    object.put_str(Tag(0x0028, 0x1053), VR::DS, "1");
    object.put_str(Tag(0x0028, 0x1052), VR::DS, "0");
    let make_table = |offset: u16| {
        sequence(
            TABLES,
            vec![Object::from_element_iter([
                DataElement::new(
                    Tag(0x0028, 0x3002),
                    VR::US,
                    PrimitiveValue::U16(vec![0, 0, 16].into()),
                ),
                DataElement::new(
                    Tag(0x0028, 0x3006),
                    VR::OW,
                    PrimitiveValue::U16(
                        (0..=u16::MAX)
                            .map(|v| v.wrapping_add(offset))
                            .collect::<Vec<_>>()
                            .into(),
                    ),
                ),
            ])],
        )
    };
    let groups = (0..80)
        .map(|_| {
            Object::from_element_iter([sequence(
                VOI,
                vec![Object::from_element_iter([make_table(0)])],
            )])
        })
        .collect();
    object.put(sequence(PER_FRAME, groups));
    let data = read(&object, 80, &|| false).unwrap();
    assert!(data.metadata_bytes < 140_000);
    assert!(
        data.frames
            .iter()
            .all(|frame| Arc::ptr_eq(frame, &data.frames[0]))
    );
    let lut = &data.frames[0].choices().luts()[0];
    assert_eq!(lut.entries().len(), 65536);
    assert_eq!(lut.normalize(-1.), Some(0.));
    assert_eq!(lut.normalize(32768.), Some(32768. / 65535.));
    assert_eq!(lut.normalize(65536.), Some(1.));
    let groups = (0..80)
        .map(|index| {
            Object::from_element_iter([sequence(
                VOI,
                vec![Object::from_element_iter([make_table(index)])],
            )])
        })
        .collect();
    object.put(sequence(PER_FRAME, groups));
    assert!(matches!(
        read(&object, 80, &|| false),
        Err(LoadError::Limit)
    ));
}
