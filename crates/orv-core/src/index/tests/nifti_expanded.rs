// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use crate::nifti_view::ComplexView;
fn header(
    version: u8,
    big: bool,
    paired: bool,
    dims: [u64; 7],
    datatype: u16,
    bits: u16,
) -> Vec<u8> {
    let mut bytes = vec![0u8; if version == 1 { 352 } else { 544 }];
    let mut int = |at: usize, value: u64, size: usize| {
        for i in 0..size {
            bytes[at + i] = (value >> (if big { size - 1 - i } else { i } * 8)) as u8;
        }
    };
    int(0, if version == 1 { 348 } else { 540 }, 4);
    if version == 1 {
        int(40, 7, 2);
        for (i, d) in dims.into_iter().enumerate() {
            int(42 + 2 * i, d, 2);
        }
        int(70, datatype as u64, 2);
        int(72, bits as u64, 2);
        int(
            108,
            (if paired { 0f32 } else { 352f32 }).to_bits() as u64,
            4,
        );
        int(112, 1f32.to_bits() as u64, 4);
        int(254, 1, 2);
        for (at, v) in [(280, 2f32), (300, 3.), (320, 4.), (292, 10.)] {
            int(at, v.to_bits() as u64, 4);
        }
        bytes[123] = 2;
        bytes[344..348].copy_from_slice(if paired { b"ni1\0" } else { b"n+1\0" });
    } else {
        int(12, datatype as u64, 2);
        int(14, bits as u64, 2);
        int(16, 7, 8);
        for (i, d) in dims.into_iter().enumerate() {
            int(24 + i * 8, d, 8);
        }
        int(168, if paired { 0 } else { 544 }, 8);
        int(176, 1f64.to_bits(), 8);
        int(348, 1, 4);
        int(500, 2, 4);
        for (at, v) in [(400, 2f64), (440, 3.), (480, 4.), (424, 10.)] {
            int(at, v.to_bits(), 8);
        }
        bytes[4..12].copy_from_slice(if paired {
            b"ni2\0\r\n\x1a\n"
        } else {
            b"n+2\0\r\n\x1a\n"
        });
    }
    bytes
}
fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    encoder.write_all(bytes).unwrap();
    encoder.finish().unwrap()
}
#[test]
fn nifti_versions_endian_time_and_vector_axes_are_separate_spatial_volumes() {
    for version in [1, 2] {
        for big in [false, true] {
            for compressed in [false, true] {
                let temp = Temp::new();
                let path = temp.0.join(if compressed {
                    "axis.nii.gz"
                } else {
                    "axis.nii"
                });
                let mut bytes = header(version, big, false, [2, 2, 2, 2, 3, 1, 1], 4, 16);
                for v in 0i16..48 {
                    bytes.extend_from_slice(
                        &(if big {
                            v.to_be_bytes()
                        } else {
                            v.to_le_bytes()
                        }),
                    );
                }
                fs::write(&path, if compressed { gzip(&bytes) } else { bytes }).unwrap();
                let study = index_paths(&[path], || false, |_| {}).unwrap();
                assert_eq!(study.series.len(), 6);
                assert_eq!(study.frame_count(), 12);
                for (i, s) in study.series.iter().enumerate() {
                    let a = s.read_frame(0, || false).unwrap();
                    let b = s.read_frame(1, || false).unwrap();
                    assert_eq!(a.pixels[0], i as f32 * 8.);
                    assert_eq!(b.pixels[0], i as f32 * 8. + 4.);
                    let context = a.nifti.as_ref().unwrap();
                    assert_eq!(context.indices, [(i % 2) as u32, (i / 2) as u32, 0, 0]);
                    assert_eq!(context.dimensions, [2, 3, 1, 1]);
                    assert!(s.label().contains(&format!("Time {}/2", i % 2 + 1)));
                    assert_eq!(a.geometry.as_ref().unwrap().position, [-10., 0., 0.]);
                    assert_eq!(b.geometry.as_ref().unwrap().position, [-10., 0., 4.]);
                    let volume =
                        crate::volume::Volume::new(s.volume_frames(|| false).unwrap()).unwrap();
                    assert_eq!(volume.dimensions(), [2, 2, 2]);
                    assert_eq!(
                        crate::scene::VolumeMetadata::from_volume(&volume).nifti,
                        a.nifti
                    );
                    let plane = volume
                        .reformat(crate::Plane::Axial, [0., 0., 2.], 32, || false)
                        .unwrap();
                    assert_eq!(plane.nifti, a.nifti);
                }
            }
        }
    }
}
#[test]
fn paired_nifti_versions_gzip_zip_and_image_entry_selection_match() {
    for version in [1, 2] {
        for big in [false, true] {
            for compressed in [false, true] {
                for zipped in [false, true] {
                    let temp = Temp::new();
                    let h = header(version, big, true, [2, 2, 2, 1, 1, 1, 1], 4, 16);
                    let mut data = Vec::new();
                    for v in 0i16..8 {
                        data.extend_from_slice(
                            &(if big {
                                v.to_be_bytes()
                            } else {
                                v.to_le_bytes()
                            }),
                        );
                    }
                    let (h, data) = if compressed {
                        (gzip(&h), gzip(&data))
                    } else {
                        (h, data)
                    };
                    let (hn, dn) = if compressed {
                        ("pair.hdr.gz", "pair.img.gz")
                    } else {
                        ("pair.hdr", "pair.img")
                    };
                    let path = if zipped {
                        let path = temp.0.join("pair.zip");
                        let mut z = zip::ZipWriter::new(File::create(&path).unwrap());
                        for (name, bytes) in [
                            (format!("nested/{dn}"), &data),
                            (format!("nested/{hn}"), &h),
                        ] {
                            z.start_file(
                                name,
                                zip::write::SimpleFileOptions::default()
                                    .compression_method(zip::CompressionMethod::Deflated),
                            )
                            .unwrap();
                            z.write_all(bytes).unwrap();
                        }
                        z.finish().unwrap();
                        path
                    } else {
                        fs::write(temp.0.join(hn), &h).unwrap();
                        fs::write(temp.0.join(dn), &data).unwrap();
                        temp.0.join(dn)
                    };
                    let study = index_paths(&[path], || false, |_| {}).unwrap();
                    assert_eq!(study.series.len(), 1);
                    assert_eq!(study.skipped_files, 0);
                    assert_eq!(
                        &*study.series[0].read_frame(1, || false).unwrap().pixels,
                        &[4., 5., 6., 7.]
                    );
                    if !zipped {
                        fs::write(temp.0.join(hn), b"changed header").unwrap();
                        assert_eq!(
                            study.series[0].read_frame(0, || false).unwrap_err(),
                            LoadError::Changed
                        );
                    }
                }
            }
        }
    }
}
#[test]
fn complex_views_scale_both_components_and_keep_phase_units_explicit() {
    for version in [1, 2] {
        for big in [false, true] {
            for datatype in [32, 1792] {
                let temp = Temp::new();
                let path = temp.0.join("complex.nii");
                let mut bytes = header(
                    version,
                    big,
                    false,
                    [2, 2, 2, 1, 1, 1, 1],
                    datatype,
                    if datatype == 32 { 64 } else { 128 },
                );
                let (slope_at, intercept_at) = if version == 1 { (112, 116) } else { (176, 184) };
                if version == 1 {
                    bytes[slope_at..slope_at + 4].copy_from_slice(
                        &(if big {
                            2f32.to_be_bytes()
                        } else {
                            2f32.to_le_bytes()
                        }),
                    );
                    bytes[intercept_at..intercept_at + 4].copy_from_slice(
                        &(if big {
                            (-1f32).to_be_bytes()
                        } else {
                            (-1f32).to_le_bytes()
                        }),
                    );
                } else {
                    bytes[slope_at..slope_at + 8].copy_from_slice(
                        &(if big {
                            2f64.to_be_bytes()
                        } else {
                            2f64.to_le_bytes()
                        }),
                    );
                    bytes[intercept_at..intercept_at + 8].copy_from_slice(
                        &(if big {
                            (-1f64).to_be_bytes()
                        } else {
                            (-1f64).to_le_bytes()
                        }),
                    );
                }
                for _ in 0..8 {
                    for v in [3f64, 4.] {
                        if datatype == 32 {
                            bytes.extend_from_slice(
                                &(if big {
                                    (v as f32).to_be_bytes()
                                } else {
                                    (v as f32).to_le_bytes()
                                }),
                            );
                        } else {
                            bytes.extend_from_slice(
                                &(if big {
                                    v.to_be_bytes()
                                } else {
                                    v.to_le_bytes()
                                }),
                            );
                        }
                    }
                }
                fs::write(&path, bytes).unwrap();
                let study = index_paths(&[path], || false, |_| {}).unwrap();
                for (s, (kind, expected)) in study.series.iter().zip([
                    (ComplexView::Magnitude, 74f32.sqrt()),
                    (ComplexView::PhaseRadians, 7f32.atan2(5.)),
                    (ComplexView::Real, 5.),
                    (ComplexView::Imaginary, 7.),
                ]) {
                    let f = s.read_frame(0, || false).unwrap();
                    assert_eq!(f.nifti.as_ref().unwrap().complex, kind);
                    assert!((f.pixels[0] - expected).abs() < 1e-6);
                    assert_eq!(crate::scene::ImageMetadata::from_frame(&f).nifti, f.nifti);
                }
                assert_eq!(study.series.len(), 4);
            }
        }
    }
}
#[test]
fn malformed_v2_dimensions_magic_offsets_and_missing_pairs_fail_closed() {
    for mutation in 0..5 {
        let temp = Temp::new();
        let path = temp.0.join("bad.nii");
        let mut bytes = header(2, false, false, [2, 2, 2, 1, 1, 1, 1], 4, 16);
        bytes.resize(560, 0);
        match mutation {
            0 => bytes[4] = 0,
            1 => bytes[24..32].copy_from_slice(&u64::MAX.to_le_bytes()),
            2 => bytes[168..176].copy_from_slice(&(-1i64).to_le_bytes()),
            3 => bytes[48..56].copy_from_slice(&100_000u64.to_le_bytes()),
            _ => bytes.truncate(550),
        }
        fs::write(&path, bytes).unwrap();
        assert!(index_paths(&[path], || false, |_| {}).is_err());
    }
    let temp = Temp::new();
    fs::write(
        temp.0.join("missing.hdr"),
        header(1, false, true, [2, 2, 2, 1, 1, 1, 1], 4, 16),
    )
    .unwrap();
    assert!(index_paths(std::slice::from_ref(&temp.0), || false, |_| {}).is_err());
}

