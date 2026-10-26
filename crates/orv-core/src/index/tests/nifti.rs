// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
fn image(big: bool) -> Vec<u8> {
    let mut b = vec![0; 352];
    b[..4].copy_from_slice(
        &(if big {
            348u32.to_be_bytes()
        } else {
            348u32.to_le_bytes()
        }),
    );
    for (at, n) in [
        (40, 3u16),
        (42, 2),
        (44, 2),
        (46, 2),
        (70, 4),
        (72, 16),
        (254, 1),
    ] {
        b[at..at + 2].copy_from_slice(
            &(if big {
                n.to_be_bytes()
            } else {
                n.to_le_bytes()
            }),
        );
    }
    for (at, n) in [
        (108, 352f32),
        (112, 2.),
        (116, -100.),
        (280, 2.),
        (296 + 4, 3.),
        (312 + 8, 4.),
        (292, 10.),
        (308, 20.),
        (324, 30.),
    ] {
        b[at..at + 4].copy_from_slice(
            &(if big {
                n.to_be_bytes()
            } else {
                n.to_le_bytes()
            }),
        );
    }
    b[123] = 2;
    b[344..348].copy_from_slice(b"n+1\0");
    for n in -1i16..7 {
        b.extend_from_slice(
            &(if big {
                n.to_be_bytes()
            } else {
                n.to_le_bytes()
            }),
        );
    }
    b
}
fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    encoder.write_all(bytes).unwrap();
    encoder.finish().unwrap()
}
#[test]
fn scalar_nifti_endian_gzip_zip_scaling_and_ras_to_lps_match() {
    for big in [false, true] {
        for container in 0..4 {
            let temp = Temp::new();
            let bytes = image(big);
            let path = temp.0.join(if container >= 2 {
                "research.zip"
            } else if container == 1 {
                "research.nii.gz"
            } else {
                "research.nii"
            });
            if container >= 2 {
                let mut zip = zip::ZipWriter::new(File::create(&path).unwrap());
                zip.start_file(
                    if container == 3 {
                        "research.nii.gz"
                    } else {
                        "research.nii"
                    },
                    zip::write::SimpleFileOptions::default()
                        .compression_method(zip::CompressionMethod::Deflated),
                )
                .unwrap();
                zip.write_all(&if container == 3 { gzip(&bytes) } else { bytes })
                    .unwrap();
                zip.finish().unwrap();
            } else {
                fs::write(&path, if container == 1 { gzip(&bytes) } else { bytes }).unwrap();
            }
            let study = index_paths(&[path], || false, |_| {}).unwrap();
            let series = &study.series[0];
            assert!(series.frames.is_empty());
            assert_eq!(series.frame_count(), 2);
            let a = series.read_frame(0, || false).unwrap();
            let b = series.read_frame(1, || false).unwrap();
            assert_eq!(&a.pixels[..], &[-102., -100., -98., -96.]);
            assert_eq!(&b.pixels[..], &[-94., -92., -90., -88.]);
            let g = a.geometry.as_ref().unwrap();
            assert_eq!(g.position, [-10., -20., 30.]);
            assert_eq!(g.spacing, [3., 2.]);
            assert_eq!(g.orientation, [-1., 0., 0., 0., -1., 0.]);
            let volume =
                crate::volume::Volume::new(series.volume_frames(|| false).unwrap()).unwrap();
            assert_eq!(volume.voxel_to_world([1., 1., 1.]), [-12., -23., 34.]);
            assert!(series.read_frame(0, || true).is_err());
        }
    }
}
#[test]
fn qform_negative_qfac_and_unknown_units_are_not_invented() {
    let temp = Temp::new();
    let path = temp.0.join("qform.nii");
    let mut bytes = image(false);
    bytes[254..256].copy_from_slice(&0u16.to_le_bytes());
    bytes[252..254].copy_from_slice(&1u16.to_le_bytes());
    for (at, n) in [
        (76, -1f32),
        (80, 2.),
        (84, 3.),
        (88, 4.),
        (268, 10.),
        (272, 20.),
        (276, 30.),
    ] {
        bytes[at..at + 4].copy_from_slice(&n.to_le_bytes());
    }
    fs::write(&path, &bytes).unwrap();
    let study = index_paths(std::slice::from_ref(&path), || false, |_| {}).unwrap();
    assert_eq!(
        study.series[0]
            .read_frame(1, || false)
            .unwrap()
            .geometry
            .as_ref()
            .unwrap()
            .position,
        [-10., -20., 26.]
    );
    let volume =
        crate::volume::Volume::new(study.series[0].volume_frames(|| false).unwrap()).unwrap();
    assert_eq!(volume.voxel_to_world([1., 1., 0.]), [-12., -23., 26.]);
    bytes[123] = 0;
    fs::write(&path, &bytes).unwrap();
    let study = index_paths(&[path], || false, |_| {}).unwrap();
    assert!(
        study.series[0]
            .read_frame(0, || false)
            .unwrap()
            .geometry
            .is_none()
    );
    assert!(crate::volume::Volume::new(study.series[0].volume_frames(|| false).unwrap()).is_err());
}
#[test]
fn nifti_refuses_unbacked_dimensions_bad_types_truncation_and_bad_gzip_crc() {
    let temp = Temp::new();
    let path = temp.0.join("unsupported.nii");
    for mutation in 0..4 {
        let mut bytes = image(false);
        match mutation {
            0 => {
                bytes[40..42].copy_from_slice(&4u16.to_le_bytes());
                bytes[48..50].copy_from_slice(&2u16.to_le_bytes());
            }
            1 => bytes[70..72].copy_from_slice(&128u16.to_le_bytes()),
            2 => {
                bytes.pop();
            }
            _ => bytes[344..348].copy_from_slice(b"ni1\0"),
        }
        fs::write(&path, bytes).unwrap();
        assert!(index_paths(std::slice::from_ref(&path), || false, |_| {}).is_err());
    }
    let path = temp.0.join("broken.nii.gz");
    let mut bytes = gzip(&image(false));
    let at = bytes.len() - 8;
    bytes[at] ^= 1;
    fs::write(&path, bytes).unwrap();
    assert!(index_paths(&[path], || false, |_| {}).is_err());
}
#[test]
fn nifti_multigib_volume_uses_metadata_and_one_frame_not_whole_volume() {
    let temp = Temp::new();
    let path = temp.0.join("large.nii");
    let mut header = image(false);
    header.truncate(352);
    for (at, n) in [(42, 512u16), (44, 512), (46, 8191)] {
        header[at..at + 2].copy_from_slice(&n.to_le_bytes());
    }
    let mut file = File::create(&path).unwrap();
    file.write_all(&header).unwrap();
    file.set_len(352 + 512 * 512 * 2 * 8191).unwrap();
    file.seek(SeekFrom::End(-2)).unwrap();
    file.write_all(&42i16.to_le_bytes()).unwrap();
    drop(file);
    let study = index_paths(&[path], || false, |_| {}).unwrap();
    let series = &study.series[0];
    assert_eq!(series.frame_count(), 8191);
    assert!(series.frames.is_empty());
    assert_eq!(
        *series
            .read_frame(8190, || false)
            .unwrap()
            .pixels
            .last()
            .unwrap(),
        -16.
    );
    assert_eq!(
        series.volume_frames(|| false).unwrap_err(),
        LoadError::Limit
    );
}

