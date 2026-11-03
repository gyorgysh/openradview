// SPDX-License-Identifier: GPL-3.0-or-later
use orv_core::{
    WindowLevel,
    curved::{Curve, CurvedError, reformat},
    study::{Frame, Geometry},
    volume::Volume,
};
use std::sync::Arc;

/// Sheared, anisotropic 9x9x9 grid holding an affine world field, so trilinear
/// sampling reproduces the field exactly wherever a sample exists.
fn field(p: [f64; 3]) -> f64 {
    p[0] + 2. * p[1] - 3. * p[2] - 100.
}
fn phantom(missing: bool) -> Volume {
    Volume::new(
        (0..9)
            .map(|z| {
                let geometry = Geometry {
                    nifti_transform: None,
                    frame_of_reference: Some("generated-curve".into()),
                    position: [0.2 * f64::from(z), 0.1 * f64::from(z), f64::from(z) * 2.],
                    orientation: [1., 0., 0., 0., 1., 0.],
                    spacing: [1.5, 0.5],
                };
                let pixels = (0..9)
                    .flat_map(|y| {
                        (0..9).map(move |x| {
                            field([
                                0.5 * f64::from(x) + 0.2 * f64::from(z),
                                1.5 * f64::from(y) + 0.1 * f64::from(z),
                                2. * f64::from(z),
                            ]) as f32
                        })
                    })
                    .collect::<Vec<_>>();
                Arc::new(Frame {
                    presentation: None,
                    calibration: None,
                    slab: None,
                    scalar: None,
                    validity: missing.then(|| vec![0; 81].into()),
                    color: None,
                    nifti: None,
                    rgba: None,
                    width: 9,
                    height: 9,
                    pixels: pixels.into(),
                    geometry: Some(geometry),
                    instance: z,
                    frame_index: 0,
                    monochrome1: false,
                    default_window: WindowLevel::SOFT_TISSUE,
                })
            })
            .collect(),
    )
    .unwrap()
}
const UP: [f64; 3] = [0., 0., 1.];
fn inside(p: [f64; 3]) -> bool {
    let z = p[2] / 2.;
    let x = (p[0] - 0.2 * z) / 0.5;
    let y = (p[1] - 0.1 * z) / 1.5;
    [x, y, z].iter().all(|v| (-1e-9..=8. + 1e-9).contains(v))
}
fn check(volume: &Volume, curve: &Curve) -> usize {
    let image = reformat(volume, curve, &|| false).unwrap();
    let [columns, rows] = curve.size();
    assert_eq!((image.frame.width, image.frame.height), (columns, rows));
    assert!(image.frame.geometry.is_none());
    let mut present = 0;
    for row in 0..rows {
        for column in 0..columns {
            let p = curve.source_position(column, row).unwrap();
            let value = image.frame.pixels[(row * columns + column) as usize];
            if inside(p) {
                present += 1;
                assert!((f64::from(value) - field(p)).abs() < 1e-4, "{column},{row}");
            } else {
                assert!(value.is_nan(), "outside {column},{row}");
            }
        }
    }
    present
}

#[test]
fn straight_and_circular_curves_sample_the_independent_field_at_exact_positions() {
    let volume = phantom(false);
    // Straight: along x at mid depth, rows along z.
    let straight = Curve::new(&[[1., 6., 8.], [4., 6., 8.]], UP, 2., 0.5).unwrap();
    assert_eq!(straight.size(), [7, 9]);
    assert_eq!(straight.source_position(2, 4), Some([2., 6., 8.]));
    assert_eq!(straight.source_position(2, 0), Some([2., 6., 6.]));
    assert!(straight.source_position(7, 0).is_none());
    assert_eq!(check(&volume, &straight), 7 * 9);
    // Circle of radius 2 mm in the axial plane, 64 segments.
    let circle: Vec<[f64; 3]> = (0..=64)
        .map(|i| {
            let a = f64::from(i) / 64. * std::f64::consts::TAU;
            [2.5 + 2. * a.cos(), 6. + 2. * a.sin(), 8.]
        })
        .collect();
    let curve = Curve::new(&circle, UP, 1., 0.25).unwrap();
    let chord = 2. * 2. * (std::f64::consts::PI / 64.).sin();
    assert!((curve.length_mm() - 64. * chord).abs() < 1e-9);
    for column in 0..curve.size()[0] {
        let p = curve.source_position(column, 4).unwrap();
        // Points lie on the chords: between the inscribed polygon and circle.
        let radius = ((p[0] - 2.5).powi(2) + (p[1] - 6.).powi(2)).sqrt();
        assert!((2. * (std::f64::consts::PI / 64.).cos() - 1e-9..=2. + 1e-9).contains(&radius));
        assert!((p[2] - 8.).abs() < 1e-12);
    }
    assert!(check(&volume, &curve) > 0);
}

