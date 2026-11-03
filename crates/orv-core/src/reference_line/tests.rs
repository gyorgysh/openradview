// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
fn image(size: [u32; 2], position: V3, orientation: [f64; 6], spacing: [f64; 2]) -> Frame {
    Frame {
        presentation: None,
        calibration: None,
        projection: None,
        sop: None,
        slab: None,
        scalar: None,
        validity: None,
        color: None,
        nifti: None,
        width: size[0],
        height: size[1],
        pixels: Vec::new().into(),
        rgba: None,
        geometry: Some(Geometry {
            position,
            orientation,
            spacing,
            frame_of_reference: Some("generated-private-reference".into()),
            nifti_transform: None,
        }),
        instance: 1,
        frame_index: 0,
        monochrome1: false,
        default_window: crate::WindowLevel::SOFT_TISSUE,
    }
}
fn pair() -> (Frame, Frame) {
    (
        image([5, 5], [0.; 3], [1., 0., 0., 0., 1., 0.], [2., 1.]),
        image([3, 3], [1., 4., -2.], [1., 0., 0., 0., 0., 1.], [2., 1.]),
    )
}
fn segment(result: Intersection, mut expected: [[f64; 2]; 2]) {
    let Intersection::Segment(mut actual) = result else {
        panic!("Expected line: {result:?}");
    };
    actual.sort_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
    expected.sort_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
    for (actual, expected) in actual
        .into_iter()
        .flatten()
        .zip(expected.into_iter().flatten())
    {
        assert!((actual - expected).abs() < 1e-9, "{actual} != {expected}");
    }
}
#[test]
fn anisotropic_lines_clip_to_both_footprints_in_each_direction_without_reading_pixels() {
    let (target, other) = pair();
    assert!(target.pixels.is_empty() && other.pixels.is_empty());
    segment(
        intersection(&target, &other).unwrap(),
        [[0.5, 2.], [3.5, 2.]],
    );
    segment(
        intersection(&other, &target).unwrap(),
        [[-0.5, 1.], [2.5, 1.]],
    );
    let mut reversed = other.clone();
    reversed.geometry.as_mut().unwrap().orientation[0] = -1.;
    reversed.geometry.as_mut().unwrap().position[0] = 3.;
    segment(
        intersection(&target, &reversed).unwrap(),
        [[0.5, 2.], [3.5, 2.]],
    );
    let mut outside = other.clone();
    outside.geometry.as_mut().unwrap().position[0] = 20.;
    assert_eq!(
        intersection(&target, &outside).unwrap(),
        Intersection::Outside
    );
}
#[test]
fn diagonal_planes_include_half_pixel_edges_but_not_a_point_touch() {
    let target = image([5, 5], [0.; 3], [1., 0., 0., 0., 1., 0.], [1.; 2]);
    let a = std::f64::consts::FRAC_1_SQRT_2;
    let other = image(
        [7, 3],
        [2. - 6. * a, 2. + 6. * a, -2.],
        [a, -a, 0., 0., 0., 1.],
        [2.; 2],
    );
    segment(
        intersection(&target, &other).unwrap(),
        [[-0.5, 4.5], [4.5, -0.5]],
    );
    let tangent = image(
        [7, 3],
        [4.5 - 6. * a, 4.5 + 6. * a, -2.],
        [a, -a, 0., 0., 0., 1.],
        [2.; 2],
    );
    assert_eq!(
        intersection(&target, &tangent).unwrap(),
        Intersection::Outside
    );
}
#[test]
fn parallel_coplanar_nearly_parallel_and_stable_small_angles_are_explicit() {
    let (target, _) = pair();
    let mut other = target.clone();
    assert_eq!(
        intersection(&target, &other).unwrap(),
        Intersection::Coplanar
    );
    other.geometry.as_mut().unwrap().position[2] = 1.;
    assert_eq!(
        intersection(&target, &other).unwrap(),
        Intersection::Parallel
    );
    let angle = 1e-10f64;
    other.geometry.as_mut().unwrap().orientation = [1., 0., 0., 0., angle.cos(), angle.sin()];
    assert_eq!(
        intersection(&target, &other).unwrap(),
        Intersection::Parallel
    );
    other.geometry.as_mut().unwrap().position[2] = 0.;
    assert_eq!(
        intersection(&target, &other).unwrap(),
        Intersection::Coplanar
    );
    let angle = 1e-7f64;
    other.geometry.as_mut().unwrap().orientation = [1., 0., 0., 0., angle.cos(), angle.sin()];
    other.geometry.as_mut().unwrap().position[1] = 4.;
    segment(
        intersection(&target, &other).unwrap(),
        [[-0.5, 2.], [4.5, 2.]],
    );
}
#[test]
fn reference_context_and_projection_refusals_precede_any_line() {
    let (mut target, mut other) = pair();
    other.geometry.as_mut().unwrap().frame_of_reference = Some("unrelated".into());
    assert_eq!(intersection(&target, &other), Err(GeometryError::Reference));
    for reference in [None, Some(String::new()), Some("x".repeat(129))] {
        target.geometry.as_mut().unwrap().frame_of_reference = reference.clone();
        other.geometry.as_mut().unwrap().frame_of_reference = reference;
        assert_eq!(intersection(&target, &other), Err(GeometryError::Reference));
    }
    let (target, mut other) = pair();
    other.geometry.as_mut().unwrap().nifti_transform = Some(crate::study::NiftiTransform {
        method: crate::study::NiftiMethod::Sform,
        code: 4,
    });
    assert_eq!(intersection(&target, &other), Err(GeometryError::Reference));
    let (target, mut other) = pair();
    other.slab = Some(crate::slab::Slab {
        projection: crate::slab::SlabProjection::Maximum,
        thickness_mm: 2.,
        intervals: 2,
    });
    assert_eq!(intersection(&target, &other), Err(GeometryError::Pixels));
}
#[test]
fn near_orthonormal_supplied_axes_use_their_exact_gram_coordinates_and_large_translations_cancel() {
    let (mut target, mut other) = pair();
    other.geometry.as_mut().unwrap().orientation = [1.00002, 0., 0., 0.00003, 0., 0.99998];
    let shift = 0.00003 * 2. / 0.99998;
    segment(
        intersection(&target, &other).unwrap(),
        [
            [1. - 0.5 * 1.00002 + shift, 2.],
            [1. + 2.5 * 1.00002 + shift, 2.],
        ],
    );
    let (base, plain) = pair();
    target = base;
    other = plain;
    for axis in 0..3 {
        let translation = [1e12, -2e12, 3e12][axis];
        target.geometry.as_mut().unwrap().position[axis] += translation;
        other.geometry.as_mut().unwrap().position[axis] += translation;
    }
    segment(
        intersection(&target, &other).unwrap(),
        [[0.5, 2.], [3.5, 2.]],
    );
}
#[test]
fn malformed_and_unrepresentable_geometry_is_refused_without_pixels_or_allocation() {
    let (target, mut other) = pair();
    other.width = 0;
    assert_eq!(
        intersection(&target, &other),
        Err(GeometryError::Dimensions)
    );
    let (target, mut other) = pair();
    other.geometry = None;
    assert_eq!(intersection(&target, &other), Err(GeometryError::Missing));
    let (target, mut other) = pair();
    other.geometry.as_mut().unwrap().position[0] = f64::NAN;
    assert_eq!(intersection(&target, &other), Err(GeometryError::Missing));
    let (target, mut other) = pair();
    other.geometry.as_mut().unwrap().orientation[0] = 2.;
    assert_eq!(
        intersection(&target, &other),
        Err(GeometryError::Orientation)
    );
    let (target, mut other) = pair();
    other.geometry.as_mut().unwrap().spacing[0] = 0.;
    assert_eq!(intersection(&target, &other), Err(GeometryError::Spacing));
    let (mut target, other) = pair();
    target.geometry.as_mut().unwrap().spacing[0] = f64::MAX;
    assert_eq!(intersection(&target, &other), Err(GeometryError::Pixels));
    let target = image([u32::MAX, 5], [0.; 3], [1., 0., 0., 0., 1., 0.], [1.; 2]);
    let other = image(
        [u32::MAX, 5],
        [0., 2., -2.],
        [1., 0., 0., 0., 0., 1.],
        [1.; 2],
    );
    segment(
        intersection(&target, &other).unwrap(),
        [[-0.5, 2.], [f64::from(u32::MAX) - 0.5, 2.]],
    );
}
