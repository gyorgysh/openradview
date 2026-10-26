// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
fn depth(object: &mut dicom_object::DefaultDicomObject, bits: u16, stored: u16) {
    for (tag, value) in [(0x0100, bits), (0x0101, stored), (0x0102, stored - 1)] {
        object.put(DataElement::new(
            Tag(0x0028, tag),
            VR::US,
            PrimitiveValue::from(value),
        ));
    }
}
fn read(object: &dicom_object::DefaultDicomObject) -> Arc<crate::study::Frame> {
    let temp = Temp::new();
    let path = temp.0.join("generated.dcm");
    object.write_to_file(&path).unwrap();
    index_paths(&[path], || false, |_| {}).unwrap().series[0]
        .read_frame(0, || false)
        .unwrap()
}
#[test]
fn rgb_16bit_native_endian_planar_rle_and_jpegls_preserve_code_precision() {
    for stored in [12, 16] {
        for planar in [false, true] {
            for codec in 0..4 {
                if planar && codec >= 2 {
                    continue;
                }
                let max = ((1u32 << stored) - 1) as u16;
                let values = [max, 0, 0, 0, max, 0, 0, 0, max, max / 2, max / 4, max / 8];
                let mut object = rgb(planar);
                depth(&mut object, 16, stored);
                let data: Vec<u16> = if planar {
                    (0..3)
                        .flat_map(|c| (0..4).map(move |i| values[i * 3 + c]))
                        .collect()
                } else {
                    values.to_vec()
                };
                object.put(DataElement::new(
                    Tag(0x7fe0, 0x0010),
                    VR::OW,
                    PrimitiveValue::U16(data.into()),
                ));
                match codec {
                    1 => object.meta_mut().transfer_syntax = "1.2.840.10008.1.2.2".into(),
                    2 => {
                        rle16(&mut object, &values);
                    }
                    3 => {
                        super::jpegls::compress(&mut object, &values, stored, 2);
                    }
                    _ => {}
                }
                object.meta_mut().update_information_group_length();
                let image = read(&object);
                assert!(image.valid_dimensions());
                assert_eq!(&*image.color.as_ref().unwrap().rgb, &values);
                assert_eq!(image.color.as_ref().unwrap().info.bits, stored);
                assert_eq!(
                    &image.rgba.as_ref().unwrap()[..12],
                    &[255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255]
                );
                assert_eq!(image.byte_len(), 40);
            }
        }
    }
}
#[test]
fn native_packed_ybr422_reads_two_samples_per_pixel_and_multiframe_offsets() {
    let mut object = rgb(false);
    object.put_str(Tag(0x0028, 0x0004), VR::CS, "YBR_FULL_422");
    object.put_str(Tag(0x0028, 0x0008), VR::IS, "2");
    object.put(DataElement::new(
        Tag(0x7fe0, 0x0010),
        VR::OB,
        PrimitiveValue::U8(
            vec![
                0, 255, 128, 128, 64, 128, 128, 128, 76, 76, 85, 255, 128, 128, 128, 128,
            ]
            .into(),
        ),
    ));
    let temp = Temp::new();
    let path = temp.0.join("422.dcm");
    object.write_to_file(&path).unwrap();
    let study = index_paths(&[path], || false, |_| {}).unwrap();
    assert_eq!(study.series[0].frame_count(), 2);
    let a = study.series[0].read_frame(0, || false).unwrap();
    let b = study.series[0].read_frame(1, || false).unwrap();
    assert_eq!(
        &*a.color.as_ref().unwrap().rgb,
        &[0, 0, 0, 255, 255, 255, 64, 64, 64, 128, 128, 128]
    );
    assert_eq!(
        &b.rgba.as_ref().unwrap()[..8],
        &[254, 0, 0, 255, 254, 0, 0, 255]
    );
    let bytes = fs::read(temp.0.join("422.dcm")).unwrap();
    let eager = crate::load::decode(&bytes).unwrap();
    assert_eq!(eager.frames[1].rgba, b.rgba);
}
fn palette(signed: bool, segmented: bool) -> dicom_object::DefaultDicomObject {
    let mut object = rgb(false);
    depth(&mut object, 16, 16);
    object.put_str(Tag(0x0028, 0x0004), VR::CS, "PALETTE COLOR");
    object.remove_element(Tag(0x0028, 0x0006));
    for (tag, value) in [(0x0002, 1u16), (0x0103, u16::from(signed))] {
        object.put(DataElement::new(
            Tag(0x0028, tag),
            VR::US,
            PrimitiveValue::from(value),
        ));
    }
    object.put(DataElement::new(
        Tag(0x7fe0, 0x0010),
        VR::OW,
        PrimitiveValue::U16(
            if signed {
                vec![65534, 65535, 0, 1]
            } else {
                vec![0, 1, 2, 65535]
            }
            .into(),
        ),
    ));
    for channel in 1..=3 {
        object.put(DataElement::new(
            Tag(0x0028, 0x1100 + channel),
            VR::US,
            PrimitiveValue::U16(vec![4, if signed { 65535 } else { 1 }, 16].into()),
        ));
        object.put(DataElement::new(
            Tag(0x0028, if segmented { 0x1220 } else { 0x1200 } + channel),
            VR::OW,
            PrimitiveValue::U16(
                if segmented {
                    vec![0, 1, 0, 1, 3, 65535]
                } else {
                    vec![0, 21845, 43690, 65535]
                }
                .into(),
            ),
        ));
    }
    object
}
#[test]
fn direct_and_segmented_palette_color_handle_signed_indices_clipping_and_endian() {
    for signed in [false, true] {
        for segmented in [false, true] {
            for big in [false, true] {
                let mut object = palette(signed, segmented);
                if big {
                    object.meta_mut().transfer_syntax = "1.2.840.10008.1.2.2".into();
                    object.meta_mut().update_information_group_length();
                }
                let image = read(&object);
                let expected = if signed {
                    [0, 0, 85, 170]
                } else {
                    [0, 0, 85, 255]
                };
                for (pixel, value) in image
                    .rgba
                    .as_ref()
                    .unwrap()
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .zip(expected)
                {
                    assert_eq!(*pixel, [value, value, value, 255]);
                }
                assert_eq!(
                    image.color.as_ref().unwrap().info.source_encoding,
                    crate::color::ColorEncoding::Palette
                );
                assert!(image.pixels.is_empty());
            }
        }
    }
}
#[test]
fn palette_eight_bit_entries_support_packed_and_word_padded_tables() {
    for padded in [false, true] {
        for big in [false, true] {
            let mut object = palette(false, false);
            for channel in 1..=3 {
                object.put(DataElement::new(
                    Tag(0x0028, 0x1100 + channel),
                    VR::US,
                    PrimitiveValue::U16(vec![4, 0, 8].into()),
                ));
                object.put(if padded {
                    DataElement::new(
                        Tag(0x0028, 0x1200 + channel),
                        VR::OW,
                        PrimitiveValue::U16(vec![0, 85, 170, 255].into()),
                    )
                } else {
                    DataElement::new(
                        Tag(0x0028, 0x1200 + channel),
                        VR::OB,
                        PrimitiveValue::U8(vec![0, 85, 170, 255].into()),
                    )
                });
            }
            if big {
                object.meta_mut().transfer_syntax = "1.2.840.10008.1.2.2".into();
                object.meta_mut().update_information_group_length();
            }
            let image = read(&object);
            assert_eq!(image.color.as_ref().unwrap().info.bits, 8);
            assert_eq!(
                image.rgba.as_ref().unwrap().as_ref(),
                &[
                    0, 0, 0, 255, 85, 85, 85, 255, 170, 170, 170, 255, 255, 255, 255, 255
                ]
            );
        }
    }
}

