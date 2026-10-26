// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use dicom_core::{DataElement, PrimitiveValue, Tag, VR};
mod expanded;
mod jpegls;
fn rgb(planar: bool) -> dicom_object::DefaultDicomObject {
    let mut object =
        dicom_object::from_reader(std::io::Cursor::new(support::fixture(0., 1, false, false)))
            .unwrap();
    for (tag, n) in [
        (0x0002, 3u16),
        (0x0006, u16::from(planar)),
        (0x0100, 8),
        (0x0101, 8),
        (0x0102, 7),
        (0x0103, 0),
    ] {
        object.put(DataElement::new(
            Tag(0x0028, tag),
            VR::US,
            PrimitiveValue::from(n),
        ));
    }
    object.put_str(Tag(0x0028, 0x0004), VR::CS, "RGB");
    let bytes = if planar {
        vec![255, 0, 0, 128, 0, 255, 0, 64, 0, 0, 255, 32]
    } else {
        vec![255, 0, 0, 0, 255, 0, 0, 0, 255, 128, 64, 32]
    };
    object.put(DataElement::new(
        Tag(0x7fe0, 0x0010),
        VR::OB,
        PrimitiveValue::U8(bytes.into()),
    ));
    object
}
#[test]
fn native_planar_and_jpegls_color_keep_channels_hashes_and_disable_scalar_roi() {
    let expected = vec![
        255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 128, 64, 32, 255,
    ];
    for (planar, compressed) in [(false, false), (true, false), (false, true)] {
        let temp = Temp::new();
        let mut object = rgb(planar);
        if compressed {
            jpegls::compress(
                &mut object,
                &[255, 0, 0, 0, 255, 0, 0, 0, 255, 128, 64, 32],
                8,
                2,
            );
        }
        object.meta_mut().update_information_group_length();
        let path = temp.0.join("rgb.dcm");
        object.write_to_file(&path).unwrap();
        let study = index_paths(&[path], || false, |_| {}).unwrap();
        let series = &study.series[0];
        let image = series.read_frame(0, || false).unwrap();
        assert_eq!(image.rgba.as_deref(), Some(expected.as_slice()));
        assert!(image.pixels.is_empty());
        assert!(image.valid_dimensions());
        assert!(
            crate::measurements::roi_stats(&image, [[0., 0.], [1., 1.]])
                .unwrap_err()
                .contains("scalar")
        );
        assert!(crate::volume::Volume::new(series.volume_frames(|| false).unwrap()).is_err());
        let before = crate::scene::frame_pixel_hash(&image, || false).unwrap();
        let mut changed = (*image).clone();
        let mut pixels = expected.clone();
        pixels[0] -= 1;
        changed.rgba = Some(pixels.into());
        assert_ne!(
            before,
            crate::scene::frame_pixel_hash(&changed, || false).unwrap()
        );
        assert!(crate::scene::fingerprint_series(series, || false).is_ok());
        assert!(
            crate::scene::ImageMetadata::from_frame(&image)
                .color_space
                .is_some()
        );
    }
}
#[test]
fn native_ybr_full_converts_chroma_once_and_unsupported_palette_is_refused() {
    let temp = Temp::new();
    let mut object = rgb(false);
    object.put_str(Tag(0x0028, 0x0004), VR::CS, "YBR_FULL");
    object.put(DataElement::new(
        Tag(0x7fe0, 0x0010),
        VR::OB,
        PrimitiveValue::U8(vec![0, 128, 128, 255, 128, 128, 76, 85, 255, 128, 128, 128].into()),
    ));
    let path = temp.0.join("ybr.dcm");
    object.write_to_file(&path).unwrap();
    let study = index_paths(std::slice::from_ref(&path), || false, |_| {}).unwrap();
    let image = study.series[0].read_frame(0, || false).unwrap();
    assert_eq!(
        image.rgba.as_deref(),
        Some(
            [
                0, 0, 0, 255, 255, 255, 255, 255, 254, 0, 0, 255, 128, 128, 128, 255
            ]
            .as_slice()
        )
    );
    object.put_str(Tag(0x0028, 0x0004), VR::CS, "PALETTE COLOR");
    object.write_to_file(&path).unwrap();
    assert!(index_paths(&[path], || false, |_| {}).is_err());
}

