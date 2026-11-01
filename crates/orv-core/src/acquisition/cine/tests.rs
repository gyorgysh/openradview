// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use dicom_core::DataElement;
fn object() -> Object {
    Object::from_element_iter([
        DataElement::new(
            Tag(0x0028, 0x0009),
            VR::AT,
            PrimitiveValue::Tags(vec![VECTOR].into()),
        ),
        DataElement::new(VECTOR, VR::DS, "0\\40\\120\\60\\80"),
    ])
}
fn available(object: &Object, count: u32) -> Arc<CineTiming> {
    match read_cine(object, count, &|| false).unwrap() {
        CineMetadata::Available(timing) => timing,
        other => panic!("Available timing expected: {other:?}"),
    }
}
#[test]
fn malformed_vectors_pointer_and_trim_are_explicit_and_rates_never_substitute_for_intervals() {
    for value in [
        "1\\40\\120\\60\\80",
        "0\\40",
        "0\\40\\120\\60\\80\\90",
        "0\\40\\0\\60\\80",
        "0\\40\\-1\\60\\80",
        "0\\NaN\\120\\60\\80",
        "0\\1e309\\120\\60\\80",
    ] {
        let mut object = object();
        object.put_str(VECTOR, VR::DS, value);
        assert!(matches!(
            read_cine(&object, 5, &|| false).unwrap(),
            CineMetadata::Unavailable(CineError::Values)
        ));
    }
    let mut o = object();
    o.remove_element(Tag(0x0028, 0x0009));
    o.put_str(Tag(0x0008, 0x2144), VR::IS, "100");
    o.put_str(Tag(0x0018, 0x1242), VR::IS, "1");
    assert!(matches!(
        read_cine(&o, 5, &|| false).unwrap(),
        CineMetadata::Unavailable(CineError::MissingPointer)
    ));
    for pointer in [vec![TIME, VECTOR], vec![Tag(0x0020, 0x0013)]] {
        let mut o = object();
        o.put(DataElement::new(
            Tag(0x0028, 0x0009),
            VR::AT,
            PrimitiveValue::Tags(pointer.into()),
        ));
        assert!(matches!(
            read_cine(&o, 5, &|| false).unwrap(),
            CineMetadata::Unavailable(CineError::Pointer)
        ));
    }
    for (first, last) in [(0, 5), (6, 5), (3, 2), (1, 6)] {
        let mut o = object();
        o.put_str(Tag(0x0008, 0x2142), VR::IS, first.to_string());
        o.put_str(Tag(0x0008, 0x2143), VR::IS, last.to_string());
        assert!(matches!(
            read_cine(&o, 5, &|| false).unwrap(),
            CineMetadata::Unavailable(CineError::Trim)
        ));
    }
    let mut o = object();
    o.put_str(Tag(0x0008, 0x2144), VR::IS, "100");
    o.put_str(Tag(0x0018, 0x1242), VR::IS, "1");
    assert_eq!(
        available(&o, 5).interval_before(2),
        Some(Duration::from_millis(120))
    );
}
#[test]
fn long_explicit_un_vectors_parse_decimal_bytes_with_bounded_storage_and_cancellation() {
    let mut o = object();
    let mut text = "0".to_owned();
    for _ in 1..10000 {
        text.push_str("\\123.456");
    }
    assert!(text.len() > 65534);
    if !text.len().is_multiple_of(2) {
        text.push('\0');
    }
    o.put(DataElement::new(
        VECTOR,
        VR::UN,
        PrimitiveValue::U8(text.into_bytes().into()),
    ));
    let timing = available(&o, 10000);
    assert_eq!(timing.frame_count(), 10000);
    assert_eq!(
        timing.interval_before(9999),
        Some(Duration::from_micros(123456))
    );
    assert!((timing.seconds_from_first(9999).unwrap() - 1234.436544).abs() < 1e-10);
    assert!(matches!(
        read_cine(&o, 65537, &|| false).unwrap(),
        CineMetadata::Unavailable(CineError::Limit)
    ));
    let calls = std::cell::Cell::new(0);
    assert!(matches!(
        read_cine(&o, 10000, &|| {
            calls.set(calls.get() + 1);
            calls.get() > 2
        }),
        Err(LoadError::Cancelled)
    ));
}
#[test]
fn unsafe_duration_ranges_nonfinite_delays_and_wrong_value_representations_do_not_become_clocks() {
    for value in ["1e-99", "1e99"] {
        let mut o = object();
        o.put(DataElement::new(
            Tag(0x0028, 0x0009),
            VR::AT,
            PrimitiveValue::Tags(vec![TIME].into()),
        ));
        o.put_str(TIME, VR::DS, value);
        assert!(matches!(
            read_cine(&o, 5, &|| false).unwrap(),
            CineMetadata::Unavailable(CineError::Range)
        ));
    }
    let mut o = object();
    o.put_str(Tag(0x0018, 0x1066), VR::DS, "inf");
    assert!(matches!(
        read_cine(&o, 5, &|| false).unwrap(),
        CineMetadata::Unavailable(CineError::Values)
    ));
    let mut o = object();
    o.put(DataElement::new(
        VECTOR,
        VR::FD,
        PrimitiveValue::F64(vec![0., 40., 120., 60., 80.].into()),
    ));
    assert!(matches!(
        read_cine(&o, 5, &|| false).unwrap(),
        CineMetadata::Unavailable(CineError::Values)
    ));
    let o = object();
    let timing = available(&o, 5);
    assert_eq!(timing.interval_before(0), None);
    assert_eq!(timing.interval_before(5), None);
    assert_eq!(timing.seconds_from_first(5), None);
}
