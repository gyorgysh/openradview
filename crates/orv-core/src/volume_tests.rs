// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use crate::{
    WindowLevel,
    measurements::{angle_degrees, distance_mm, roi_stats},
};

fn stack() -> Vec<Arc<Frame>> {
    (0..3)
        .map(|z| {
            let pixels = (0..5)
                .flat_map(|y| (0..4).map(move |x| (x + 10 * y + 100 * z) as f32))
                .collect::<Vec<_>>();
            Arc::new(Frame {
                validity: None,
                color: None,
                nifti: None,
                rgba: None,
                width: 4,
                height: 5,
                pixels: pixels.into(),
                geometry: Some(Geometry {
                    nifti_transform: None,
                    frame_of_reference: Some("synthetic-reference".into()),
                    position: [5.0 + z as f64 * 3.0, 10.0, 15.0],
                    orientation: [0., 1., 0., 0., 0., 1.],
                    spacing: [2.0, 0.5],
                }),
                instance: 10 - z,
                frame_index: 0,
                monochrome1: false,
                default_window: WindowLevel {
                    width: 256.0,
                    center: 128.0,
                },
            })
        })
        .collect()
}
fn change(frames: &mut [Arc<Frame>], index: usize, f: impl FnOnce(&mut Frame)) {
    let mut frame = (*frames[index]).clone();
    f(&mut frame);
    frames[index] = Arc::new(frame);
}
fn close(a: f64, b: f64) {
    assert!((a - b).abs() < 1e-6, "{a} != {b}");
}

#[test]
fn rotated_anisotropic_volume_uses_lps_and_trilinear_modality_values() {
    let mut frames = stack();
    frames.reverse();
    let volume = Volume::new(frames).unwrap();
    assert_eq!(volume.dimensions(), [4, 5, 3]);
    assert_eq!(volume.bounds(), [[5.0, 10.0, 15.0], [11.0, 11.5, 23.0]]);
    let p = [9.5, 10.75, 20.0]; // voxel 1.5, 2.5, 1.5 in a permuted-axis volume.
    assert_eq!(volume.world_to_voxel(p), [1.5, 2.5, 1.5]);
    assert_eq!(volume.voxel_to_world([1.5, 2.5, 1.5]), p);
    close(volume.sample(p).unwrap() as f64, 176.5);
    assert!(volume.sample([4.0, 10.0, 15.0]).is_none());
    assert!(volume.sample([f64::NAN, 0., 0.]).is_none());
    assert_eq!(volume.clamp_world([20., 10., 15.]), Some([11., 10., 15.]));
}

#[test]
fn anatomical_reformats_are_world_aligned_with_correct_superior_direction() {
    let volume = Volume::new(stack()).unwrap();
    let point = volume.center();
    for (plane, axis, top) in [
        (Plane::Axial, 2, [1., 0., 0., 0., 1., 0.]),
        (Plane::Coronal, 1, [1., 0., 0., 0., 0., -1.]),
        (Plane::Sagittal, 0, [0., 1., 0., 0., 0., -1.]),
    ] {
        let frame = volume.reformat(plane, point, 128, || false).unwrap();
        let g = frame.geometry.as_ref().unwrap();
        assert_eq!(g.orientation, top);
        close(g.position[axis], point[axis]);
        for y in 0..frame.height {
            for x in 0..frame.width {
                let p = g.pixel_to_world([x as f64, y as f64]).unwrap();
                // Independent analytic phantom in LPS: source index axes are Y, Z, X.
                let expected =
                    (p[1] - 10.0) / 0.5 + 10.0 * (p[2] - 15.0) / 2.0 + 100.0 * (p[0] - 5.0) / 3.0;
                assert!(
                    (frame.pixels[(y * frame.width + x) as usize] as f64 - expected).abs() < 1e-4
                );
            }
        }
        let pixel = g.world_to_pixel(point).unwrap();
        let sample = frame.pixels
            [pixel[1].round() as usize * frame.width as usize + pixel[0].round() as usize];
        assert!(sample.is_finite());
        let world = g.pixel_to_world(pixel).unwrap();
        for axis in 0..3 {
            close(world[axis], point[axis]);
        }
    }
    assert_eq!(
        volume
            .reformat(Plane::Axial, point, 128, || true)
            .unwrap_err(),
        GeometryError::Cancelled
    );
}

#[test]
fn regular_shear_is_preserved_and_irregular_gaps_or_duplicate_planes_are_rejected() {
    let mut frames = stack();
    for z in 0..3 {
        change(&mut frames, z, |f| {
            f.geometry.as_mut().unwrap().position[1] += z as f64 * 0.2
        });
    }
    let volume = Volume::new(frames).unwrap();
    close(volume.sample([9.5, 11.05, 20.0]).unwrap() as f64, 176.5);
    for position in [[5., 10., 15.], [8.5, 10., 15.], [f64::NAN, 10., 15.]] {
        let mut frames = stack();
        change(&mut frames, 1, |f| {
            f.geometry.as_mut().unwrap().position = position
        });
        assert!(Volume::new(frames).is_err());
    }
}

