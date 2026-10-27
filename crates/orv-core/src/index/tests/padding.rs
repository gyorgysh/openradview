// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use crate::{inspection, measurements};
use dicom_core::{DataElement, PrimitiveValue, Tag, VR};

fn object() -> dicom_object::DefaultDicomObject {
    let mut object =
        dicom_object::from_reader(std::io::Cursor::new(support::fixture(0., 1, false, false)))
            .unwrap();
    for tag in [0x1050, 0x1051] {
        object.remove_element(Tag(0x0028, tag));
    }
    object.put(DataElement::new(
        Tag(0x0028, 0x0120),
        VR::SS,
        PrimitiveValue::from(-1i16),
    ));
    object
}

#[test]
fn stored_padding_is_not_a_sample_or_part_of_automatic_window_histogram_or_roi() {
    let temp = Temp::new();
    let path = temp.0.join("synthetic-padding.dcm");
    object().write_to_file(&path).unwrap();
    let study = index_paths(std::slice::from_ref(&path), || false, |_| {}).unwrap();
    let indexed = study.series[0].read_frame(0, || false).unwrap();
    let fingerprint = crate::scene::fingerprint_series(&study.series[0], || false).unwrap();
    assert_eq!(
        fingerprint.frame_sha256.as_ref().unwrap()[0],
        crate::scene::frame_pixel_hash(&indexed, || false).unwrap()
    );
    let eager = crate::load::decode(&fs::read(&path).unwrap()).unwrap();
    for image in [&indexed, &eager.frames[0]] {
        assert!(
            !image.has_sample(3),
            "declared padding must not become an ordinary sample"
        );
        assert_eq!(&image.pixels[..3], &[-100., 0., 100.]);
        assert_eq!(
            image.default_window,
            crate::WindowLevel {
                function: crate::VoiFunction::LinearExact,
                width: 200.,
                center: 0.
            }
        );
        let histogram = inspection::histogram(image, 256, || false).unwrap();
        assert_eq!((histogram.count, histogram.missing), (3, 1));
        let roi = measurements::roi_stats(image, [[0., 0.], [1., 1.]]).unwrap();
        assert_eq!(roi.count, 3);
        assert_eq!((roi.min, roi.max, roi.mean), (-100., 100., 0.));
        assert!(inspection::sample(image, [1., 1.]).is_none());
    }
}

fn read(object: &dicom_object::DefaultDicomObject) -> Arc<crate::study::Frame> {
    let temp = Temp::new();
    let path = temp.0.join("synthetic.dcm");
    object.write_to_file(&path).unwrap();
    index_paths(&[path], || false, |_| {}).unwrap().series[0]
        .read_frame(0, || false)
        .unwrap()
}
fn field(object: &mut dicom_object::DefaultDicomObject, tag: u16, value: u16) {
    object.put(DataElement::new(
        Tag(0x0028, tag),
        VR::US,
        PrimitiveValue::from(value),
    ));
}

