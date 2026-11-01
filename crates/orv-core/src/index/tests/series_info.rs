// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use crate::{inspection::series_info, spatial::GeometryError};
use dicom_core::{Tag, VR};
use std::io::Cursor;

fn write_plane(path: &Path, z: f64, x: f64, flags: Option<(&str, &str)>, reference: Option<&str>) {
    let mut object =
        dicom_object::from_reader(Cursor::new(support::fixture(z, 1, false, false))).unwrap();
    object.put_str(Tag(0x0020, 0x0032), VR::DS, format!("{x}\\0\\{z}"));
    if let Some((lossy, image_type)) = flags {
        object.put_str(Tag(0x0028, 0x2110), VR::CS, lossy);
        object.put_str(Tag(0x0008, 0x0008), VR::CS, image_type);
    }
    if let Some(reference) = reference {
        object.put_str(Tag(0x0020, 0x0052), VR::UI, reference);
    }
    object.write_to_file(path).unwrap();
}

#[test]
fn series_header_information_preserves_unknown_quality_and_never_decodes_pixels() {
    let temp = Temp::new();
    for (index, flags) in [
        Some(("01", "ORIGINAL\\PRIMARY")),
        Some(("00", "ORIGINAL\\PRIMARY")),
        Some(("", "ORIGINAL\\PRIMARY")),
    ]
    .into_iter()
    .enumerate()
    {
        write_plane(
            &temp.0.join(format!("plane-{index}.dcm")),
            index as f64 * 2.,
            0.,
            flags,
            None,
        );
    }
    let study = index_paths(std::slice::from_ref(&temp.0), || false, |_| {}).unwrap();
    let series = &study.series[0];
    let info = series_info(series, || false).unwrap();
    assert_eq!(info.frames, 3);
    assert_eq!(info.decoded_bytes, 48);
    assert_eq!(
        [info.lossy.yes, info.lossy.no, info.lossy.unknown],
        [1, 1, 1]
    );
    assert_eq!(
        [info.derived.yes, info.derived.no, info.derived.unknown],
        [0, 3, 0]
    );
    let grid = info.geometry.as_ref().unwrap();
    assert_eq!(grid.dimensions, [2, 2, 3]);
    assert_eq!(grid.steps_mm, [1., 2., 2.]);
    assert!(!grid.tilted);
    assert_eq!(series.source.as_ref().unwrap().cache_bytes(), Some(0));
    for sensitive in ["plane-", "2.25.", "Synthetic", "PRIMARY"] {
        assert!(!format!("{info:?}").contains(sensitive));
    }
    assert_eq!(
        series_info(series, || true).unwrap_err(),
        GeometryError::Cancelled
    );
    let calls = std::cell::Cell::new(0);
    assert_eq!(
        series_info(series, || {
            calls.set(calls.get() + 1);
            calls.get() > 2
        })
        .unwrap_err(),
        GeometryError::Cancelled
    );
}

#[test]
fn different_source_derivations_remain_separate_acquisitions() {
    let temp = Temp::new();
    for (index, flags) in [
        Some(("01", "DERIVED\\PRIMARY")),
        Some(("00", "ORIGINAL\\PRIMARY")),
        None,
    ]
    .into_iter()
    .enumerate()
    {
        write_plane(
            &temp.0.join(format!("plane-{index}.dcm")),
            index as f64,
            0.,
            flags,
            None,
        );
    }
    let study = index_paths(std::slice::from_ref(&temp.0), || false, |_| {}).unwrap();
    assert_eq!(study.series.len(), 3);
    let infos = study
        .series
        .iter()
        .map(|series| series_info(series, || false).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(infos.iter().map(|info| info.derived.yes).sum::<usize>(), 1);
    assert_eq!(infos.iter().map(|info| info.derived.no).sum::<usize>(), 1);
    assert_eq!(
        infos.iter().map(|info| info.derived.unknown).sum::<usize>(),
        1
    );
    assert!(
        infos
            .iter()
            .all(|info| info.frames == 1 && info.geometry == Err(GeometryError::TooFew))
    );
    assert!(
        study
            .series
            .iter()
            .all(|series| series.source.as_ref().unwrap().cache_bytes() == Some(0))
    );
}

#[test]
fn source_geometry_check_uses_volume_tolerance_and_preserves_regular_tilt() {
    for tilted in [false, true] {
        let temp = Temp::new();
        for (index, z) in [0., 1.009, 2.].into_iter().enumerate() {
            write_plane(
                &temp.0.join(format!("plane-{index}.dcm")),
                z,
                if tilted { z * 0.25 } else { 0. },
                None,
                None,
            );
        }
        let study = index_paths(std::slice::from_ref(&temp.0), || false, |_| {}).unwrap();
        let info = series_info(&study.series[0], || false).unwrap();
        let grid = info.geometry.unwrap();
        assert_eq!(grid.tilted, tilted);
        let expected_step = if tilted { 1.0625f64.sqrt() } else { 1. };
        assert!((grid.steps_mm[2] - expected_step).abs() < 1e-12);
        assert_eq!(
            study.series[0].source.as_ref().unwrap().cache_bytes(),
            Some(0)
        );
        let volume = study.series[0].volume(|| false).unwrap();
        let expected = [if tilted { 1.5 } else { 1. }, 2., 2.];
        for (actual, expected) in volume
            .voxel_to_world([1., 1., 2.])
            .into_iter()
            .zip(expected)
        {
            assert!((actual - expected).abs() < 1e-12);
        }
    }
}

#[test]
fn geometry_refusals_do_not_require_or_remove_source_pixel_access() {
    for (positions, expected) in [
        (vec![0.], GeometryError::TooFew),
        (vec![0., 1., 4.], GeometryError::Irregular),
        (vec![0., 0.], GeometryError::Irregular),
    ] {
        let temp = Temp::new();
        for (index, z) in positions.into_iter().enumerate() {
            write_plane(
                &temp.0.join(format!("plane-{index}.dcm")),
                z,
                0.,
                None,
                None,
            );
        }
        let study = index_paths(std::slice::from_ref(&temp.0), || false, |_| {}).unwrap();
        let series = &study.series[0];
        assert_eq!(
            series_info(series, || false).unwrap().geometry,
            Err(expected)
        );
        assert_eq!(series.source.as_ref().unwrap().cache_bytes(), Some(0));
        assert_eq!(
            &series.read_frame(0, || false).unwrap().pixels[..],
            &[-100., 0., 100., -102.]
        );
    }
}

#[test]
fn consistent_geometry_does_not_claim_consistent_pixel_units() {
    let temp = Temp::new();
    for (index, unit) in ["HU", "OD"].into_iter().enumerate() {
        let mut object = dicom_object::from_reader(Cursor::new(support::fixture(
            index as f64,
            index as i32,
            false,
            false,
        )))
        .unwrap();
        object.put_str(Tag(0x0028, 0x1054), VR::LO, unit);
        object
            .write_to_file(temp.0.join(format!("plane-{index}.dcm")))
            .unwrap();
    }
    let study = index_paths(std::slice::from_ref(&temp.0), || false, |_| {}).unwrap();
    assert!(
        series_info(&study.series[0], || false)
            .unwrap()
            .geometry
            .is_ok()
    );
    assert!(
        study.series[0]
            .volume(|| false)
            .unwrap_err()
            .contains("consistent intensity")
    );
}
