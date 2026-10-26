// SPDX-License-Identifier: GPL-3.0-or-later
//! Known channel values, explicitly encoded in each JPEG-LS interleave mode.
//! CharLS native buffers are planar for None and by pixel for Line/Sample.
//! Do not use the DICOM transcoder's implicit encoder layout as an oracle.
use super::*;
use charls::{CharLS, FrameInfo, InterleaveMode};

pub(super) fn compress(
    object: &mut dicom_object::DefaultDicomObject,
    values: &[u16],
    stored: u16,
    layout: u8,
) {
    let count = values.len() / 3;
    let ordered: Vec<u16> = (0..values.len())
        .map(|index| {
            let (pixel, channel) = match layout {
                0 => (index % count, index / count),
                _ => (index / 3, index % 3),
            };
            values[pixel * 3 + channel]
        })
        .collect();
    let bytes: Vec<u8> = if stored == 8 {
        ordered.iter().map(|v| *v as u8).collect()
    } else {
        ordered.iter().flat_map(|v| v.to_ne_bytes()).collect()
    };
    let mut encoder = CharLS::default();
    encoder
        .set_interleave_mode(match layout {
            0 => InterleaveMode::None,
            1 => InterleaveMode::Line,
            _ => InterleaveMode::Sample,
        })
        .unwrap();
    let mut encoded = encoder
        .encode(
            FrameInfo {
                width: 2,
                height: 2,
                bits_per_sample: stored as i32,
                component_count: 3,
            },
            0,
            &bytes,
        )
        .unwrap();
    if !encoded.len().is_multiple_of(2) {
        encoded.push(0);
    }
    object.meta_mut().transfer_syntax = "1.2.840.10008.1.2.4.80".into();
    object.put(DataElement::new(
        Tag(0x7fe0, 0x0010),
        VR::OB,
        dicom_core::DicomValue::PixelSequence(dicom_core::value::PixelFragmentSequence::new(
            Vec::new(),
            vec![encoded],
        )),
    ));
    for (tag, value) in [
        (0x0100, if stored == 8 { 8 } else { 16 }),
        (0x0101, stored),
        (0x0102, stored - 1),
        (0x0006, 0),
    ] {
        object.put(DataElement::new(
            Tag(0x0028, tag),
            VR::US,
            PrimitiveValue::from(value),
        ));
    }
    object.meta_mut().update_information_group_length();
}

#[test]
fn all_jpegls_interleave_modes_preserve_rgb_and_ybr_code_values() {
    for stored in [8u16, 12, 16] {
        let max = ((1u32 << stored) - 1) as u16;
        let middle = 1u16 << (stored - 1);
        for ybr in [false, true] {
            let rgb_values = vec![
                max,
                1,
                2,
                3,
                max - 4,
                5,
                6,
                7,
                max - 8,
                max / 2,
                max / 4,
                max / 8,
            ];
            let values = if ybr {
                vec![
                    0,
                    middle,
                    middle,
                    max,
                    middle,
                    middle,
                    max / 4,
                    middle,
                    middle,
                    max / 2,
                    middle,
                    middle,
                ]
            } else {
                rgb_values.clone()
            };
            let expected = if ybr {
                vec![
                    0,
                    0,
                    0,
                    max,
                    max,
                    max,
                    max / 4,
                    max / 4,
                    max / 4,
                    max / 2,
                    max / 2,
                    max / 2,
                ]
            } else {
                rgb_values
            };
            for layout in 0..3 {
                let temp = Temp::new();
                let mut object = rgb(false);
                if ybr {
                    object.put_str(Tag(0x0028, 0x0004), VR::CS, "YBR_FULL");
                }
                compress(&mut object, &values, stored, layout);
                let path = temp.0.join("synthetic-interleave.dcm");
                object.write_to_file(&path).unwrap();
                dump_for_native_qa(&object, stored, ybr, layout);
                let study = index_paths(&[path], || false, |_| {}).unwrap();
                let image = study.series[0].read_frame(0, || false).unwrap();
                assert_eq!(
                    &*image.color.as_ref().unwrap().rgb,
                    &expected,
                    "stored={stored} ybr={ybr} layout={layout}"
                );
                assert_eq!(image.color.as_ref().unwrap().info.bits, stored);
            }
        }
    }
}

