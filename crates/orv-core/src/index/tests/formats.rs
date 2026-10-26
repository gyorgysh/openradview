// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use dicom_core::{DataElement, DicomValue, Length, PrimitiveValue, Tag, VR};

fn rle(values: [u16; 4]) -> Vec<u8> {
    let mut data = vec![0u8; 64];
    data[..4].copy_from_slice(&2u32.to_le_bytes());
    data[4..8].copy_from_slice(&64u32.to_le_bytes());
    data[8..12].copy_from_slice(&69u32.to_le_bytes());
    data.push(3);
    data.extend(values.map(|v| (v >> 8) as u8));
    data.push(3);
    data.extend(values.map(|v| v as u8));
    data
}

#[test]
fn bounded_rle_frames_use_basic_empty_and_extended_offsets_without_loading_large_padding() {
    for table in [0, 1, 2] {
        let temp = Temp::new();
        let mut object =
            dicom_object::from_reader(std::io::Cursor::new(support::fixture(0., 1, false, false)))
                .unwrap();
        object.meta_mut().transfer_syntax = "1.2.840.10008.1.2.5".into();
        object.put_str(Tag(0x0028, 0x0008), VR::IS, "2");
        let fragments = vec![rle([0, 50, 100, 65535]), rle([100, 50, 0, 65535])];
        let next = fragments[0].len() as u32 + 8;
        let offsets = if table == 0 { vec![0u32, next] } else { vec![] };
        object.put(DataElement::new(
            Tag(0x7fe0, 0x0010),
            VR::OB,
            DicomValue::PixelSequence(dicom_core::value::PixelFragmentSequence::new(
                offsets, fragments,
            )),
        ));
        if table == 2 {
            object.put(DataElement::new(
                Tag(0x7fe0, 0x0001),
                VR::OV,
                PrimitiveValue::U64(vec![0u64, next as u64].into()),
            ));
            object.put(DataElement::new(
                Tag(0x7fe0, 0x0002),
                VR::OV,
                PrimitiveValue::U64(vec![74u64, 74].into()),
            ));
        }
        let path = temp.0.join("synthetic-rle.dcm");
        object.write_to_file(&path).unwrap();
        // Valid trailing padding makes the object larger than the eager parser's
        // 64MiB cap. Only the requested compressed frame must be read/decoded.
        let mut file = File::options().append(true).open(&path).unwrap();
        file.write_all(&[0xfc, 0xff, 0xfc, 0xff, b'O', b'B', 0, 0])
            .unwrap();
        file.write_all(&(80u32 * 1024 * 1024).to_le_bytes())
            .unwrap();
        let size = file.metadata().unwrap().len();
        file.set_len(size + 80 * 1024 * 1024).unwrap();
        drop(file);
        let study = index_paths(&[path], || false, |_| {}).unwrap();
        let series = &study.series[0];
        assert_eq!(series.frame_count(), 2);
        assert_eq!(
            &series.read_frame(1, || false).unwrap().pixels[..4],
            &[100., 0., -100., -102.]
        );
        assert_eq!(
            &series.read_frame(0, || false).unwrap().pixels[..4],
            &[-100., 0., 100., -102.]
        );
    }
}

#[test]
fn enhanced_per_frame_rescale_is_preserved_but_spatial_geometry_is_not_guessed() {
    let temp = Temp::new();
    let mut object =
        dicom_object::from_reader(std::io::Cursor::new(support::fixture(0., 1, false, false)))
            .unwrap();
    object.remove_element(Tag(0x0028, 0x1052));
    object.remove_element(Tag(0x0028, 0x1053));
    object.put_str(Tag(0x0028, 0x0008), VR::IS, "2");
    object.put(DataElement::new(
        Tag(0x7fe0, 0x0010),
        VR::OW,
        PrimitiveValue::U16(vec![0u16, 50, 100, 65535, 0, 50, 100, 65535].into()),
    ));
    let items = [("-100", "2"), ("50", "3")]
        .into_iter()
        .map(|(intercept, slope)| {
            let transform = dicom_object::InMemDicomObject::from_element_iter([
                DataElement::new(Tag(0x0028, 0x1052), VR::DS, intercept),
                DataElement::new(Tag(0x0028, 0x1053), VR::DS, slope),
            ]);
            dicom_object::InMemDicomObject::from_element_iter([DataElement::new(
                Tag(0x0028, 0x9145),
                VR::SQ,
                DicomValue::new_sequence(vec![transform], Length::UNDEFINED),
            )])
        })
        .collect::<Vec<_>>();
    object.put(DataElement::new(
        Tag(0x5200, 0x9230),
        VR::SQ,
        DicomValue::new_sequence(items, Length::UNDEFINED),
    ));
    let path = temp.0.join("synthetic-enhanced.dcm");
    object.write_to_file(&path).unwrap();
    let study = index_paths(&[path], || false, |_| {}).unwrap();
    let a = study.series[0].read_frame(0, || false).unwrap();
    let b = study.series[0].read_frame(1, || false).unwrap();
    assert_eq!(&a.pixels[..4], &[-100., 0., 100., -102.]);
    assert_eq!(&b.pixels[..4], &[50., 200., 350., 47.]);
    assert!(a.geometry.is_none() && b.geometry.is_none());
}