#[test]
fn padding_ranges_are_inclusive_signed_and_independent_of_inversion_or_rescale() {
    for bits in [8u16, 12, 16] {
        for mono1 in [false, true] {
            let mut object = object();
            field(&mut object, 0x0100, if bits == 8 { 8 } else { 16 });
            field(&mut object, 0x0101, bits);
            field(&mut object, 0x0102, bits - 1);
            object.put_str(
                Tag(0x0028, 0x0004),
                VR::CS,
                if mono1 { "MONOCHROME1" } else { "MONOCHROME2" },
            );
            for (tag, value) in [
                (0x0120, if mono1 { -1i16 } else { -2 }),
                (0x0121, if mono1 { -2 } else { -1 }),
            ] {
                object.put(DataElement::new(
                    Tag(0x0028, tag),
                    VR::SS,
                    PrimitiveValue::from(value),
                ));
            }
            object.put_str(Tag(0x0028, 0x1053), VR::DS, "-2");
            object.put_str(Tag(0x0028, 0x1052), VR::DS, "10");
            object.put(if bits == 8 {
                DataElement::new(
                    Tag(0x7fe0, 0x0010),
                    VR::OB,
                    PrimitiveValue::U8(vec![254, 255, 0, 5].into()),
                )
            } else {
                DataElement::new(
                    Tag(0x7fe0, 0x0010),
                    VR::OW,
                    PrimitiveValue::U16(vec![65534, 65535, 0, 5].into()),
                )
            });
            let image = read(&object);
            assert_eq!(image.validity.as_deref(), Some([0, 0, 1, 1].as_slice()));
            assert_eq!(&image.pixels[2..], &[10., 0.]);
            assert_eq!(image.monochrome1, mono1);
            assert_eq!(
                image.default_window,
                crate::WindowLevel {
                    function: crate::VoiFunction::LinearExact,
                    width: 10.,
                    center: 5.
                }
            );
            assert_eq!(image.default_window.normalize(0.), 0.);
            assert_eq!(image.default_window.normalize(10.), 1.);
            assert_eq!(image.byte_len(), 20);
        }
    }
}

#[test]
fn padding_uses_integer_codes_when_scaled_f32_values_collide_and_hashes_the_mask() {
    let mut object = object();
    field(&mut object, 0x0103, 0);
    field(&mut object, 0x0120, 0);
    object.put_str(Tag(0x0028, 0x1053), VR::DS, "1");
    object.put_str(Tag(0x0028, 0x1052), VR::DS, "16777216");
    object.put(DataElement::new(
        Tag(0x7fe0, 0x0010),
        VR::OW,
        PrimitiveValue::U16(vec![0, 1, 2, 3].into()),
    ));
    let padded = read(&object);
    assert_eq!(
        padded.pixels[0], padded.pixels[1],
        "fixture deliberately has a scaled f32 collision"
    );
    assert!(!padded.has_sample(0));
    assert!(padded.has_sample(1));
    object.remove_element(Tag(0x0028, 0x0120));
    let unpadded = read(&object);
    assert_eq!(padded.pixels, unpadded.pixels);
    assert!(unpadded.validity.is_none());
    assert_ne!(
        crate::scene::frame_pixel_hash(&padded, || false).unwrap(),
        crate::scene::frame_pixel_hash(&unpadded, || false).unwrap()
    );
    field(&mut object, 0x0120, 65535);
    assert!(
        read(&object).validity.is_none(),
        "unused padding needs no retained mask"
    );
}