#[test]
fn reversal_inflections_repeats_and_missing_data_follow_the_convention() {
    let volume = phantom(false);
    let points = [[1., 3., 6.], [2., 5., 7.], [1.5, 7., 8.], [3., 8., 9.]];
    let forward = Curve::new(&points, UP, 1., 0.5).unwrap();
    assert!(check(&volume, &forward) > 0);
    let mut reversed_points = points;
    reversed_points.reverse();
    let reversed = Curve::new(&reversed_points, UP, 1., 0.5).unwrap();
    assert!((forward.length_mm() - reversed.length_mm()).abs() < 1e-12);
    // Arc length runs from the other end; rows keep the reference direction.
    let last = reversed.size()[0] - 1;
    let tail = forward.length_mm() - f64::from(last) * 0.5;
    let a = forward.source_position(last, 0).unwrap();
    let b = reversed.source_position(0, 0).unwrap();
    assert!((0. ..0.5).contains(&tail));
    assert!((a[2] - b[2]).abs() < 1.);
    // Repeated points leave the same curve and identity.
    let repeated = Curve::new(
        &[
            points[0], points[0], points[1], points[2], points[2], points[3],
        ],
        UP,
        1.,
        0.5,
    )
    .unwrap();
    assert_eq!(repeated, forward);
    assert_eq!(repeated.digest(), forward.digest());
    assert_ne!(reversed.digest(), forward.digest());
    // Wide rows leave the source: missing, never zero.
    let wide = Curve::new(&[[1., 6., 2.], [4., 6., 2.]], UP, 6., 1.).unwrap();
    let present = check(&volume, &wide);
    assert!(present > 0 && present < (wide.size()[0] * wide.size()[1]) as usize);
    let missing = reformat(&phantom(true), &wide, &|| false).unwrap();
    assert!(missing.frame.pixels.iter().all(|v| v.is_nan()));
    // A curve image has no planar geometry, so measurements refuse it.
    assert!(orv_core::measurements::roi_stats(&missing.frame, [[0., 0.], [1., 1.]]).is_err());
}

#[test]
fn degenerate_aligned_oversized_and_cancelled_curves_are_refused() {
    let volume = phantom(false);
    for (points, reference, half, step, error) in [
        (vec![[1., 1., 1.]], UP, 1., 1., CurvedError::Points),
        (vec![[1., 1., 1.]; 3], UP, 1., 1., CurvedError::Points),
        (
            vec![[1., 1., 1.], [f64::NAN, 1., 1.]],
            UP,
            1.,
            1.,
            CurvedError::Points,
        ),
        (
            vec![[1., 1., 1.], [2., 1., 1.]],
            [0.; 3],
            1.,
            1.,
            CurvedError::Reference,
        ),
        (
            vec![[1., 1., 1.], [1., 1., 5.]],
            UP,
            1.,
            1.,
            CurvedError::Alignment,
        ),
        (
            vec![[1., 1., 1.], [1., 1.1, 5.]],
            UP,
            1.,
            1.,
            CurvedError::Alignment,
        ),
        (
            vec![[0., 0., 0.], [5000., 0., 0.]],
            UP,
            1.,
            1.,
            CurvedError::Limit,
        ),
        (
            vec![[0., 0., 0.], [5., 0., 0.]],
            UP,
            1.,
            0.,
            CurvedError::Limit,
        ),
        (
            vec![[0., 0., 0.], [5., 0., 0.]],
            UP,
            -1.,
            1.,
            CurvedError::Limit,
        ),
    ] {
        assert_eq!(Curve::new(&points, reference, half, step), Err(error));
    }
    let too_many = vec![[0., 0., 0.]; orv_core::curved::MAX_POINTS + 1];
    assert_eq!(Curve::new(&too_many, UP, 1., 1.), Err(CurvedError::Points));
    let curve = Curve::new(&[[1., 6., 8.], [4., 6., 8.]], UP, 2., 0.5).unwrap();
    assert_eq!(
        reformat(&volume, &curve, &|| true).map(|_| ()),
        Err(CurvedError::Cancelled)
    );
}