#[test]
fn all_advertised_scalar_types_decode_in_both_endian_orders_and_nonfinite_values_fail() {
    for big in [false, true] {
        for (datatype, bits) in [
            (2u16, 8u16),
            (256, 8),
            (4, 16),
            (512, 16),
            (8, 32),
            (768, 32),
            (16, 32),
            (64, 64),
        ] {
            let temp = Temp::new();
            let path = temp.0.join("scalar.nii");
            let mut bytes = image(big);
            bytes.truncate(352);
            bytes[70..72].copy_from_slice(
                &(if big {
                    datatype.to_be_bytes()
                } else {
                    datatype.to_le_bytes()
                }),
            );
            bytes[72..74].copy_from_slice(
                &(if big {
                    bits.to_be_bytes()
                } else {
                    bits.to_le_bytes()
                }),
            );
            for n in 0i64..8 {
                let mut value = match datatype {
                    2 | 256 => vec![n as u8],
                    4 => (n as i16).to_le_bytes().to_vec(),
                    512 => (n as u16).to_le_bytes().to_vec(),
                    8 => (n as i32).to_le_bytes().to_vec(),
                    768 => (n as u32).to_le_bytes().to_vec(),
                    16 => (n as f32).to_le_bytes().to_vec(),
                    64 => (n as f64).to_le_bytes().to_vec(),
                    _ => unreachable!(),
                };
                if big {
                    value.reverse();
                }
                bytes.extend_from_slice(&value);
            }
            fs::write(&path, &bytes).unwrap();
            let study = index_paths(std::slice::from_ref(&path), || false, |_| {}).unwrap();
            assert_eq!(
                &study.series[0].read_frame(1, || false).unwrap().pixels[..],
                &[-92., -90., -88., -86.]
            );
            if datatype == 16 {
                bytes[352..356].copy_from_slice(
                    &(if big {
                        f32::NAN.to_be_bytes()
                    } else {
                        f32::NAN.to_le_bytes()
                    }),
                );
                fs::write(&path, bytes).unwrap();
                let study = index_paths(&[path], || false, |_| {}).unwrap();
                assert_eq!(
                    study.series[0].read_frame(0, || false).unwrap_err(),
                    LoadError::Unsupported
                );
            }
        }
    }
}

