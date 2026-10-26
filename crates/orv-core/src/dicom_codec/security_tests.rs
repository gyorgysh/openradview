// SPDX-License-Identifier: GPL-3.0-or-later
//! Original synthetic regression inputs; native decode runs in a bounded child.
use charls::{CharLS, FrameInfo, InterleaveMode};
use std::{
    process::{Command, Stdio},
    time::{Duration, Instant},
};

fn encoded(mode: InterleaveMode) -> Vec<u8> {
    let mut codec = CharLS::default();
    codec.set_interleave_mode(mode).unwrap();
    codec
        .encode(
            FrameInfo {
                width: 64,
                height: 4,
                bits_per_sample: 8,
                component_count: 3,
            },
            0,
            &(0..768).map(|n| (n % 251) as u8).collect::<Vec<_>>(),
        )
        .unwrap()
}

fn oversized_dimensions_are_blocked_before_native_decode() {
    use dicom_core::{DataElement, DicomValue, PrimitiveValue, Tag, VR};
    let mut data = encoded(InterleaveMode::None);
    let sof = data.windows(2).position(|b| b == [0xff, 0xf7]).unwrap();
    data[sof + 5..sof + 9].fill(0);
    // A standards-shaped LSE ID4 gives dimensions 99,999 x 99,999 while
    // SOF dimensions are zero. Read header only: never allocate/decode it.
    let after_sof = sof + 2 + usize::from(u16::from_be_bytes([data[sof + 2], data[sof + 3]]));
    data.splice(
        after_sof..after_sof,
        [0xff, 0xf8, 0, 12, 4, 4, 0, 1, 0x86, 0x9f, 0, 1, 0x86, 0x9f],
    );
    let info = CharLS::default().get_frame_info(&data).unwrap();
    assert_eq!((info.width, info.height), (99_999, 99_999));
    let mut object = dicom_object::from_reader(std::io::Cursor::new(crate::test_support::fixture(
        0., 1, false, false,
    )))
    .unwrap();
    object.meta_mut().transfer_syntax = "1.2.840.10008.1.2.4.80".into();
    for (tag, value) in [(0x0010, 4u16), (0x0011, 64), (0x0002, 3), (0x0101, 8)] {
        object.put(DataElement::new(
            Tag(0x0028, tag),
            VR::US,
            PrimitiveValue::from(value),
        ));
    }
    object.put(DataElement::new(
        Tag(0x7fe0, 0x0010),
        VR::OB,
        DicomValue::PixelSequence(dicom_core::value::PixelFragmentSequence::new(
            Vec::new(),
            vec![data],
        )),
    ));
    assert!(super::validate(&object).is_err());
}

#[test]
fn native_scan_corruption_and_truncation_are_bounded() {
    const CHILD: &str = "ORV_JPEGLS_REGRESSION_CHILD";
    if std::env::var_os(CHILD).is_some() {
        oversized_dimensions_are_blocked_before_native_decode();
        let planar = encoded(InterleaveMode::None);
        assert_eq!(CharLS::default().decode(&planar).unwrap().len(), 768);
        let scans = planar
            .windows(2)
            .enumerate()
            .filter_map(|(i, b)| (b == [0xff, 0xda]).then_some(i))
            .collect::<Vec<_>>();
        assert_eq!(scans.len(), 3);
        for mode in [InterleaveMode::Line, InterleaveMode::Sample] {
            let packed = encoded(mode);
            let scan = packed.windows(2).position(|b| b == [0xff, 0xda]).unwrap();
            let mut mixed = planar[..scans[2]].to_vec();
            mixed.extend_from_slice(&packed[scan..]);
            // First two single-component scans followed by an interleaved
            // three-component scan: GHSA-vx55-856r-qqxw's layout violation.
            assert!(CharLS::default().decode(&mixed).is_err());
        }
        for cut in [
            8,
            planar.len() / 3,
            planar.len() / 2,
            planar.len() - 3,
            planar.len() - 1,
        ] {
            assert!(CharLS::default().decode(&planar[..cut]).is_err());
        }
        println!("ORV_JPEGLS_CHILD_COMPLETED");
        return;
    }
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "dicom_codec::security_tests::native_scan_corruption_and_truncation_are_bounded",
            "--nocapture",
        ])
        .env(CHILD, "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            let output = child.wait_with_output().unwrap();
            assert!(
                status.success(),
                "Native JPEG-LS regression failed: {status}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(String::from_utf8_lossy(&output.stdout).contains("ORV_JPEGLS_CHILD_COMPLETED"));
            break;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("Native JPEG-LS decode exceeded the regression deadline");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn native_jpeg2000_truncation_and_packet_mutations_are_bounded() {
    const CHILD: &str = "ORV_JPEG2000_REGRESSION_CHILD";
    if std::env::var_os(CHILD).is_some() {
        for source in [
            include_bytes!("../../tests/fixtures/synthetic-u16.j2k").as_slice(),
            include_bytes!("../../tests/fixtures/synthetic-rgb.j2k").as_slice(),
            include_bytes!("../../tests/fixtures/synthetic-rgb16.j2k").as_slice(),
            include_bytes!("../../tests/fixtures/synthetic-u16-lossy.j2k").as_slice(),
        ] {
            let image = jpeg2k::Image::from_bytes(source).unwrap();
            assert!(image.width() > 0 && image.height() > 0);
            for cut in [8, 42, source.len() / 2, source.len() - 2, source.len() - 1] {
                assert!(
                    jpeg2k::Image::from_bytes(&source[..cut]).is_err(),
                    "Strict decode accepted truncated source at {cut}"
                );
            }
            let cod = source.windows(2).position(|b| b == [0xff, 0x52]).unwrap();
            let mut invalid_levels = source.to_vec();
            invalid_levels[cod + 9] = 255;
            assert!(jpeg2k::Image::from_bytes(&invalid_levels).is_err());
            let sot = source.windows(2).position(|b| b == [0xff, 0x90]).unwrap();
            let mut invalid_part = source.to_vec();
            invalid_part[sot + 10] = 255;
            invalid_part[sot + 11] = 1;
            assert!(jpeg2k::Image::from_bytes(&invalid_part).is_err());
            // A large packet iteration count with no tile payload must not
            // keep revisiting an exhausted packet header source. Exercise the
            // otherwise unused tolerant mode too, for the native loop fix.
            let mut empty = source[..sot + 12].to_vec();
            empty[cod + 6..cod + 8].copy_from_slice(&65_535u16.to_be_bytes());
            empty[sot + 6..sot + 10].copy_from_slice(&14u32.to_be_bytes());
            empty.extend_from_slice(&[0xff, 0x93, 0xff, 0xd9]);
            assert!(jpeg2k::Image::from_bytes(&empty).is_err());
            let _ = jpeg2k::Image::from_bytes_with(
                &empty,
                jpeg2k::DecodeParameters::default().strict(false),
            );
        }
        println!("ORV_JPEG2000_CHILD_COMPLETED");
        return;
    }
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "dicom_codec::security_tests::native_jpeg2000_truncation_and_packet_mutations_are_bounded", "--nocapture"])
        .env(CHILD,"1").stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            let output = child.wait_with_output().unwrap();
            assert!(
                status.success(),
                "Native JPEG 2000 regression failed: {status}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(
                String::from_utf8_lossy(&output.stdout).contains("ORV_JPEG2000_CHILD_COMPLETED")
            );
            break;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("Native JPEG 2000 decode exceeded the regression deadline");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
