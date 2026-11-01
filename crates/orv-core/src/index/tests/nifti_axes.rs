// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use crate::{
    acquisition::{NiftiCatalog, Sampling, SelectionError},
    nifti_view::ComplexView,
};
#[path = "../../../tests/support/nifti_axes.rs"]
mod phantom;

#[test]
fn imported_versions_endianness_and_all_axis_tuples_resolve_to_independent_known_samples() {
    for version in [1, 2] {
        for big in [false, true] {
            let dir = Temp::new();
            let path = dir.0.join("axes.nii");
            std::fs::write(&path, phantom::fixture(version, big, true, 18, 250., -500.)).unwrap();
            let study = index_paths(&[path], || false, |_| {}).unwrap();
            assert_eq!(study.series.len(), 96);
            let catalog = NiftiCatalog::prepare(&study, || false).unwrap();
            for a7 in 0..2 {
                for a6 in 0..2 {
                    for component in 0..2 {
                        for time in 0..3 {
                            for (complex, increment) in
                                [(ComplexView::Real, 0.), (ComplexView::Imaginary, 50000.)]
                            {
                                let index = catalog
                                    .resolve(0, [time, component, a6, a7], complex)
                                    .unwrap();
                                let series = &study.series[index];
                                assert!(series.frames.is_empty());
                                assert_eq!(
                                    catalog.axes(index).unwrap().coordinate(),
                                    Some(-0.5 + f64::from(time) * 0.25)
                                );
                                for z in 0..3 {
                                    let frame = series.read_frame(z, || false).unwrap();
                                    assert_eq!(
                                        frame.pixels[0],
                                        (a7 * 10000
                                            + a6 * 1000
                                            + component * 100
                                            + time * 10
                                            + z as u32)
                                            as f32
                                            + increment
                                    );
                                    assert_eq!(
                                        frame.nifti.as_ref().unwrap().indices,
                                        [time, component, a6, a7]
                                    );
                                    assert_eq!(
                                        frame.geometry.as_ref().unwrap().position[2],
                                        z as f64 * 3.
                                    );
                                }
                            }
                        }
                    }
                }
            }
            assert_eq!(
                catalog.resolve(0, [3, 0, 0, 0], ComplexView::Real),
                Err(SelectionError::OutOfRange)
            );
            assert_eq!(
                catalog.resolve(0, [0; 4], ComplexView::Scalar),
                Err(SelectionError::Unavailable)
            );
            assert!(matches!(
                NiftiCatalog::prepare(&study, || true),
                Err(crate::load::LoadError::Cancelled)
            ));
        }
    }
}
#[test]
fn identical_inputs_keep_separate_families_and_ambiguous_or_missing_tuples_are_not_guessed() {
    let dir = Temp::new();
    let paths = [dir.0.join("first.nii"), dir.0.join("second.nii")];
    for path in &paths {
        std::fs::write(path, phantom::fixture(1, false, false, 18, 250., -500.)).unwrap();
    }
    let mut study = index_paths(&paths, || false, |_| {}).unwrap();
    let catalog = NiftiCatalog::prepare(&study, || false).unwrap();
    let first = catalog
        .resolve(0, [2, 1, 1, 1], ComplexView::Scalar)
        .unwrap();
    let second = catalog
        .resolve(1, [2, 1, 1, 1], ComplexView::Scalar)
        .unwrap();
    assert_eq!((first, second), (46, 47));
    study.series.push(study.series[46].clone());
    let catalog = NiftiCatalog::prepare(&study, || false).unwrap();
    assert_eq!(
        catalog.resolve(0, [2, 1, 1, 1], ComplexView::Scalar),
        Err(SelectionError::Ambiguous)
    );
    assert_eq!(
        catalog.resolve(1, [2, 1, 1, 1], ComplexView::Scalar),
        Ok(47)
    );
    study.series.remove(2);
    let catalog = NiftiCatalog::prepare(&study, || false).unwrap();
    assert_eq!(
        catalog.resolve(0, [1, 0, 0, 0], ComplexView::Scalar),
        Err(SelectionError::Unavailable)
    );
}

#[test]
fn optional_timing_does_not_refuse_pixels_or_change_existing_source_fingerprints() {
    let dir = Temp::new();
    let mut expected = None;
    for (number, (units, step, offset)) in [
        (18, 250., -500.),
        (2, 0., f64::NAN),
        (18, -1., 0.),
        (34, 5., -10.),
        (58, 1., 0.),
    ]
    .into_iter()
    .enumerate()
    {
        let path = dir.0.join(format!("timing-{number}.nii"));
        fs::write(
            &path,
            phantom::fixture(2, false, false, units, step, offset),
        )
        .unwrap();
        let study = index_paths(&[path], || false, |_| {}).unwrap();
        let series = &study.series[0];
        assert_eq!(series.read_frame(2, || false).unwrap().pixels[0], 2.);
        let hash = crate::scene::series_fingerprint(series, || false).unwrap();
        if let Some(expected) = &expected {
            assert_eq!(expected, &hash);
        } else {
            expected = Some(hash);
        }
        assert_eq!(series.nifti_axes().unwrap().view().indices, [0; 4]);
    }
    let path = dir.0.join("precise-v2.nii");
    fs::write(
        &path,
        phantom::fixture(2, true, false, 10, 0.125, 16777217.25),
    )
    .unwrap();
    let study = index_paths(&[path], || false, |_| {}).unwrap();
    assert_eq!(
        study.series[2].nifti_axes().unwrap().coordinate(),
        Some(16777217.5)
    );
    assert_eq!(
        study.series[2].nifti_axes().unwrap().sampling(),
        Sampling::Seconds {
            step: 0.125,
            offset: 16777217.25
        }
    );
}

#[test]
fn navigation_metadata_has_bounded_members_and_retains_no_source_or_pixel_capabilities() {
    let dir = Temp::new();
    let path = dir.0.join("metadata.nii");
    fs::write(&path, phantom::fixture(1, false, false, 18, 250., -500.)).unwrap();
    let study = index_paths(&[path], || false, |_| {}).unwrap();
    let source = Arc::downgrade(study.series[0].source.as_ref().unwrap());
    let image = study.series[0].read_frame(0, || false).unwrap();
    let pixels = Arc::downgrade(&image);
    let catalog = NiftiCatalog::prepare(&study, || false).unwrap();
    drop(image);
    drop(study);
    assert!(source.upgrade().is_none() && pixels.upgrade().is_none());
    assert!(
        catalog
            .resolve(0, [2, 1, 1, 1], ComplexView::Scalar)
            .is_ok()
    );
    let path = dir.0.join("limit.nii");
    fs::write(&path, phantom::fixture(1, false, false, 18, 250., -500.)).unwrap();
    let mut study = index_paths(&[path], || false, |_| {}).unwrap();
    while study.series.len() < 4097 {
        study.series.push(study.series[0].clone());
    }
    assert!(matches!(
        NiftiCatalog::prepare(&study, || false),
        Err(LoadError::Limit)
    ));
    let calls = std::cell::Cell::new(0);
    assert!(matches!(
        NiftiCatalog::prepare(&study, || {
            calls.set(calls.get() + 1);
            calls.get() > 1
        }),
        Err(LoadError::Cancelled)
    ));
}
