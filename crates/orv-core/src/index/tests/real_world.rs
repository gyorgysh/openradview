// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use crate::scalar::{RealWorld, Unit};
use dicom_core::{DataElement, PrimitiveValue, Tag, VR};
use dicom_object::{DefaultDicomObject, InMemDicomObject as Object};
use std::sync::Arc;
#[path = "../../../tests/support/real_world.rs"]
pub(super) mod phantom;
const SYNTAXES: [&str; 3] = [
    "1.2.840.10008.1.2.1",
    "1.2.840.10008.1.2.2",
    "1.2.840.10008.1.2",
];
type Edit = Box<dyn Fn(&mut DefaultDicomObject)>;
fn bytes(object: &DefaultDicomObject) -> Vec<u8> {
    let mut data = Vec::new();
    object.write_all(&mut data).unwrap();
    data
}
fn both(object: &DefaultDicomObject) -> Result<[Arc<crate::study::Frame>; 2], LoadError> {
    let eager = crate::load::decode(&bytes(object))?.frames.remove(0);
    let temp = Temp::new();
    let path = temp.0.join("mapped.dcm");
    fs::write(&path, bytes(object)).unwrap();
    let study = index_paths(&[path], || false, |_| {})?;
    let lazy = study.series[0].read_frame(0, || false)?;
    Ok([eager, lazy])
}

#[test]
fn one_linear_mapping_replaces_stored_integer_values_in_both_paths() {
    for syntax in SYNTAXES {
        let object = phantom::integer(syntax, phantom::item(0.5, -10., Some((0, 255)), None));
        for frame in both(&object).unwrap() {
            // Independently: stored [0, 1, 2, 255] * 0.5 - 10.
            assert_eq!(&*frame.pixels, &[-10., -9.5, -9., 117.5], "{syntax}");
            assert!(frame.validity.is_none());
            let info = frame.scalar.as_ref().unwrap().info();
            assert_eq!(info.unit, Unit::Counts);
            assert_eq!((info.rescale, info.modality_lut), (None, None));
            assert_eq!(
                info.real_world,
                Some(RealWorld {
                    slope: 0.5,
                    intercept: -10.,
                    first: 0.,
                    last: 255.,
                })
            );
        }
    }
}

#[test]
fn float_mappings_apply_to_stored_values_and_leave_outside_values_missing() {
    for syntax in SYNTAXES {
        let object = phantom::float(syntax, phantom::item(2., 1., None, Some((-10., 10.))));
        for frame in both(&object).unwrap() {
            // Stored [-20, -1.5, 0.25, 30]: only -1.5 and 0.25 lie in [-10, 10].
            assert_eq!(frame.validity.as_deref(), Some(&[0, 1, 1, 0][..]));
            assert_eq!(&frame.pixels[1..3], &[-2., 1.5], "{syntax}");
            assert_eq!(crate::inspection::sample(&frame, [0., 0.]), None);
        }
        // The integer stored-value attributes also apply to float data.
        let object = phantom::float(syntax, phantom::item(1., 0., Some((-2, 1)), None));
        for frame in both(&object).unwrap() {
            assert_eq!(frame.validity.as_deref(), Some(&[0, 1, 1, 0][..]));
            assert_eq!(&frame.pixels[1..3], &[-1.5, 0.25]);
        }
    }
}

#[test]
fn shared_functional_group_mappings_are_read_and_per_frame_ones_refused() {
    let mut object = phantom::integer(SYNTAXES[0], phantom::item(2., 0., Some((0, 255)), None));
    let mapping = object.element(Tag(0x0040, 0x9096)).unwrap().clone();
    object.remove_element(Tag(0x0040, 0x9096));
    let mut group = Object::new_empty();
    group.put(mapping.clone());
    object.put(phantom::sequence(Tag(0x5200, 0x9229), vec![group.clone()]));
    let [eager, lazy] = both(&object).unwrap();
    assert_eq!(&*eager.pixels, &[0., 2., 4., 510.]);
    assert_eq!(eager.pixels, lazy.pixels);
    let mut per_frame = object.clone();
    per_frame.remove_element(Tag(0x5200, 0x9229));
    per_frame.put(phantom::sequence(Tag(0x5200, 0x9230), vec![group.clone()]));
    assert!(both(&per_frame).is_err());
    let mut twice = object;
    twice.put(mapping);
    assert!(both(&twice).is_err());
}