#[test]
fn deflated_multiframe_forward_and_backward_navigation_and_import_cancellation_work() {
    let temp = Temp::new();
    let (native, _, _) = large::native_frames(&temp.0, 256);
    let path = temp.0.join("synthetic-deflated.zip");
    let mut zip = zip::ZipWriter::new(File::create(&path).unwrap());
    zip.start_file(
        "synthetic.dcm",
        zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .compression_level(Some(1)),
    )
    .unwrap();
    std::io::copy(&mut File::open(native).unwrap(), &mut zip).unwrap();
    zip.finish().unwrap();
    let study = index_paths(std::slice::from_ref(&path), || false, |_| {}).unwrap();
    for i in [0, 1, 2, 255, 127, 128] {
        let image = study.series[0].read_frame(i, || false).unwrap();
        let expected = if i == 0 {
            [-100., 0., 100., -102.]
        } else if i == 255 {
            [100., 0., -100., -102.]
        } else {
            [-100.; 4]
        };
        assert_eq!(&image.pixels[..4], &expected);
    }
    let cancel = AtomicBool::new(false);
    let result = index_paths(
        &[path],
        || cancel.load(Ordering::Relaxed),
        |p| {
            if p.checked_bytes > 1024 * 1024 {
                cancel.store(true, Ordering::Relaxed);
            }
        },
    );
    assert_eq!(result.unwrap_err(), LoadError::Cancelled);
}

#[test]
fn huge_declared_metadata_lengths_are_rejected_before_value_allocation_and_bad_crc_is_not_ignored()
{
    let fixture = support::fixture(0., 1, false, false);
    let group = u32::from_le_bytes(fixture[140..144].try_into().unwrap()) as usize;
    let dataset = 144 + group;
    let mut bad = fixture[..dataset].to_vec();
    bad.extend_from_slice(&[8, 0, 0x11, 0x11, b'U', b'N', 0, 0]);
    bad.extend_from_slice(&0x7fff_fff0u32.to_le_bytes());
    assert!(matches!(
        read_header(&mut std::io::Cursor::new(bad), || false),
        Err(LoadError::Limit)
    ));
    let mut bad = fixture[..dataset].to_vec();
    bad[140..144].copy_from_slice(&((group + 12) as u32).to_le_bytes());
    bad.extend_from_slice(&[2, 0, 2, 1, b'O', b'B', 0, 0]);
    bad.extend_from_slice(&0x7fff_fff0u32.to_le_bytes());
    assert!(matches!(
        read_header(&mut std::io::Cursor::new(bad), || false),
        Err(LoadError::Limit)
    ));
    let temp = Temp::new();
    let path = temp.0.join("bad-crc.zip");
    let mut zip = zip::ZipWriter::new(File::create(&path).unwrap());
    zip.start_file(
        "synthetic.dcm",
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored),
    )
    .unwrap();
    zip.write_all(&fixture).unwrap();
    zip.finish().unwrap();
    let offset = {
        let mut archive = zip::ZipArchive::new(File::open(&path).unwrap()).unwrap();
        archive.by_index(0).unwrap().data_start().unwrap()
    };
    let pixel = read_header(&mut std::io::Cursor::new(fixture), || false)
        .unwrap()
        .offset;
    let mut file = File::options().write(true).open(&path).unwrap();
    file.seek(SeekFrom::Start(offset + pixel)).unwrap();
    file.write_all(&[77]).unwrap();
    drop(file);
    assert_eq!(
        index_paths(&[path], || false, |_| {}).unwrap_err(),
        LoadError::Unsupported
    );
}

#[test]
fn deflated_dicom_dataset_streams_inside_native_and_zip_sources() {
    for zip_method in [
        None,
        Some(zip::CompressionMethod::Stored),
        Some(zip::CompressionMethod::Deflated),
    ] {
        let temp = Temp::new();
        let mut object =
            dicom_object::from_reader(std::io::Cursor::new(support::fixture(0., 1, false, false)))
                .unwrap();
        object.meta_mut().transfer_syntax = "1.2.840.10008.1.2.1.99".into();
        object.meta_mut().update_information_group_length();
        let mut bytes = Vec::new();
        object.write_all(&mut bytes).unwrap();
        let path = temp.0.join(if zip_method.is_some() {
            "dataset.zip"
        } else {
            "dataset.dcm"
        });
        if let Some(method) = zip_method {
            let mut writer = zip::ZipWriter::new(File::create(&path).unwrap());
            writer
                .start_file(
                    "image.dcm",
                    zip::write::SimpleFileOptions::default().compression_method(method),
                )
                .unwrap();
            writer.write_all(&bytes).unwrap();
            writer.finish().unwrap();
        } else {
            fs::write(&path, &bytes).unwrap();
        }
        let study = index_paths(&[path], || false, |_| {}).unwrap();
        assert!(study.series[0].frames.is_empty());
        assert_eq!(
            &study.series[0].read_frame(0, || false).unwrap().pixels[..],
            &[-100., 0., 100., -102.]
        );
    }
}

