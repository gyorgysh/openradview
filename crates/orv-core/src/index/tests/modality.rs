// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use crate::scalar::{ModalityLut, ScalarData, ScalarInfo, Storage, Unit, ValueType, VoiChoices};
use dicom_core::{DataElement, PrimitiveValue, Tag, VR};
use dicom_object::DefaultDicomObject;
use std::sync::Arc;
#[path = "../../../tests/support/modality.rs"]
mod phantom;
const SYNTAXES: [&str; 3] = [
    "1.2.840.10008.1.2.1",
    "1.2.840.10008.1.2.2",
    "1.2.840.10008.1.2",
];
fn bytes(object: &DefaultDicomObject) -> Vec<u8> {
    let mut data = Vec::new();
    object.write_all(&mut data).unwrap();
    data
}
fn table<R>(
    object: &mut DefaultDicomObject,
    mut edit: impl FnMut(&mut dicom_object::InMemDicomObject) -> R,
) -> R {
    let mut result = None;
    assert!(object.update_value(Tag(0x0028, 0x3000), |value| {
        result = Some(edit(&mut value.items_mut().unwrap()[0]));
    }));
    result.unwrap()
}
fn eager(object: &DefaultDicomObject) -> Arc<crate::study::Frame> {
    crate::load::decode(&bytes(object))
        .unwrap()
        .frames
        .remove(0)
}
#[test]
fn modality_tables_map_signed_and_unsigned_stored_codes_in_both_loading_paths_and_byte_orders() {
    for syntax in SYNTAXES {
        for signed in [false, true] {
            for stored in [8, 12, 16, 32] {
                for bits in [8, 16] {
                    for packed in [false, true]
                        .into_iter()
                        .filter(|packed| !*packed || bits == 8)
                    {
                        let object = phantom::fixture(0., 1, syntax, signed, stored, bits, packed);
                        let expected = if bits == 8 {
                            [1., 11., 101., 255.]
                        } else {
                            [0., 100., 900., 65535.]
                        };
                        let first = eager(&object);
                        assert_eq!(
                            &*first.pixels, &expected,
                            "{syntax} signed={signed} stored={stored} bits={bits} packed={packed}"
                        );
                        let temp = Temp::new();
                        let path = temp.0.join("modality.dcm");
                        fs::write(&path, bytes(&object)).unwrap();
                        let study = index_paths(&[path], || false, |_| {}).unwrap();
                        let frame = study.series[0].read_frame(0, || false).unwrap();
                        assert_eq!(&*frame.pixels, &expected);
                        let scalar = frame.scalar.as_ref().unwrap();
                        assert_eq!(scalar.info().unit, Unit::Hounsfield);
                        assert_eq!(scalar.info().rescale, None);
                        assert_eq!(scalar.info().stored_bits, stored);
                        assert_eq!(scalar.digest(), first.scalar.as_ref().unwrap().digest());
                        let info = scalar.info().modality_lut.unwrap();
                        assert_eq!(
                            (info.first, info.entries, info.bits),
                            (if signed { -2 } else { 0 }, 4, bits)
                        );
                        assert!(frame.valid_dimensions());
                    }
                }
            }
        }
    }
}
#[test]
fn stored_padding_and_unused_high_bits_are_resolved_before_a_nonlinear_lookup() {
    let mut object = phantom::fixture(0., 1, SYNTAXES[0], true, 12, 16, false);
    object.put(DataElement::new(
        Tag(0x7fe0, 0x0010),
        VR::OW,
        PrimitiveValue::U16(vec![0xaffd, 0xaffe, 0xb000, 0xc002].into()),
    ));
    object.put(DataElement::new(
        Tag(0x0028, 0x0120),
        VR::SS,
        PrimitiveValue::from(-3i16),
    ));
    let frame = eager(&object);
    assert_eq!(&*frame.pixels, &[0., 0., 900., 65535.]);
    assert_eq!(frame.validity.as_deref(), Some(&[0, 1, 1, 1][..]));
    assert!(!frame.has_sample(0) && frame.has_sample(1));
    assert_eq!(crate::inspection::sample(&frame, [0., 0.]), None);
    assert_eq!(
        crate::inspection::sample(&frame, [1., 0.]),
        Some(crate::inspection::Sample::Scalar(0.))
    );
}
#[test]
fn spatial_interpolation_uses_mapped_samples_and_refuses_different_table_contexts() {
    let mut frames = Vec::new();
    for z in 0..3 {
        frames.push(eager(&phantom::fixture(
            z as f64 * 2.,
            z + 1,
            SYNTAXES[0],
            true,
            12,
            16,
            false,
        )));
    }
    let volume = crate::volume::Volume::prepare(frames.clone(), || false).unwrap();
    assert_eq!(
        volume.sample(volume.voxel_to_world([0.5, 0., 1.])),
        Some(50.)
    );
    let frame = volume
        .reformat(
            crate::Plane::Axial,
            volume.voxel_to_world([0.5, 0.5, 1.]),
            8,
            || false,
        )
        .unwrap();
    assert!(frame.scalar.as_ref().unwrap().info().resampled);
    assert_eq!(
        frame.scalar.as_ref().unwrap().info().modality_lut,
        frames[0].scalar.as_ref().unwrap().info().modality_lut
    );
    let mut changed = phantom::fixture(2., 2, SYNTAXES[0], true, 12, 16, false);
    table(&mut changed, |item| {
        item.put(DataElement::new(
            Tag(0x0028, 0x3006),
            VR::OW,
            PrimitiveValue::U16(vec![0, 101, 900, 65535].into()),
        ))
    });
    let different = eager(&changed);
    assert_ne!(
        different.scalar.as_ref().unwrap().digest(),
        frames[1].scalar.as_ref().unwrap().digest()
    );
    frames[1] = different;
    assert!(crate::volume::Volume::prepare(frames, || false).is_err());
}
#[test]
fn malformed_or_conflicting_tables_are_refused_instead_of_falling_back_to_rescale() {
    for variant in 0..10 {
        let mut object = phantom::fixture(0., 1, SYNTAXES[0], true, 12, 16, false);
        match variant {
            0 => {
                object.put_str(Tag(0x0028, 0x1053), VR::DS, "1");
            }
            1 => {
                object.put_str(Tag(0x0028, 0x1052), VR::DS, "");
            }
            2 => {
                table(&mut object, |item| {
                    item.put(DataElement::new(
                        Tag(0x0028, 0x3002),
                        VR::SS,
                        PrimitiveValue::I16(vec![4, -2].into()),
                    ))
                });
            }
            3 => {
                table(&mut object, |item| {
                    item.put(DataElement::new(
                        Tag(0x0028, 0x3002),
                        VR::SS,
                        PrimitiveValue::I16(vec![5, -2, 16].into()),
                    ))
                });
            }
            4 => {
                table(&mut object, |item| {
                    item.put(DataElement::new(
                        Tag(0x0028, 0x3002),
                        VR::SS,
                        PrimitiveValue::I16(vec![4, -2, 12].into()),
                    ))
                });
            }
            5 => {
                table(&mut object, |item| item.remove_element(Tag(0x0028, 0x3004)));
            }
            6 => {
                table(&mut object, |item| {
                    item.put_str(Tag(0x0028, 0x3004), VR::LO, "")
                });
            }
            7 => {
                let first = table(&mut object, |item| item.clone());
                object.put(phantom::sequence(
                    Tag(0x0028, 0x3000),
                    vec![first.clone(), first],
                ));
            }
            8 => {
                object.put(phantom::sequence(Tag(0x0028, 0x3000), vec![]));
            }
            _ => {
                object.put_str(Tag(0x0028, 0x1054), VR::LO, "HU");
            }
        }
        assert!(
            crate::load::decode(&bytes(&object)).is_err(),
            "variant{variant}"
        );
        let temp = Temp::new();
        let path = temp.0.join("invalid.dcm");
        fs::write(&path, bytes(&object)).unwrap();
        assert!(
            index_paths(&[path], || false, |_| {}).is_err(),
            "variant{variant}"
        );
    }
}
#[test]
fn mapping_identity_retains_units_and_omits_explanations_from_portable_metadata() {
    let mut a = phantom::fixture(0., 1, SYNTAXES[0], true, 12, 16, false);
    table(&mut a, |item| {
        item.put_str(Tag(0x0028, 0x3004), VR::LO, "UNIT_A")
    });
    let first = eager(&a);
    let info = first.scalar.as_ref().unwrap().info();
    assert_eq!(info.unit, Unit::Unknown);
    let json = serde_json::to_string(&crate::scene::ImageMetadata::from_frame(&first)).unwrap();
    assert!(json.contains("modality_lut"));
    assert!(!json.contains("UNIT_A") && !json.contains("Generated private explanation"));
    table(&mut a, |item| {
        item.put_str(Tag(0x0028, 0x3004), VR::LO, "UNIT_B")
    });
    let second = eager(&a);
    assert!(
        !first
            .scalar
            .as_ref()
            .unwrap()
            .compatible(second.scalar.as_ref().unwrap())
    );
    assert_ne!(
        first.scalar.as_ref().unwrap().digest(),
        second.scalar.as_ref().unwrap().digest()
    );
    assert!(
        ScalarData::new(info, VoiChoices::new(vec![], vec![]).unwrap()).is_err(),
        "Declared table without actual mapping was accepted"
    );
}
#[test]
fn maximum_table_endpoint_mapping_and_metadata_bounds_remain_explicit() {
    let values: Arc<[u16]> = (0..65536u32).map(|v| v as u16).collect();
    let lut = ModalityLut::new(-32768, 16, values).unwrap();
    for (input, expected) in [
        (i64::MIN, 0.),
        (-32768, 0.),
        (-32767, 1.),
        (32767, 65535.),
        (i64::MAX, 65535.),
    ] {
        assert_eq!(lut.map(input), expected);
    }
    assert_eq!(lut.info().entries, 65536);
    assert!(
        format!("{lut:?}").len() < 400,
        "Debug output included the full table"
    );
    assert!(ModalityLut::new(0, 8, Arc::from([256])).is_err());
    assert!(ModalityLut::new(0, 16, Arc::from([])).is_err());
    assert!(ModalityLut::new(65536, 16, Arc::from([1])).is_err());
}
#[test]
fn legacy_scalar_context_hash_and_serialization_remain_byte_compatible_without_a_table() {
    let info = ScalarInfo {
        modality_lut: None,
        value_type: ValueType::F32,
        storage: Storage::SignedInteger,
        stored_bits: 16,
        unit: Unit::Unknown,
        rescale: Some(crate::scalar::Rescale {
            slope: 2.,
            intercept: -100.,
        }),
        resampled: false,
    };
    let legacy = r#"{"value_type":"f32","storage":"signed_integer","stored_bits":16,"unit":"unknown","rescale":{"slope":2.0,"intercept":-100.0},"resampled":false}"#;
    assert_eq!(serde_json::to_string(&info).unwrap(), legacy);
    assert_eq!(serde_json::from_str::<ScalarInfo>(legacy).unwrap(), info);
    let scalar = ScalarData::new(info, VoiChoices::new(vec![], vec![]).unwrap()).unwrap();
    // Independently concatenated legacy fields and SHA-256, not a new-code roundtrip.
    assert_eq!(
        crate::scene::hex(&scalar.digest()),
        "f90d108a23e33bbcf28c35cbb61efaa88efda93f3eb9d9c3f1a741be2e17de44"
    );
}
#[test]
fn zero_descriptor_count_means_65536_and_mapping_cancellation_publishes_no_partial_pixels() {
    use dicom_pixeldata::PixelDecoder;
    let mut object = phantom::fixture(0., 1, SYNTAXES[0], false, 16, 16, false);
    table(&mut object, |item| {
        item.put(DataElement::new(
            Tag(0x0028, 0x3002),
            VR::US,
            PrimitiveValue::U16(vec![0, 0, 16].into()),
        ));
        item.put(DataElement::new(
            Tag(0x0028, 0x3006),
            VR::OW,
            PrimitiveValue::U16((0..65536u32).map(|v| v as u16).collect()),
        ));
    });
    let scalar = crate::dicom_voi::read(&object, 1, false, &|| false).unwrap();
    assert_eq!(
        scalar.get(0).unwrap().info().modality_lut.unwrap().entries,
        65536
    );
    assert!(scalar.metadata_bytes >= 131072 && scalar.metadata_bytes < 140000);
    assert_eq!(&*eager(&object).pixels, &[0., 1., 2., 3.]);
    let decoded = object.decode_pixel_data().unwrap();
    let calls = std::cell::Cell::new(0);
    let cancelled = || {
        let old = calls.get();
        calls.set(old + 1);
        old >= 1
    };
    assert_eq!(
        crate::dicom_voi::pixels(&object, &decoded, 0, 0, scalar.get(0).unwrap(), &cancelled)
            .unwrap_err(),
        crate::load::LoadError::Cancelled
    );
}
#[test]
fn packed_ow_bytes_and_padding_follow_original_order_across_odd_multiframe_boundaries() {
    for syntax in SYNTAXES {
        let mut object = phantom::fixture(0., 1, syntax, false, 8, 8, false);
        object.put(DataElement::new(
            Tag(0x0028, 0x0010),
            VR::US,
            PrimitiveValue::from(1u16),
        ));
        object.put(DataElement::new(
            Tag(0x0028, 0x0011),
            VR::US,
            PrimitiveValue::from(3u16),
        ));
        object.put_str(Tag(0x0028, 0x0008), VR::IS, "2");
        object.put(DataElement::new(
            Tag(0x7fe0, 0x0010),
            VR::OW,
            PrimitiveValue::U16(vec![0x0100, 0x0302, 0x0002].into()),
        ));
        object.put(DataElement::new(
            Tag(0x0028, 0x0120),
            VR::US,
            PrimitiveValue::from(0u16),
        ));
        let eager = crate::load::decode(&bytes(&object)).unwrap();
        let temp = Temp::new();
        let path = temp.0.join("odd.dcm");
        fs::write(&path, bytes(&object)).unwrap();
        let indexed = index_paths(&[path], || false, |_| {}).unwrap();
        for (frame, expected, mask) in [
            (0, [1., 11., 101.], [0, 1, 1]),
            (1, [255., 101., 1.], [1, 1, 0]),
        ] {
            for image in [
                eager.frames[frame].clone(),
                indexed.series[0].read_frame(frame, || false).unwrap(),
            ] {
                assert_eq!(&*image.pixels, &expected, "{syntax} frame{frame}");
                assert_eq!(
                    image.validity.as_deref(),
                    Some(&mask[..]),
                    "{syntax} frame{frame}"
                );
            }
        }
    }
}
#[test]
fn source_modality_mapping_precedes_voi_and_compressed_decoding_keeps_stored_padding() {
    use dicom_core::DicomValue;
    use dicom_core::value::PixelFragmentSequence;
    for signed in [false, true] {
        let mut object = phantom::fixture(0., 1, "1.2.840.10008.1.2.5", signed, 12, 16, false);
        let raw = if signed {
            vec![4094u16, 4095, 0, 1]
        } else {
            vec![0, 1, 2, 3]
        };
        let mut encoded = vec![0u8; 64];
        for (at, value) in [(0, 2u32), (4, 64), (8, 69)] {
            encoded[at..at + 4].copy_from_slice(&value.to_le_bytes());
        }
        for shift in [8, 0] {
            encoded.push(3);
            encoded.extend(raw.iter().map(|v| (v >> shift) as u8));
        }
        object.put(DataElement::new(
            Tag(0x7fe0, 0x0010),
            VR::OB,
            DicomValue::PixelSequence(PixelFragmentSequence::new(vec![], vec![encoded])),
        ));
        object.put(DataElement::new(
            Tag(0x0028, 0x0120),
            if signed { VR::SS } else { VR::US },
            if signed {
                PrimitiveValue::from(-1i16)
            } else {
                PrimitiveValue::from(1u16)
            },
        ));
        let first = eager(&object);
        let temp = Temp::new();
        let path = temp.0.join("rle.dcm");
        fs::write(&path, bytes(&object)).unwrap();
        let study = index_paths(&[path], || false, |_| {}).unwrap();
        for frame in [first, study.series[0].read_frame(0, || false).unwrap()] {
            assert_eq!(&*frame.pixels, &[0., 100., 900., 65535.]);
            assert_eq!(frame.validity.as_deref(), Some(&[1, 0, 1, 1][..]));
        }
    }
    let mut object = phantom::fixture(0., 1, SYNTAXES[2], true, 32, 8, false);
    object.remove_element(Tag(0x0028, 0x1050));
    object.remove_element(Tag(0x0028, 0x1051));
    table(&mut object, |item| {
        item.put(DataElement::new(
            Tag(0x0028, 0x3006),
            VR::OW,
            PrimitiveValue::U16(vec![0, 1, 3, 2].into()),
        ));
    });
    object.put(phantom::sequence(
        Tag(0x0028, 0x3010),
        vec![dicom_object::InMemDicomObject::from_element_iter([
            DataElement::new(
                Tag(0x0028, 0x3002),
                VR::US,
                PrimitiveValue::U16(vec![4, 0, 8].into()),
            ),
            DataElement::new(
                Tag(0x0028, 0x3006),
                VR::OW,
                PrimitiveValue::U16(vec![0, 64, 192, 255].into()),
            ),
        ])],
    ));
    let frame = eager(&object);
    assert_eq!(&*frame.pixels, &[0., 1., 3., 2.]);
    assert_eq!(frame.default_window, crate::WindowLevel::lookup(0));
    for (&value, expected) in frame.pixels.iter().zip([0., 64. / 255., 1., 192. / 255.]) {
        assert_eq!(frame.normalize(frame.default_window, value), Some(expected));
    }
}
#[test]
fn bit_packed_codes_and_unsupported_mapping_scopes_keep_explicit_interpretation() {
    let mut object = phantom::fixture(0., 1, SYNTAXES[0], false, 8, 8, false);
    for (tag, value) in [
        (Tag(0x0028, 0x0100), 1u16),
        (Tag(0x0028, 0x0101), 1),
        (Tag(0x0028, 0x0102), 0),
    ] {
        object.put(DataElement::new(tag, VR::US, PrimitiveValue::from(value)));
    }
    object.put(DataElement::new(
        Tag(0x7fe0, 0x0010),
        VR::OB,
        PrimitiveValue::U8(vec![0b1010, 0].into()),
    ));
    let frame = eager(&object);
    assert_eq!(&*frame.pixels, &[1., 11., 1., 11.]);
    for variant in 0..3 {
        let mut object = phantom::fixture(0., 1, SYNTAXES[0], true, 12, 16, false);
        match variant {
            0 => {
                object.put_str(Tag(0x0028, 0x0004), VR::CS, "RGB");
            }
            1 => {
                object.put(phantom::sequence(Tag(0x0040, 0x9096), vec![]));
            }
            _ => {
                let lookup = table(&mut object, |item| item.clone());
                object.remove_element(Tag(0x0028, 0x3000));
                object.put(phantom::sequence(
                    Tag(0x5200, 0x9229),
                    vec![dicom_object::InMemDicomObject::from_element_iter([
                        phantom::sequence(Tag(0x0028, 0x3000), vec![lookup]),
                    ])],
                ));
            }
        }
        assert!(
            crate::load::decode(&bytes(&object)).is_err(),
            "scope{variant}"
        );
    }
}
#[test]
fn literal_ow_streams_follow_low_word_first_and_bit_frames_need_no_internal_padding() {
    let object = phantom::fixture(0., 1, SYNTAXES[1], false, 32, 16, false);
    let data = bytes(&object);
    let header = [0x7f, 0xe0, 0, 0x10, b'O', b'W', 0, 0, 0, 0, 0, 16];
    let offset = data
        .windows(header.len())
        .position(|v| v == header)
        .unwrap()
        + header.len();
    assert_eq!(
        &data[offset..offset + 16],
        &[0, 0, 0, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0, 3, 0, 0]
    );
    let mut little = phantom::fixture(0., 1, SYNTAXES[0], false, 8, 8, false);
    little.put(DataElement::new(
        Tag(0x0028, 0x0010),
        VR::US,
        PrimitiveValue::from(1u16),
    ));
    little.put(DataElement::new(
        Tag(0x0028, 0x0011),
        VR::US,
        PrimitiveValue::from(3u16),
    ));
    little.put_str(Tag(0x0028, 0x0008), VR::IS, "2");
    little.put(DataElement::new(
        Tag(0x0028, 0x0100),
        VR::US,
        PrimitiveValue::from(1u16),
    ));
    little.put(DataElement::new(
        Tag(0x0028, 0x0101),
        VR::US,
        PrimitiveValue::from(1u16),
    ));
    little.put(DataElement::new(
        Tag(0x0028, 0x0102),
        VR::US,
        PrimitiveValue::from(0u16),
    ));
    for syntax in SYNTAXES {
        for ow in [false, true] {
            let mut object = little.clone();
            object.update_meta(|meta| meta.transfer_syntax = syntax.into());
            object.put(DataElement::new(
                Tag(0x7fe0, 0x0010),
                if ow { VR::OW } else { VR::OB },
                if ow {
                    PrimitiveValue::U16(vec![0b011010].into())
                } else {
                    PrimitiveValue::U8(vec![0b011010, 0].into())
                },
            ));
            let eager = crate::load::decode(&bytes(&object)).unwrap();
            let temp = Temp::new();
            let path = temp.0.join("bits.dcm");
            fs::write(&path, bytes(&object)).unwrap();
            let study = index_paths(&[path], || false, |_| {}).unwrap();
            for (index, expected) in [(0, [1., 11., 1.]), (1, [11., 11., 1.])] {
                assert_eq!(
                    &*eager.frames[index].pixels, &expected,
                    "eager {syntax} ow={ow}"
                );
                let frame = study.series[0].read_frame(index, || false).unwrap();
                assert_eq!(&*frame.pixels, &expected, "indexed {syntax} ow={ow}");
            }
        }
    }
    // The same word-normalized native index also corrects ordinary eight-bit OW.
    let mut object = phantom::fixture(0., 1, SYNTAXES[1], false, 8, 8, false);
    object.remove_element(Tag(0x0028, 0x3000));
    object.put_str(Tag(0x0028, 0x1052), VR::DS, "0");
    object.put_str(Tag(0x0028, 0x1053), VR::DS, "1");
    object.put(DataElement::new(
        Tag(0x7fe0, 0x0010),
        VR::OW,
        PrimitiveValue::U16(vec![0x0100, 0x0302].into()),
    ));
    let data = bytes(&object);
    let temp = Temp::new();
    let path = temp.0.join("plain.dcm");
    fs::write(&path, &data).unwrap();
    let study = index_paths(&[path], || false, |_| {}).unwrap();
    assert_eq!(
        &*crate::load::decode(&data).unwrap().frames[0].pixels,
        &[0., 1., 2., 3.]
    );
    assert_eq!(
        &*study.series[0].read_frame(0, || false).unwrap().pixels,
        &[0., 1., 2., 3.]
    );
}
#[test]
fn voi_input_after_a_modality_table_is_unsigned_even_for_signed_stored_pixels() {
    for syntax in SYNTAXES {
        let mut object = phantom::fixture(0., 1, syntax, true, 12, 16, false);
        object.remove_element(Tag(0x0028, 0x1050));
        object.remove_element(Tag(0x0028, 0x1051));
        object.put(phantom::sequence(
            Tag(0x0028, 0x3010),
            vec![dicom_object::InMemDicomObject::from_element_iter([
                DataElement::new(
                    Tag(0x0028, 0x3002),
                    VR::US,
                    PrimitiveValue::U16(vec![4, 32768, 8].into()),
                ),
                DataElement::new(
                    Tag(0x0028, 0x3006),
                    VR::OW,
                    PrimitiveValue::U16(vec![0, 64, 192, 255].into()),
                ),
            ])],
        ));
        let frame = eager(&object);
        assert_eq!(
            frame.scalar.as_ref().unwrap().choices().luts()[0].first(),
            32768
        );
        for (&value, expected) in frame.pixels.iter().zip([0., 0., 0., 1.]) {
            assert_eq!(
                frame.normalize(frame.default_window, value),
                Some(expected),
                "{syntax}"
            );
        }
    }
}
