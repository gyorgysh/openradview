// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use crate::scalar::{Storage, Unit};
use dicom_core::{DataElement, PrimitiveValue, Tag, VR};
use dicom_object::DefaultDicomObject;
use std::sync::Arc;
#[path = "../../../tests/support/float.rs"]
mod phantom;
const SYNTAXES: [&str; 4] = [
    "1.2.840.10008.1.2.1",
    "1.2.840.10008.1.2.2",
    "1.2.840.10008.1.2",
    "1.2.840.10008.1.2.1.99",
];
fn bytes(object: &DefaultDicomObject) -> Vec<u8> {
    let mut data = Vec::new();
    object.write_all(&mut data).unwrap();
    data
}
fn indexed(object: &DefaultDicomObject) -> Result<crate::study::Study, LoadError> {
    let temp = Temp::new();
    let path = temp.0.join("float.dcm");
    fs::write(&path, bytes(object)).unwrap();
    let study = index_paths(&[path], || false, |_| {})?;
    // Keep the file alive while frames are read lazily.
    let frames: Vec<_> = (0..study.series[0].frame_count())
        .map(|frame| study.series[0].read_frame(frame, || false))
        .collect();
    drop(temp);
    frames.into_iter().collect::<Result<Vec<_>, _>>()?;
    Ok(study)
}
fn frames(object: &DefaultDicomObject) -> Vec<Arc<crate::study::Frame>> {
    let temp = Temp::new();
    let path = temp.0.join("float.dcm");
    fs::write(&path, bytes(object)).unwrap();
    let study = index_paths(&[path], || false, |_| {}).unwrap();
    (0..study.series[0].frame_count())
        .map(|frame| study.series[0].read_frame(frame, || false).unwrap())
        .collect()
}
fn put(object: &mut DefaultDicomObject, values: Vec<f32>) {
    object.put(DataElement::new(
        Tag(0x7fe0, 0x0008),
        VR::OF,
        PrimitiveValue::F32(values.into()),
    ));
}

#[test]
fn float_pixels_keep_stored_values_across_byte_orders_frames_and_both_loading_paths() {
    let expected = phantom::values(3);
    for syntax in SYNTAXES {
        let object = phantom::fixture(0., 1, syntax, 3);
        let encoded = bytes(&object);
        // Independent of the reader: the encoded payload holds each declared
        // float in the transfer syntax byte order (deflate hides it).
        if syntax != SYNTAXES[3] {
            let word = |value: f32| {
                if syntax == SYNTAXES[1] {
                    value.to_be_bytes()
                } else {
                    value.to_le_bytes()
                }
            };
            let payload: Vec<u8> = expected.iter().flat_map(|v| word(*v)).collect();
            assert!(encoded.ends_with(&payload), "{syntax}");
        }
        let eager = crate::load::decode(&encoded).unwrap();
        let lazy = frames(&object);
        assert_eq!((eager.frames.len(), lazy.len()), (3, 3), "{syntax}");
        for (index, (a, b)) in eager.frames.iter().zip(&lazy).enumerate() {
            let want = &expected[index * 3..index * 3 + 3];
            assert_eq!(&*a.pixels, want, "{syntax} eager {index}");
            assert_eq!(&*b.pixels, want, "{syntax} indexed {index}");
            assert!(a.validity.is_none() && b.validity.is_none());
            let info = b.scalar.as_ref().unwrap().info();
            assert_eq!(
                (info.storage, info.stored_bits, info.unit, info.rescale),
                (Storage::Float, 32, Unit::Unknown, None)
            );
            assert!(info.modality_lut.is_none());
            assert_eq!(
                a.scalar.as_ref().unwrap().digest(),
                b.scalar.as_ref().unwrap().digest()
            );
            assert!(b.default_window.is_valid());
            assert!(b.valid_dimensions());
        }
    }
}

#[test]
fn float_padding_ranges_and_nonfinite_samples_are_missing_in_both_paths() {
    let mut object = phantom::fixture(0., 1, SYNTAXES[0], 2);
    put(
        &mut object,
        vec![-7.25, f32::NAN, 4., f32::NEG_INFINITY, -7., 1e-40],
    );
    object.put(DataElement::new(
        Tag(0x0028, 0x0122),
        VR::FL,
        PrimitiveValue::from(-7.25f32),
    ));
    object.put(DataElement::new(
        Tag(0x0028, 0x0124),
        VR::FL,
        PrimitiveValue::from(-7f32),
    ));
    let eager = crate::load::decode(&bytes(&object)).unwrap();
    let lazy = frames(&object);
    for frames in [eager.frames, lazy] {
        assert_eq!(frames[0].validity.as_deref(), Some(&[0, 0, 1][..]));
        assert_eq!(frames[1].validity.as_deref(), Some(&[0, 0, 1][..]));
        assert_eq!(frames[0].pixels[2], 4.);
        // A subnormal finite value remains a real sample.
        assert_eq!(frames[1].pixels[2], 1e-40);
        assert!(frames[0].pixels.iter().all(|value| value.is_finite()));
        assert_eq!(crate::inspection::sample(&frames[0], [0., 0.]), None);
        assert_eq!(
            crate::inspection::sample(&frames[0], [2., 0.]),
            Some(crate::inspection::Sample::Scalar(4.))
        );
        // Automatic display range is derived from the valid sample alone.
        let window = frames[0].default_window;
        assert!(window.center - window.width / 2. <= 4. && window.center + window.width / 2. >= 4.);
        assert!(window.center - window.width / 2. > -7.);
    }
}