#[test]
fn nifti_reference_space_changes_identity_and_survives_reformat_metadata() {
    let temp = Temp::new();
    let path = temp.0.join("reference.nii");
    let mut bytes = image(false);
    fs::write(&path, &bytes).unwrap();
    let study = index_paths(std::slice::from_ref(&path), || false, |_| {}).unwrap();
    let before = crate::scene::series_fingerprint(&study.series[0], || false).unwrap();
    bytes[254..256].copy_from_slice(&4u16.to_le_bytes());
    fs::write(&path, bytes).unwrap();
    let study = index_paths(&[path], || false, |_| {}).unwrap();
    let series = &study.series[0];
    assert_ne!(
        before,
        crate::scene::series_fingerprint(series, || false).unwrap()
    );
    let volume = crate::volume::Volume::new(series.volume_frames(|| false).unwrap()).unwrap();
    let metadata = crate::scene::VolumeMetadata::from_volume(&volume);
    assert_eq!(metadata.nifti_transform.unwrap().code, 4);
    let frame = volume
        .reformat(crate::Plane::Axial, volume.center(), 32, || false)
        .unwrap();
    assert_eq!(
        crate::scene::ImageMetadata::from_frame(&frame)
            .geometry
            .unwrap()
            .nifti_transform
            .unwrap()
            .code,
        4
    );
}

#[test]
fn quaternion_rotation_keeps_the_physical_axes() {
    let temp = Temp::new();
    let path = temp.0.join("rotated.nii");
    let mut bytes = image(false);
    bytes[254..256].copy_from_slice(&0u16.to_le_bytes());
    bytes[252..254].copy_from_slice(&1u16.to_le_bytes());
    for (at, n) in [
        (76, 1f32),
        (80, 2.),
        (84, 3.),
        (88, 4.),
        (264, std::f32::consts::FRAC_1_SQRT_2),
        (268, 10.),
        (272, 20.),
        (276, 30.),
    ] {
        bytes[at..at + 4].copy_from_slice(&n.to_le_bytes());
    }
    fs::write(&path, bytes).unwrap();
    let study = index_paths(&[path], || false, |_| {}).unwrap();
    let volume =
        crate::volume::Volume::new(study.series[0].volume_frames(|| false).unwrap()).unwrap();
    for (actual, expected) in volume
        .voxel_to_world([1., 1., 1.])
        .iter()
        .zip([-7., -22., 34.])
    {
        assert!((actual - expected).abs() < 1e-6);
    }
}