#[test]
fn incomplete_or_mixed_geometry_is_never_silently_used_for_mpr() {
    for mutation in 0..7 {
        let mut frames = stack();
        change(&mut frames, 1, |frame| match mutation {
            0 => frame.geometry = None,
            1 => frame.geometry.as_mut().unwrap().orientation = [0.; 6],
            2 => frame.geometry.as_mut().unwrap().spacing[0] = 3.0,
            3 => frame.geometry.as_mut().unwrap().frame_of_reference = Some("different".into()),
            4 => frame.pixels = vec![1.0].into(),
            5 => frame.geometry.as_mut().unwrap().frame_of_reference = None,
            _ => frame.monochrome1 = true,
        });
        assert!(Volume::new(frames).is_err(), "accepted mutation {mutation}");
    }
    assert_eq!(Volume::new(vec![]).unwrap_err(), GeometryError::TooFew);
}

#[test]
fn measurements_use_physical_spacing_and_roi_reports_modality_units() {
    let frame = &stack()[0];
    let g = frame.geometry.as_ref().unwrap();
    let a = g.pixel_to_world([0., 0.]).unwrap();
    let b = g.pixel_to_world([3., 4.]).unwrap();
    close(
        distance_mm(a, b).unwrap(),
        (1.5f64.powi(2) + 8f64.powi(2)).sqrt(),
    );
    close(
        angle_degrees(
            g.pixel_to_world([1., 0.]).unwrap(),
            a,
            g.pixel_to_world([0., 1.]).unwrap(),
        )
        .unwrap(),
        90.0,
    );
    assert!(angle_degrees(a, a, b).is_err());
    let stats = roi_stats(frame, [[0., 0.], [1., 1.]]).unwrap();
    assert_eq!(stats.count, 4);
    close(stats.mean, 5.5);
    close(stats.standard_deviation, 25.25f64.sqrt());
    assert_eq!(stats.min, 0.0);
    assert_eq!(stats.max, 11.0);
    assert_eq!(stats.area_mm2, Some(4.0));
    assert!(roi_stats(frame, [[-1., 0.], [1., 1.]]).is_err());
    assert!(roi_stats(frame, [[0.1, 0.1], [0.2, 0.2]]).is_err());
}

#[test]
fn source_validity_survives_hashes_mpr_oblique_and_threshold_without_copying_pixels() {
    let mut frames = stack();
    for z in 0..frames.len() {
        change(&mut frames, z, |f| {
            let mut mask = vec![1; f.pixels.len()];
            mask[0] = 0;
            f.validity = Some(mask.into());
        });
    }
    let pixels = frames[0].pixels.clone();
    let volume = Arc::new(Volume::new(frames.clone()).unwrap());
    assert!(Arc::ptr_eq(&pixels, &volume.frames()[0].pixels));
    assert!(volume.has_missing_scalar());
    assert!(volume.sample(volume.voxel_to_world([0., 0., 0.])).is_none());
    assert!(
        volume
            .sample(volume.voxel_to_world([0.5, 0., 0.5]))
            .is_none()
    );
    assert_eq!(volume.sample(volume.voxel_to_world([1., 0., 0.])), Some(1.));
    let point = volume.voxel_to_world([1.5, 2., 1.]);
    for plane in [Plane::Axial, Plane::Coronal, Plane::Sagittal] {
        let image = volume.reformat(plane, point, 32, || false).unwrap();
        assert!(image.valid_dimensions());
        assert!(image.pixels.iter().any(|v| v.is_finite()));
    }
    let g = frames[0].geometry.as_ref().unwrap();
    let plane = crate::oblique::ObliquePlane {
        center_mm: point,
        column_direction: g.directions().0,
        row_direction: g.directions().1,
    };
    let oblique = volume.reformat_oblique(&plane, 32, || false).unwrap();
    assert!(oblique.pixels.iter().any(|v| v.is_nan()));
    assert!(oblique.pixels.iter().any(|v| v.is_finite()));
    let series = crate::study::Series {
        volume: Default::default(),
        key: "synthetic".into(),
        source: None,
        modality: "CT",
        number: 1,
        frames,
    };
    let fingerprint = crate::scene::fingerprint_series(&series, || false).unwrap();
    let mut unmasked = series.clone();
    change(&mut unmasked.frames, 0, |f| f.validity = None);
    assert_ne!(
        fingerprint.source_sha256,
        crate::scene::fingerprint_series(&unmasked, || false)
            .unwrap()
            .source_sha256
    );
    let mut labels = crate::labels::Labels::empty(volume).unwrap();
    labels
        .threshold(None, [-1000., 1000.], 7, || false)
        .unwrap();
    let overlay = labels.plane_rgba(0).unwrap();
    assert_eq!(overlay[3], 0, "padding must not be threshold-labelled");
    assert!(overlay[7] > 0);
}