#[test]
fn alternative_lookup_partial_and_conflicting_mappings_are_refused() {
    for variant in 0..12 {
        let mut item = phantom::item(0.5, -10., Some((0, 255)), None);
        let mut extra: Option<Edit> = None;
        match variant {
            0 => {
                extra = Some(Box::new(|object| {
                    let item = phantom::item(1., 0., Some((0, 255)), None);
                    let first = phantom::item(0.5, -10., Some((0, 255)), None);
                    object.put(phantom::sequence(Tag(0x0040, 0x9096), vec![first, item]));
                }))
            }
            1 => {
                item.put(DataElement::new(
                    Tag(0x0040, 0x9212),
                    VR::FD,
                    PrimitiveValue::F64(vec![0.; 256].into()),
                ));
            }
            2 => {
                item.remove_element(Tag(0x0040, 0x9225));
            }
            3 => item = phantom::item(0.5, -10., Some((0, 254)), None),
            4 => item = phantom::item(0.5, -10., None, Some((0., 255.))),
            5 => item = phantom::item(0., -10., Some((0, 255)), None),
            6 => {
                item.remove_element(Tag(0x0040, 0x08ea));
            }
            7 => {
                let mut code = Object::new_empty();
                code.put_str(Tag(0x0008, 0x0100), VR::SH, "{counts}");
                let codes = vec![code.clone(), code];
                item.put(phantom::sequence(Tag(0x0040, 0x08ea), codes));
            }
            8 => {
                extra = Some(Box::new(|object| {
                    object.put_str(Tag(0x0028, 0x1052), VR::DS, "0");
                    object.put_str(Tag(0x0028, 0x1053), VR::DS, "1");
                }))
            }
            9 => {
                extra = Some(Box::new(|object| {
                    object.put_str(Tag(0x0028, 0x1054), VR::LO, "HU");
                }))
            }
            10 => {
                extra = Some(Box::new(|object| {
                    let mut table = Object::new_empty();
                    table.put(DataElement::new(
                        Tag(0x0028, 0x3002),
                        VR::US,
                        PrimitiveValue::U16(vec![2, 0, 8].into()),
                    ));
                    table.put(DataElement::new(
                        Tag(0x0028, 0x3006),
                        VR::OW,
                        PrimitiveValue::U16(vec![0x0100].into()),
                    ));
                    object.put(phantom::sequence(Tag(0x0028, 0x3010), vec![table]));
                }))
            }
            _ => {
                item.put(DataElement::new(
                    Tag(0x0040, 0x9225),
                    VR::FD,
                    PrimitiveValue::F64(vec![1., 2.].into()),
                ));
            }
        }
        let mut object = phantom::integer(SYNTAXES[0], item);
        if let Some(extra) = extra {
            extra(&mut object);
        }
        assert!(both(&object).is_err(), "variant{variant}");
    }
}

#[test]
fn unit_codes_and_mappings_are_bound_to_identity_without_portable_text() {
    let custom = |value: &str| {
        let mut item = phantom::item(0.5, -10., Some((0, 255)), None);
        item.put(phantom::units("99LOCAL", value));
        both(&phantom::integer(SYNTAXES[0], item)).unwrap()[0].clone()
    };
    let a = custom("UNIT_A");
    let b = custom("UNIT_B");
    let (a, b) = (a.scalar.as_ref().unwrap(), b.scalar.as_ref().unwrap());
    assert_eq!(a.info().unit, Unit::Unknown);
    assert!(!a.compatible(b));
    assert_ne!(a.digest(), b.digest());
    let plain = both(&phantom::integer(
        SYNTAXES[0],
        phantom::item(0.5, -10., Some((0, 255)), None),
    ))
    .unwrap()[0]
        .clone();
    let changed = both(&phantom::integer(
        SYNTAXES[0],
        phantom::item(0.25, -10., Some((0, 255)), None),
    ))
    .unwrap()[0]
        .clone();
    // Different mappings never share a volume or a hash.
    assert!(
        !plain
            .scalar
            .as_ref()
            .unwrap()
            .compatible(changed.scalar.as_ref().unwrap())
    );
    let resampled = plain.scalar.as_ref().unwrap().resampled();
    assert_eq!(
        resampled.info().real_world,
        plain.scalar.as_ref().unwrap().info().real_world
    );
    let json = serde_json::to_string(&crate::scene::ImageMetadata::from_frame(&plain)).unwrap();
    assert!(json.contains("real_world"));
    for private in [
        "UNIT_A",
        "{counts}",
        "GENERATED_LABEL",
        "Generated private explanation",
        "Generated unit meaning",
    ] {
        assert!(!json.contains(private), "{private}");
    }
}