#[test]
fn declared_float_windows_are_used_and_regular_float_stacks_reformat_valid_values() {
    let mut slices = Vec::new();
    for z in 0..3u32 {
        let mut object = phantom::fixture(f64::from(z) * 2., z + 1, SYNTAXES[0], 1);
        object.put(DataElement::new(
            Tag(0x0028, 0x0010),
            VR::US,
            PrimitiveValue::from(2u16),
        ));
        let value = 10. * z as f32;
        put(&mut object, vec![0.5, value, -3., 0.5, value, -3.]);
        object.put_str(Tag(0x0028, 0x1050), VR::DS, "1.5");
        object.put_str(Tag(0x0028, 0x1051), VR::DS, "0.25");
        // LINEAR requires a width of at least 1. LINEAR_EXACT permits fractions.
        object.put_str(Tag(0x0028, 0x1056), VR::CS, "LINEAR_EXACT");
        let frame = crate::load::decode(&bytes(&object))
            .unwrap()
            .frames
            .remove(0);
        assert_eq!(
            (frame.default_window.center, frame.default_window.width),
            (1.5, 0.25)
        );
        slices.push(frame);
    }
    let volume = crate::volume::Volume::prepare(slices, || false).unwrap();
    // Halfway between z=0 (0) and z=2 (10) in source space.
    assert_eq!(
        volume.sample(volume.voxel_to_world([1., 0., 0.5])),
        Some(5.)
    );
    let frame = volume
        .reformat(
            crate::Plane::Axial,
            volume.voxel_to_world([1., 0., 1.]),
            8,
            || false,
        )
        .unwrap();
    let info = frame.scalar.as_ref().unwrap().info();
    assert!(info.resampled && info.storage == Storage::Float);
}

#[test]
fn unsupported_float_descriptions_are_refused_in_both_paths() {
    for variant in 0..15 {
        let mut object = phantom::fixture(0., 1, SYNTAXES[0], 1);
        let us = |object: &mut DefaultDicomObject, tag, value: u16| {
            object.put(DataElement::new(tag, VR::US, PrimitiveValue::from(value)))
        };
        let _ = match variant {
            0 => us(&mut object, Tag(0x0028, 0x0101), 32),
            1 => us(&mut object, Tag(0x0028, 0x0103), 1),
            2 => us(&mut object, Tag(0x0028, 0x0100), 64),
            3 => object.put_str(Tag(0x0028, 0x0004), VR::CS, "MONOCHROME1"),
            4 => {
                object.put_str(Tag(0x0028, 0x1052), VR::DS, "0");
                object.put_str(Tag(0x0028, 0x1053), VR::DS, "2");
                None
            }
            5 => {
                object.put(DataElement::new(
                    Tag(0x7fe0, 0x0009),
                    VR::OD,
                    PrimitiveValue::F64(vec![1., 2., 3.].into()),
                ));
                object.remove_element(Tag(0x7fe0, 0x0008));
                None
            }
            6 => {
                put(&mut object, vec![1., 2.]);
                None
            }
            7 => object.put(DataElement::new(
                Tag(0x0028, 0x0124),
                VR::FL,
                PrimitiveValue::from(1f32),
            )),
            8 => {
                object.put(DataElement::new(
                    Tag(0x0028, 0x0122),
                    VR::FL,
                    PrimitiveValue::from(2f32),
                ));
                object.put(DataElement::new(
                    Tag(0x0028, 0x0124),
                    VR::FL,
                    PrimitiveValue::from(1f32),
                ));
                None
            }
            9 => object.put(DataElement::new(
                Tag(0x0028, 0x0122),
                VR::FL,
                PrimitiveValue::from(f32::NAN),
            )),
            10 => object.put(DataElement::new(
                Tag(0x0028, 0x0120),
                VR::US,
                PrimitiveValue::from(0u16),
            )),
            11 => object.put(DataElement::new(
                Tag(0x7fe0, 0x0010),
                VR::OW,
                PrimitiveValue::U16(vec![0; 6].into()),
            )),
            12 => object.put(DataElement::new(
                Tag(0x0040, 0x9096),
                VR::SQ,
                dicom_core::DicomValue::new_sequence(
                    vec![dicom_object::InMemDicomObject::new_empty()],
                    dicom_core::Length::UNDEFINED,
                ),
            )),
            13 => {
                // An explicit VR other than OF is not Float Pixel Data.
                object.remove_element(Tag(0x7fe0, 0x0008));
                object.put(DataElement::new(
                    Tag(0x7fe0, 0x0008),
                    VR::OB,
                    PrimitiveValue::U8(vec![0; 12].into()),
                ));
                None
            }
            _ => us(&mut object, Tag(0x0028, 0x0002), 3),
        };
        assert!(
            crate::load::decode(&bytes(&object)).is_err(),
            "eager variant{variant}"
        );
        assert!(indexed(&object).is_err(), "indexed variant{variant}");
    }
}

#[test]
fn float_and_integer_scalar_contexts_are_never_combined() {
    let float = crate::load::decode(&bytes(&phantom::fixture(0., 1, SYNTAXES[0], 1)))
        .unwrap()
        .frames
        .remove(0);
    let info = float.scalar.as_ref().unwrap().info();
    let integer = crate::scalar::ScalarInfo {
        storage: Storage::SignedInteger,
        ..info
    };
    assert!(info.validate() && integer.validate());
    let other = crate::scalar::ScalarData::new(
        integer,
        crate::scalar::VoiChoices::new(vec![], vec![]).unwrap(),
    )
    .unwrap();
    assert!(!float.scalar.as_ref().unwrap().compatible(&other));
    let json = serde_json::to_string(&crate::scene::ImageMetadata::from_frame(&float)).unwrap();
    assert!(json.contains("\"float\""));
}