#[test]
fn palette_padding_masks_indices_before_palette_and_icc_presentation() {
    for signed in [false, true] {
        for icc in [false, true] {
            let mut object = palette(signed, false);
            object.put(if signed {
                DataElement::new(Tag(0x0028, 0x0120), VR::SS, PrimitiveValue::from(-2i16))
            } else {
                DataElement::new(Tag(0x0028, 0x0120), VR::US, PrimitiveValue::from(0u16))
            });
            if icc {
                object.put(DataElement::new(
                    Tag(0x0028, 0x2000),
                    VR::OB,
                    PrimitiveValue::U8(profile(false).into()),
                ));
            }
            let image = read(&object);
            assert!(image.valid_dimensions());
            assert!(
                image.validity.is_none(),
                "color uses its existing alpha validity"
            );
            assert_eq!(&image.rgba.as_ref().unwrap()[..4], &[0; 4]);
            assert!(!image.has_sample(0));
            assert!((1..4).all(|i| image.has_sample(i)));
            let histogram = crate::inspection::histogram(&image, 256, || false).unwrap();
            assert_eq!((histogram.count, histogram.missing), (3, 1));
            let stats = crate::measurements::color_roi_stats(&image, [[0., 0.], [1., 1.]]).unwrap();
            assert_eq!(stats.count, 3);
            assert_eq!(image.color.as_ref().unwrap().info.icc_sha256.is_some(), icc);
        }
    }
}
fn profile(p3: bool) -> Vec<u8> {
    let mut profile = if p3 {
        moxcms::ColorProfile::new_display_p3()
    } else {
        moxcms::ColorProfile::new_srgb()
    };
    profile.profile_class = moxcms::ProfileClass::InputDevice;
    profile.encode().unwrap()
}
#[test]
fn embedded_icc_conversion_matches_srgb_and_preserves_input_codes_and_hash() {
    let mut object = rgb(false);
    let original = read(&object);
    object.put(DataElement::new(
        Tag(0x0028, 0x2000),
        VR::OB,
        PrimitiveValue::U8(profile(false).into()),
    ));
    let converted = read(&object);
    for (a, b) in original
        .rgba
        .as_ref()
        .unwrap()
        .iter()
        .zip(converted.rgba.as_ref().unwrap().iter())
    {
        assert!(a.abs_diff(*b) <= 1);
    }
    assert_eq!(
        original.color.as_ref().unwrap().rgb,
        converted.color.as_ref().unwrap().rgb
    );
    assert!(converted.color.as_ref().unwrap().info.icc_sha256.is_some());
    assert!(
        crate::scene::ImageMetadata::from_frame(&converted)
            .color_space
            .unwrap()
            .starts_with("ICC converted")
    );
    assert_ne!(
        crate::scene::frame_pixel_hash(&original, || false).unwrap(),
        crate::scene::frame_pixel_hash(&converted, || false).unwrap()
    );
    object.put(DataElement::new(
        Tag(0x0028, 0x2000),
        VR::OB,
        PrimitiveValue::U8(profile(true).into()),
    ));
    let p3 = read(&object);
    assert_ne!(converted.rgba, p3.rgba);
}
#[test]
fn invalid_icc_and_palette_metadata_fail_instead_of_silently_assuming_srgb() {
    for mutation in 0..5 {
        let mut icc = profile(false);
        match mutation {
            0 => icc.truncate(130),
            1 => icc[12..16].copy_from_slice(b"mntr"),
            2 => icc[16..20].copy_from_slice(b"CMYK"),
            3 => icc[..4].copy_from_slice(&u32::MAX.to_be_bytes()),
            _ => icc[128..132].copy_from_slice(&u32::MAX.to_be_bytes()),
        }
        let mut object = rgb(false);
        object.put(DataElement::new(
            Tag(0x0028, 0x2000),
            VR::OB,
            PrimitiveValue::U8(icc.into()),
        ));
        let temp = Temp::new();
        let path = temp.0.join("invalid.dcm");
        object.write_to_file(&path).unwrap();
        let study = index_paths(&[path], || false, |_| {}).unwrap();
        assert!(study.series[0].read_frame(0, || false).is_err());
    }
    let mut object = palette(false, false);
    object.put(DataElement::new(
        Tag(0x0028, 0x1102),
        VR::US,
        PrimitiveValue::U16(vec![5, 1, 16].into()),
    ));
    let temp = Temp::new();
    let path = temp.0.join("invalid.dcm");
    object.write_to_file(&path).unwrap();
    let study = index_paths(&[path], || false, |_| {}).unwrap();
    assert!(study.series[0].read_frame(0, || false).is_err());
}
#[test]
fn source_precision_changes_identity_even_when_quantized_rgba_is_equal() {
    let mut object = rgb(false);
    depth(&mut object, 16, 16);
    object.put(DataElement::new(
        Tag(0x7fe0, 0x0010),
        VR::OW,
        PrimitiveValue::U16(vec![32768; 12].into()),
    ));
    let a = read(&object);
    object.put(DataElement::new(
        Tag(0x7fe0, 0x0010),
        VR::OW,
        PrimitiveValue::U16(vec![32769; 12].into()),
    ));
    let b = read(&object);
    assert_eq!(a.rgba, b.rgba);
    assert_ne!(
        crate::scene::frame_pixel_hash(&a, || false).unwrap(),
        crate::scene::frame_pixel_hash(&b, || false).unwrap()
    );
}