// Explicit test-only fixture export for native acceptance. Off unless requested;
// only the original synthetic objects from this module can reach this function.
fn dump_for_native_qa(
    object: &dicom_object::DefaultDicomObject,
    stored: u16,
    ybr: bool,
    layout: u8,
) {
    let Some(folder) = std::env::var_os("ORV_TEST_CODEC_QA_DIR").map(PathBuf::from) else {
        return;
    };
    fs::create_dir_all(&folder).unwrap();
    let mut object = object.clone();
    let number = u32::from(stored) * 10 + u32::from(layout) + if ybr { 1000 } else { 0 };
    object.put_str(Tag(0x0008, 0x0018), VR::UI, format!("2.25.18000{number}"));
    object.meta_mut().media_storage_sop_instance_uid = format!("2.25.18000{number}");
    object.put_str(Tag(0x0020, 0x000e), VR::UI, format!("2.25.18100{number}"));
    object.put_str(Tag(0x0020, 0x0011), VR::IS, number.to_string());
    object.meta_mut().update_information_group_length();
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(folder.join(format!(
            "synthetic-jls-{}-{stored}-ilv{layout}.dcm",
            if ybr { "ybr" } else { "rgb" }
        )))
        .unwrap();
    object.write_all(&mut output).unwrap();
    if !ybr && stored == 8 && layout == 2 {
        object.meta_mut().transfer_syntax = "1.2.840.10008.1.2.4.90".into();
        for (tag, value) in [
            (0x0010, 8u16),
            (0x0011, 8),
            (0x0002, 1),
            (0x0100, 16),
            (0x0101, 16),
            (0x0102, 15),
        ] {
            object.put(DataElement::new(
                Tag(0x0028, tag),
                VR::US,
                PrimitiveValue::from(value),
            ));
        }
        object.put_str(Tag(0x0028, 0x0004), VR::CS, "MONOCHROME2");
        object.put_str(Tag(0x0008, 0x0018), VR::UI, "2.25.18000999");
        object.meta_mut().media_storage_sop_instance_uid = "2.25.18000999".into();
        object.put_str(Tag(0x0020, 0x000e), VR::UI, "2.25.18100999");
        object.put_str(Tag(0x0020, 0x0011), VR::IS, "999");
        object.put(DataElement::new(
            Tag(0x7fe0, 0x0010),
            VR::OB,
            dicom_core::DicomValue::PixelSequence(dicom_core::value::PixelFragmentSequence::new(
                Vec::new(),
                vec![include_bytes!("../../../../tests/fixtures/synthetic-u16.j2k").to_vec()],
            )),
        ));
        object.meta_mut().update_information_group_length();
        let mut output = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(folder.join("synthetic-jpeg2000-u16.dcm"))
            .unwrap();
        object.write_all(&mut output).unwrap();
    }
}

#[test]
fn jpegls_scan_layout_crosses_fragment_boundaries_without_using_the_planar_tag() {
    let expected = [255, 1, 2, 3, 251, 5, 6, 7, 247, 127, 63, 31];
    for layout in 0..3 {
        let mut object = rgb(false);
        compress(&mut object, &expected, 8, layout);
        let encoded = object
            .element_by_name("PixelData")
            .unwrap()
            .value()
            .fragments()
            .unwrap()[0]
            .clone();
        let sos = encoded.windows(2).position(|b| b == [0xff, 0xda]).unwrap();
        let split = if (sos + 1).is_multiple_of(2) {
            sos + 1
        } else {
            sos + 2
        };
        let fragments = vec![encoded[..split].to_vec(), encoded[split..].to_vec()];
        object.put(DataElement::new(
            Tag(0x7fe0, 0x0010),
            VR::OB,
            dicom_core::DicomValue::PixelSequence(dicom_core::value::PixelFragmentSequence::new(
                Vec::new(),
                fragments,
            )),
        ));
        let temp = Temp::new();
        let path = temp.0.join("split-header.dcm");
        object.write_to_file(&path).unwrap();
        let study = index_paths(&[path], || false, |_| {}).unwrap();
        let frame = study.series[0].read_frame(0, || false).unwrap();
        assert_eq!(&*frame.color.as_ref().unwrap().rgb, &expected);
    }
}

#[test]
fn multiframe_jpegls_uses_each_frames_own_interleave_mode() {
    let mut object = rgb(false);
    let mut fragments = Vec::new();
    let mut offsets = Vec::new();
    let mut expectations = Vec::new();
    let mut offset = 0u32;
    for layout in 0..3 {
        let values = [
            255,
            1 + u16::from(layout),
            2,
            3,
            251,
            5,
            6,
            7,
            247,
            127,
            63,
            31,
        ];
        compress(&mut object, &values, 8, layout);
        let encoded = object
            .element_by_name("PixelData")
            .unwrap()
            .value()
            .fragments()
            .unwrap()[0]
            .clone();
        offsets.push(offset);
        offset += 8 + encoded.len() as u32;
        fragments.push(encoded);
        expectations.push(values);
    }
    object.put_str(Tag(0x0028, 0x0008), VR::IS, "3");
    object.put(DataElement::new(
        Tag(0x7fe0, 0x0010),
        VR::OB,
        dicom_core::DicomValue::PixelSequence(dicom_core::value::PixelFragmentSequence::new(
            offsets, fragments,
        )),
    ));
    let temp = Temp::new();
    let path = temp.0.join("mixed-layout-frames.dcm");
    object.write_to_file(&path).unwrap();
    let study = index_paths(&[path], || false, |_| {}).unwrap();
    assert_eq!(study.series[0].frame_count(), 3);
    for (index, expected) in expectations.iter().enumerate() {
        let frame = study.series[0].read_frame(index, || false).unwrap();
        assert_eq!(&*frame.color.as_ref().unwrap().rgb, expected);
    }
}