#[test]
fn jpeg2000_and_jpegls_decode_unsigned16_samples_without_baking_window() {
    use dicom_pixeldata::Transcode;
    for jpeg2000 in [true, false] {
        let temp = Temp::new();
        let mut object =
            dicom_object::from_reader(std::io::Cursor::new(support::fixture(0., 1, false, false)))
                .unwrap();
        for tag in [Tag(0x0028, 0x0010), Tag(0x0028, 0x0011)] {
            object.put(DataElement::new(
                tag,
                VR::US,
                PrimitiveValue::U16(vec![8].into()),
            ));
        }
        object.put(DataElement::new(
            Tag(0x0028, 0x0103),
            VR::US,
            PrimitiveValue::U16(vec![0].into()),
        ));
        if jpeg2000 {
            object.meta_mut().transfer_syntax = "1.2.840.10008.1.2.4.90".into();
            object.put(DataElement::new(
                Tag(0x7fe0, 0x0010),
                VR::OB,
                DicomValue::PixelSequence(dicom_core::value::PixelFragmentSequence::new(
                    Vec::new(),
                    vec![include_bytes!("../../../tests/fixtures/synthetic-u16.j2k").to_vec()],
                )),
            ));
        } else {
            object.put(DataElement::new(
                Tag(0x7fe0, 0x0010),
                VR::OW,
                PrimitiveValue::U16([0u16, 50, 100, 65535].repeat(16).into()),
            ));
            object
                .transcode(
                    &dicom_transfer_syntax_registry::entries::JPEG_LS_LOSSLESS_IMAGE_COMPRESSION
                        .erased(),
                )
                .unwrap();
        }
        object.meta_mut().update_information_group_length();
        let path = temp.0.join("codec.dcm");
        object.write_to_file(&path).unwrap();
        let study = index_paths(&[path], || false, |_| {}).unwrap();
        let frame = study.series[0].read_frame(0, || false).unwrap();
        assert_eq!(&frame.pixels[..], &[-100., 0., 100., 130970.].repeat(16));
        // Encoded dimensions must agree with the bounded DICOM header before
        // allocation, including an attacker-controlled oversized codestream.
        let mut payload = object
            .element_by_name("PixelData")
            .unwrap()
            .value()
            .fragments()
            .unwrap()[0]
            .clone();
        if jpeg2000 {
            payload[8..12].copy_from_slice(&u32::MAX.to_be_bytes());
        } else {
            let sof = payload.windows(2).position(|b| b == [0xff, 0xf7]).unwrap();
            payload[sof + 7..sof + 9].copy_from_slice(&u16::MAX.to_be_bytes());
        }
        object.put(DataElement::new(
            Tag(0x7fe0, 0x0010),
            VR::OB,
            DicomValue::PixelSequence(dicom_core::value::PixelFragmentSequence::new(
                Vec::new(),
                vec![payload],
            )),
        ));
        let path = temp.0.join("mismatched-codestream.dcm");
        object.write_to_file(&path).unwrap();
        let invalid = index_paths(&[path], || false, |_| {}).unwrap();
        assert_eq!(
            invalid.series[0].read_frame(0, || false).unwrap_err(),
            LoadError::Unsupported
        );
    }
}

#[test]
fn irreversible_jpeg2000_matches_independent_decoder_values_before_rescale() {
    let temp = Temp::new();
    let mut object =
        dicom_object::from_reader(std::io::Cursor::new(support::fixture(0., 1, false, false)))
            .unwrap();
    for tag in [Tag(0x0028, 0x0010), Tag(0x0028, 0x0011)] {
        object.put(DataElement::new(tag, VR::US, PrimitiveValue::from(8u16)));
    }
    object.put(DataElement::new(
        Tag(0x0028, 0x0103),
        VR::US,
        PrimitiveValue::from(0u16),
    ));
    object.meta_mut().transfer_syntax = "1.2.840.10008.1.2.4.91".into();
    object.meta_mut().update_information_group_length();
    object.put(DataElement::new(
        Tag(0x7fe0, 0x0010),
        VR::OB,
        DicomValue::PixelSequence(dicom_core::value::PixelFragmentSequence::new(
            Vec::new(),
            vec![include_bytes!("../../../tests/fixtures/synthetic-u16-lossy.j2k").to_vec()],
        )),
    ));
    let path = temp.0.join("lossy.dcm");
    object.write_to_file(&path).unwrap();
    let study = index_paths(&[path], || false, |_| {}).unwrap();
    let expected: Vec<f32> = include_bytes!("../../../tests/fixtures/synthetic-u16-lossy.u16le")
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| u16::from_le_bytes(*b) as f32 * 2. - 100.)
        .collect();
    assert_eq!(
        &study.series[0].read_frame(0, || false).unwrap().pixels[..],
        &expected
    );
}