fn rle16(object: &mut dicom_object::DefaultDicomObject, values: &[u16; 12]) {
    let mut payload = vec![0u8; 64];
    payload[..4].copy_from_slice(&6u32.to_le_bytes());
    for c in 0..3 {
        for byte in 0..2 {
            let segment = c * 2 + byte;
            payload[4 + segment * 4..8 + segment * 4]
                .copy_from_slice(&(64 + segment as u32 * 6).to_le_bytes());
            for row in 0..2 {
                payload.push(1);
                for col in 0..2 {
                    payload.push((values[(row * 2 + col) * 3 + c] >> ((1 - byte) * 8)) as u8);
                }
            }
        }
    }
    object.meta_mut().transfer_syntax = "1.2.840.10008.1.2.5".into();
    object.put(DataElement::new(
        Tag(0x0028, 0x0006),
        VR::US,
        PrimitiveValue::from(1u16),
    ));
    object.put(DataElement::new(
        Tag(0x7fe0, 0x0010),
        VR::OB,
        dicom_core::DicomValue::PixelSequence(dicom_core::value::PixelFragmentSequence::new(
            Vec::new(),
            vec![payload],
        )),
    ));
}

#[test]
fn jpeg2000_rgb16_rct_fixture_preserves_all_channels() {
    let mut object = rgb(false);
    depth(&mut object, 16, 16);
    for tag in [0x0010, 0x0011] {
        object.put(DataElement::new(
            Tag(0x0028, tag),
            VR::US,
            PrimitiveValue::from(8u16),
        ));
    }
    object.put_str(Tag(0x0028, 0x0004), VR::CS, "YBR_RCT");
    object.meta_mut().transfer_syntax = "1.2.840.10008.1.2.4.90".into();
    object.meta_mut().update_information_group_length();
    object.put(DataElement::new(
        Tag(0x7fe0, 0x0010),
        VR::OB,
        dicom_core::DicomValue::PixelSequence(dicom_core::value::PixelFragmentSequence::new(
            Vec::new(),
            vec![include_bytes!("../../../../tests/fixtures/synthetic-rgb16.j2k").to_vec()],
        )),
    ));
    let image = read(&object);
    let rgb = &image.color.as_ref().unwrap().rgb;
    for y in 0..8 {
        for x in 0..8 {
            assert_eq!(
                &rgb[(y * 8 + x) * 3..(y * 8 + x) * 3 + 3],
                &[(x * 65535 / 7) as u16, (y * 65535 / 7) as u16, 32768]
            );
        }
    }
}
#[test]
fn truncated_rle_segments_are_rejected_before_the_codec_indexes_them() {
    let mut object = rgb(false);
    depth(&mut object, 16, 16);
    rle16(&mut object, &[65535; 12]);
    for bytes in [vec![0u8; 64], vec![0u8; 4]] {
        object.put(DataElement::new(
            Tag(0x7fe0, 0x0010),
            VR::OB,
            dicom_core::DicomValue::PixelSequence(dicom_core::value::PixelFragmentSequence::new(
                Vec::new(),
                vec![bytes],
            )),
        ));
        let temp = Temp::new();
        let path = temp.0.join("broken.dcm");
        object.write_to_file(&path).unwrap();
        let study = index_paths(&[path], || false, |_| {}).unwrap();
        assert!(study.series[0].read_frame(0, || false).is_err());
    }
}