#[test]
fn jpeg_ybr_and_jpeg2000_rct_color_decode_once_and_match_synthetic_values() {
    use dicom_core::DicomValue;
    for jpeg2k in [false, true] {
        let temp = Temp::new();
        let mut object = rgb(false);
        for tag in [0x0010, 0x0011] {
            object.put(DataElement::new(
                Tag(0x0028, tag),
                VR::US,
                PrimitiveValue::from(8u16),
            ));
        }
        object.meta_mut().transfer_syntax = if jpeg2k {
            "1.2.840.10008.1.2.4.90"
        } else {
            "1.2.840.10008.1.2.4.50"
        }
        .into();
        object.meta_mut().update_information_group_length();
        object.put_str(
            Tag(0x0028, 0x0004),
            VR::CS,
            if jpeg2k { "YBR_RCT" } else { "YBR_FULL_422" },
        );
        let payload = if jpeg2k {
            include_bytes!("../../../tests/fixtures/synthetic-rgb.j2k").as_slice()
        } else {
            include_bytes!("../../../tests/fixtures/synthetic-ybr.jpg").as_slice()
        };
        object.put(DataElement::new(
            Tag(0x7fe0, 0x0010),
            VR::OB,
            DicomValue::PixelSequence(dicom_core::value::PixelFragmentSequence::new(
                Vec::new(),
                vec![payload.to_vec()],
            )),
        ));
        let path = temp.0.join("encoded-color.dcm");
        object.write_to_file(&path).unwrap();
        let study = index_paths(&[path], || false, |_| {}).unwrap();
        let image = study.series[0].read_frame(0, || false).unwrap();
        let rgba = image.rgba.as_deref().unwrap();
        if jpeg2k {
            assert_eq!(
                rgba,
                &[
                    255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 128, 64, 32, 255
                ]
                .repeat(16)
            );
        } else {
            for pixel in rgba.as_chunks::<4>().0.iter() {
                for (actual, expected) in pixel.iter().zip([32, 96, 192, 255]) {
                    assert!((i16::from(*actual) - expected).abs() <= 2);
                }
            }
        }
        let mut bytes = Vec::new();
        object.write_all(&mut bytes).unwrap();
        assert_eq!(
            crate::load::decode(&bytes).unwrap().frames[0].rgba,
            image.rgba
        );
    }
}

#[test]
fn rle_color_preserves_rgb_channels_instead_of_planar_or_luma_output() {
    use dicom_core::DicomValue;
    let temp = Temp::new();
    let mut object = rgb(false);
    object.meta_mut().transfer_syntax = "1.2.840.10008.1.2.5".into();
    object.meta_mut().update_information_group_length();
    object.put(DataElement::new(
        Tag(0x0028, 0x0006),
        VR::US,
        PrimitiveValue::from(1u16),
    ));
    let mut payload = vec![0u8; 64];
    payload[..4].copy_from_slice(&3u32.to_le_bytes());
    for (i, channel) in [[255u8, 0, 0, 128], [0, 255, 0, 64], [0, 0, 255, 32]]
        .into_iter()
        .enumerate()
    {
        payload[4 + i * 4..8 + i * 4].copy_from_slice(&(64 + 5 * i as u32).to_le_bytes());
        payload.push(3);
        payload.extend(channel);
    }
    object.put(DataElement::new(
        Tag(0x7fe0, 0x0010),
        VR::OB,
        DicomValue::PixelSequence(dicom_core::value::PixelFragmentSequence::new(
            Vec::new(),
            vec![payload],
        )),
    ));
    let path = temp.0.join("rle-color.dcm");
    object.write_to_file(&path).unwrap();
    let study = index_paths(&[path], || false, |_| {}).unwrap();
    assert_eq!(
        study.series[0]
            .read_frame(0, || false)
            .unwrap()
            .rgba
            .as_deref(),
        Some(
            [
                255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 128, 64, 32, 255
            ]
            .as_slice()
        )
    );
}