#[test]
fn padding_survives_native_endianness_compression_zip_and_multiframe_selection() {
    use dicom_pixeldata::Transcode;
    for codec in 0..6 {
        let temp = Temp::new();
        let mut object = object();
        field(&mut object, 0x0103, 0);
        field(&mut object, 0x0120, 65535);
        for tag in [0x0010, 0x0011] {
            field(&mut object, tag, 8);
        }
        object.put(DataElement::new(
            Tag(0x7fe0, 0x0010),
            VR::OW,
            PrimitiveValue::U16([0u16, 50, 100, 65535].repeat(16).into()),
        ));
        match codec {
            1 => object.meta_mut().transfer_syntax = "1.2.840.10008.1.2.2".into(),
            2 => object.meta_mut().transfer_syntax = "1.2.840.10008.1.2.1.99".into(),
            3 => {
                // Original independent two-segment literal PackBits stream.
                let values = [0u16, 50, 100, 65535].repeat(16);
                let mut data = vec![0u8; 64];
                data[..4].copy_from_slice(&2u32.to_le_bytes());
                data[4..8].copy_from_slice(&64u32.to_le_bytes());
                data[8..12].copy_from_slice(&129u32.to_le_bytes());
                data.push(63);
                data.extend(values.iter().map(|v| (v >> 8) as u8));
                data.push(63);
                data.extend(values.iter().map(|v| *v as u8));
                object.meta_mut().transfer_syntax = "1.2.840.10008.1.2.5".into();
                object.put(DataElement::new(
                    Tag(0x7fe0, 0x0010),
                    VR::OB,
                    dicom_core::DicomValue::PixelSequence(
                        dicom_core::value::PixelFragmentSequence::new(Vec::new(), vec![data]),
                    ),
                ));
            }
            4 => object
                .transcode(
                    &dicom_transfer_syntax_registry::entries::JPEG_LS_LOSSLESS_IMAGE_COMPRESSION
                        .erased(),
                )
                .unwrap(),
            5 => {
                object.meta_mut().transfer_syntax = "1.2.840.10008.1.2.4.90".into();
                object.put(DataElement::new(
                    Tag(0x7fe0, 0x0010),
                    VR::OB,
                    dicom_core::DicomValue::PixelSequence(
                        dicom_core::value::PixelFragmentSequence::new(
                            Vec::new(),
                            vec![
                                include_bytes!("../../../tests/fixtures/synthetic-u16.j2k")
                                    .to_vec(),
                            ],
                        ),
                    ),
                ));
            }
            _ => {}
        }
        object.meta_mut().update_information_group_length();
        let mut bytes = Vec::new();
        object.write_all(&mut bytes).unwrap();
        let path = temp.0.join("synthetic.zip");
        let mut zip = zip::ZipWriter::new(File::create(&path).unwrap());
        zip.start_file("synthetic.dcm", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(&bytes).unwrap();
        zip.finish().unwrap();
        let study = index_paths(&[path], || false, |_| {}).unwrap();
        let frame = study.series[0].read_frame(0, || false).unwrap();
        assert_eq!(
            frame.validity.as_deref(),
            Some([1u8, 1, 1, 0].repeat(16).as_slice()),
            "codec {codec}"
        );
        let hist = inspection::histogram(&frame, 256, || false).unwrap();
        assert_eq!((hist.count, hist.missing), (48, 16));
        assert_eq!((hist.channels[0].min, hist.channels[0].max), (-100., 100.));
    }
    let mut object = object();
    object.put_str(Tag(0x0028, 0x0008), VR::IS, "2");
    object.put(DataElement::new(
        Tag(0x7fe0, 0x0010),
        VR::OW,
        PrimitiveValue::U16(vec![0, 50, 100, 65535, 65535, 50, 0, 100].into()),
    ));
    let temp = Temp::new();
    let path = temp.0.join("multi.dcm");
    object.write_to_file(&path).unwrap();
    let study = index_paths(&[path], || false, |_| {}).unwrap();
    assert_eq!(
        study.series[0]
            .read_frame(0, || false)
            .unwrap()
            .validity
            .as_deref(),
        Some([1, 1, 1, 0].as_slice())
    );
    assert_eq!(
        study.series[0]
            .read_frame(1, || false)
            .unwrap()
            .validity
            .as_deref(),
        Some([0, 1, 1, 1].as_slice())
    );
}

#[test]
fn invalid_padding_metadata_is_refused_and_all_padding_is_a_valid_empty_image() {
    for mutation in 0..5 {
        let mut object = object();
        match mutation {
            0 => {
                object.remove_element(Tag(0x0028, 0x0120));
                object.put(DataElement::new(
                    Tag(0x0028, 0x0121),
                    VR::SS,
                    PrimitiveValue::from(-1i16),
                ));
            }
            1 => {
                object.put(DataElement::new(
                    Tag(0x0028, 0x0120),
                    VR::SS,
                    PrimitiveValue::I16(vec![-1, -2].into()),
                ));
            }
            2 => {
                field(&mut object, 0x0101, 12);
                field(&mut object, 0x0102, 11);
                object.put(DataElement::new(
                    Tag(0x0028, 0x0120),
                    VR::SS,
                    PrimitiveValue::from(-3000i16),
                ));
            }
            3 => {
                object.put_str(Tag(0x0028, 0x0120), VR::DS, "-1");
            }
            _ => {
                object.put(DataElement::new(
                    Tag(0x0028, 0x0121),
                    VR::SS,
                    PrimitiveValue::from(-2i16),
                ));
            }
        };
        assert!(
            crate::dicom_padding::Padding::read(&object).is_err(),
            "mutation {mutation}"
        );
    }
    let mut object = object();
    object.put(DataElement::new(
        Tag(0x7fe0, 0x0010),
        VR::OW,
        PrimitiveValue::U16(vec![65535; 4].into()),
    ));
    let image = read(&object);
    assert!(image.valid_dimensions());
    assert!((0..4).all(|i| !image.has_sample(i)));
    assert_eq!(
        image.default_window,
        crate::WindowLevel {
            function: crate::VoiFunction::LinearExact,
            width: 1.,
            center: 0.
        }
    );
    assert!(inspection::histogram(&image, 256, || false).is_err());
    assert!(measurements::roi_stats(&image, [[0., 0.], [1., 1.]]).is_err());
}

#[test]
fn padding_mask_cancels_before_publication_and_invalid_masks_cannot_be_fingerprinted() {
    use dicom_pixeldata::PixelDecoder;
    let object = object();
    let padding = crate::dicom_padding::Padding::read(&object)
        .unwrap()
        .unwrap();
    let decoded = object.decode_pixel_data().unwrap();
    assert_eq!(
        padding.mask(&decoded, 0, &|| true).unwrap_err(),
        LoadError::Cancelled
    );
    let calls = std::cell::Cell::new(0);
    assert_eq!(
        padding
            .mask(&decoded, 0, &|| {
                let old = calls.get();
                calls.set(old + 1);
                old > 0
            })
            .unwrap_err(),
        LoadError::Cancelled
    );
    assert!(padding.mask(&decoded, 1, &|| false).is_err());
    for mask in [vec![1u8; 3], vec![2; 4]] {
        let mut frame = (*read(&object)).clone();
        frame.validity = Some(mask.into());
        assert!(!frame.valid_dimensions());
        assert!(crate::scene::frame_pixel_hash(&frame, || false).is_err());
    }
}

#[test]
fn implicit_vr_preserves_signed_padding_and_unused_high_bits() {
    let mut object = object();
    object.meta_mut().transfer_syntax = "1.2.840.10008.1.2".into();
    object.meta_mut().update_information_group_length();
    field(&mut object, 0x0101, 12);
    field(&mut object, 0x0102, 11);
    object.put(DataElement::new(
        Tag(0x7fe0, 0x0010),
        VR::OW,
        PrimitiveValue::U16(vec![0x0fff, 0xffff, 0, 50].into()),
    ));
    let image = read(&object);
    assert_eq!(image.validity.as_deref(), Some([0, 0, 1, 1].as_slice()));
    assert_eq!(&image.pixels[2..], &[-100., 0.]);
}
#[test]
fn padded_volume_is_budgeted_before_materializing_but_remains_viewable_in_2d() {
    let temp = Temp::new();
    let frames = (MAX_VOLUME_BYTES / (512 * 512 * 5) + 1) as u32;
    let (path, _, _) = super::large::native_with_padding(&temp.0, frames, Some(0));
    let study = index_paths(&[path], || false, |_| {}).unwrap();
    let series = &study.series[0];
    let indexed = series.source.as_ref().unwrap();
    assert_eq!(indexed.decoded_bytes(), 512 * 512 * u64::from(frames) * 5);
    assert!(indexed.decoded_bytes() > MAX_VOLUME_BYTES);
    assert_eq!(indexed.materialize(|| false).unwrap_err(), LoadError::Limit);
    assert_eq!(indexed.cache_bytes(), Some(0), "Reject before pixel I/O");
    let first = series.read_frame(0, || false).unwrap();
    assert!(!first.has_sample(0));
    assert!(first.has_sample(1));
    assert_eq!(first.byte_len(), 512 * 512 * 5);
    assert_eq!(indexed.cache_bytes(), Some(first.byte_len()));
}