#[test]
fn ybr_full_16bit_native_rle_and_jpegls_convert_chroma_exactly_once() {
    let values = [
        0, 32768, 32768, 65535, 32768, 32768, 16384, 32768, 32768, 32768, 32768, 32768,
    ];
    for codec in 0..3 {
        let mut object = rgb(false);
        depth(&mut object, 16, 16);
        object.put_str(Tag(0x0028, 0x0004), VR::CS, "YBR_FULL");
        object.put(DataElement::new(
            Tag(0x7fe0, 0x0010),
            VR::OW,
            PrimitiveValue::U16(values.to_vec().into()),
        ));
        if codec == 1 {
            rle16(&mut object, &values);
        } else if codec == 2 {
            super::jpegls::compress(&mut object, &values, 16, 2);
        }
        object.put_str(Tag(0x0028, 0x0004), VR::CS, "YBR_FULL");
        object.meta_mut().update_information_group_length();
        let image = read(&object);
        assert_eq!(
            &*image.color.as_ref().unwrap().rgb,
            &[
                0, 0, 0, 65535, 65535, 65535, 16384, 16384, 16384, 32768, 32768, 32768
            ]
        );
    }
}

#[test]
fn indexed_color_volume_budget_counts_retained_code_buffers_before_decoding() {
    let temp = Temp::new();
    let path = temp.0.join("large-color.dcm");
    let mut object = rgb(false);
    depth(&mut object, 16, 16);
    for tag in [0x0010, 0x0011] {
        object.put(DataElement::new(
            Tag(0x0028, tag),
            VR::US,
            PrimitiveValue::from(2048u16),
        ));
    }
    object.put_str(Tag(0x0028, 0x0008), VR::IS, "13");
    object.put(DataElement::new(
        Tag(0x7fe0, 0x0010),
        VR::OW,
        PrimitiveValue::U16(Vec::new().into()),
    ));
    let mut bytes = Vec::new();
    object.write_all(&mut bytes).unwrap();
    let n = bytes.len();
    let payload = 13u32 * 2048 * 2048 * 6;
    bytes[n - 4..].copy_from_slice(&payload.to_le_bytes());
    let mut file = super::super::large::create_sparse_file(&path);
    file.write_all(&bytes).unwrap();
    file.set_len(bytes.len() as u64 + payload as u64).unwrap();
    drop(file);
    let study = index_paths(&[path], || false, |_| {}).unwrap();
    let series = &study.series[0];
    assert_eq!(
        series.source.as_ref().unwrap().decoded_bytes(),
        13 * 2048 * 2048 * 10
    );
    assert_eq!(
        series.volume_frames(|| false).unwrap_err(),
        LoadError::Limit
    );
    assert_eq!(series.source.as_ref().unwrap().cache_bytes(), Some(0));
}