#[test]
fn nifti_subvolume_identity_distinguishes_equal_pixels_and_rejects_cross_volume_stacks() {
    let temp = Temp::new();
    let path = temp.0.join("time.nii");
    let mut bytes = header(2, false, false, [2, 2, 2, 2, 1, 1, 1], 4, 16);
    bytes.resize(544 + 32, 0);
    fs::write(&path, bytes).unwrap();
    let study = index_paths(&[path], || false, |_| {}).unwrap();
    assert_ne!(
        crate::scene::series_fingerprint(&study.series[0], || false).unwrap(),
        crate::scene::series_fingerprint(&study.series[1], || false).unwrap()
    );
    let mixed = vec![
        study.series[0].read_frame(0, || false).unwrap(),
        study.series[1].read_frame(1, || false).unwrap(),
    ];
    assert!(crate::volume::Volume::new(mixed).is_err());
}
#[test]
fn paired_v2_sparse_multigib_seeks_to_one_slice_with_a_bounded_cache() {
    let temp = Temp::new();
    let hp = temp.0.join("large.hdr");
    let dp = temp.0.join("large.img");
    fs::write(
        &hp,
        header(2, false, true, [512, 512, 8191, 1, 1, 1, 1], 4, 16),
    )
    .unwrap();
    let mut file = File::create(&dp).unwrap();
    file.set_len(512 * 512 * 8191 * 2).unwrap();
    file.seek(SeekFrom::End(-2)).unwrap();
    file.write_all(&42i16.to_le_bytes()).unwrap();
    drop(file);
    let study = index_paths(&[hp], || false, |_| {}).unwrap();
    let series = &study.series[0];
    assert_eq!(
        *series
            .read_frame(8190, || false)
            .unwrap()
            .pixels
            .last()
            .unwrap(),
        42.
    );
    assert_eq!(
        series.source.as_ref().unwrap().cache_bytes().unwrap(),
        512 * 512 * 4
    );
    assert_eq!(
        series.volume_frames(|| false).unwrap_err(),
        LoadError::Limit
    );
    assert_eq!(
        series.read_frame(0, || true).unwrap_err(),
        LoadError::Cancelled
    );
}
#[test]
fn paired_zip_data_crc_and_ambiguous_companions_are_checked() {
    let temp = Temp::new();
    let path = temp.0.join("bad.zip");
    let mut z = zip::ZipWriter::new(File::create(&path).unwrap());
    z.start_file(
        "pair.hdr",
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored),
    )
    .unwrap();
    z.write_all(&header(1, false, true, [2, 2, 2, 1, 1, 1, 1], 4, 16))
        .unwrap();
    z.start_file(
        "pair.img",
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored),
    )
    .unwrap();
    z.write_all(&[0; 16]).unwrap();
    z.finish().unwrap();
    let mut z = zip::ZipArchive::new(File::open(&path).unwrap()).unwrap();
    let at = z.by_name("pair.img").unwrap().data_start().unwrap();
    drop(z);
    let mut file = File::options().write(true).open(&path).unwrap();
    file.seek(SeekFrom::Start(at)).unwrap();
    file.write_all(&[255]).unwrap();
    drop(file);
    assert!(index_paths(&[path], || false, |_| {}).is_err());
    fs::write(
        temp.0.join("pair.hdr"),
        header(1, false, true, [2, 2, 2, 1, 1, 1, 1], 4, 16),
    )
    .unwrap();
    fs::write(temp.0.join("pair.img"), [0; 16]).unwrap();
    fs::write(temp.0.join("pair.img.gz"), gzip(&[0; 16])).unwrap();
    assert!(index_paths(&[temp.0.join("pair.hdr")], || false, |_| {}).is_err());
}
