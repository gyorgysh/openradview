// SPDX-License-Identifier: GPL-3.0-or-later
use orv_core::{
    fusion::{FusionError, resample_onto},
    study::{Frame, Geometry},
};
use std::sync::Arc;

/// Scalar frame whose sample at pixel (x, y) is the world value f(world).
fn frame(
    width: u32,
    height: u32,
    position: [f64; 3],
    orientation: [f64; 6],
    spacing: [f64; 2],
    reference: Option<&str>,
    f: impl Fn([f64; 3]) -> f64,
) -> Frame {
    let mut frame = (*orv_core::study::demo_study().series[0].frames[0]).clone();
    let geometry = Geometry {
        nifti_transform: None,
        frame_of_reference: reference.map(str::to_owned),
        position,
        orientation,
        spacing,
    };
    frame.width = width;
    frame.height = height;
    frame.rgba = None;
    frame.color = None;
    frame.validity = None;
    frame.presentation = None;
    frame.pixels = (0..height)
        .flat_map(|y| (0..width).map(move |x| (x, y)))
        .map(|(x, y)| {
            f(geometry
                .pixel_to_world([f64::from(x), f64::from(y)])
                .unwrap()) as f32
        })
        .collect();
    frame.geometry = Some(geometry);
    frame
}
const AXIAL: [f64; 6] = [1., 0., 0., 0., 1., 0.];
fn field(p: [f64; 3]) -> f64 {
    // Affine in-plane, so pixel-centre bilinear sampling reproduces it exactly.
    3. * p[0] - 2. * p[1] + 100.
}

#[test]
fn source_values_land_on_the_target_grid_by_world_position() {
    // Target: 1 mm pixels from (0, 0). Source: 2 mm columns, 0.5 mm rows,
    // shifted by (1, -1) mm and 0.25 mm along the normal.
    let target = frame(8, 8, [0., 0., 10.], AXIAL, [1., 1.], Some("2.25.1"), |_| 0.);
    let source = frame(
        6,
        30,
        [1., -1., 10.25],
        AXIAL,
        [0.5, 2.],
        Some("2.25.1"),
        field,
    );
    let fused = resample_onto(&target, &source, 0.5, &|| false).unwrap();
    assert_eq!((fused.width, fused.height), (8, 8));
    let mut present = 0;
    for y in 0..8 {
        for x in 0..8 {
            let index = (y * 8 + x) as usize;
            // Source covers x in [1, 11] mm and y in [-1, 13.5] mm.
            let inside = (1..=8).contains(&x);
            assert_eq!(fused.valid[index] == 1, inside, "{x},{y}");
            if inside {
                present += 1;
                let expected = field([f64::from(x), f64::from(y), 10.]);
                assert!((f64::from(fused.values[index]) - expected).abs() < 1e-4);
            }
        }
    }
    assert_eq!(present, 56);
}

#[test]
fn a_missing_source_neighbour_leaves_the_fused_sample_missing() {
    let target = frame(4, 4, [0.5, 0., 0.], AXIAL, [1., 1.], Some("2.25.1"), |_| 0.);
    let mut source = frame(4, 4, [0., 0., 0.], AXIAL, [1., 1.], Some("2.25.1"), field);
    let mut validity = vec![1u8; 16];
    validity[5] = 0;
    source.validity = Some(Arc::from(validity));
    let fused = resample_onto(&target, &source, 0., &|| false).unwrap();
    // Target pixel (x, y) samples source (x + 0.5, y) on an exact source row,
    // so only targets 0 and 1 of row 1 weight the missing source (1, 1).
    for (x, y) in [(0, 1), (1, 1)] {
        assert_eq!(fused.valid[y * 4 + x], 0, "{x},{y}");
    }
    for (x, y) in [(0, 0), (1, 0), (2, 1), (0, 2), (1, 2)] {
        assert_eq!(fused.valid[y * 4 + x], 1, "{x},{y}");
    }
    assert_eq!(fused.valid[3 * 4 + 3], 0, "outside the source");
    assert_eq!(fused.valid[3 * 4 + 2], 1);
}

#[test]
fn unrelated_references_planes_offsets_and_colour_are_refused() {
    let target = frame(4, 4, [0., 0., 0.], AXIAL, [1., 1.], Some("2.25.1"), |_| 0.);
    let cases: Vec<(Frame, f64, FusionError)> = vec![
        (
            frame(4, 4, [0.; 3], AXIAL, [1., 1.], Some("2.25.2"), field),
            1.,
            FusionError::Reference,
        ),
        (
            frame(4, 4, [0.; 3], AXIAL, [1., 1.], None, field),
            1.,
            FusionError::Reference,
        ),
        (
            frame(
                4,
                4,
                [0.; 3],
                [1., 0., 0., 0., 0., 1.],
                [1., 1.],
                Some("2.25.1"),
                field,
            ),
            1.,
            FusionError::Orientation,
        ),
        (
            frame(4, 4, [0., 0., 0.6], AXIAL, [1., 1.], Some("2.25.1"), field),
            0.5,
            FusionError::Offset,
        ),
    ];
    for (source, offset, error) in cases {
        assert_eq!(
            resample_onto(&target, &source, offset, &|| false),
            Err(error)
        );
    }
    let mut color = frame(4, 4, [0.; 3], AXIAL, [1., 1.], Some("2.25.1"), field);
    color.rgba = Some(vec![0; 64].into());
    assert_eq!(
        resample_onto(&target, &color, 1., &|| false),
        Err(FusionError::Color)
    );
    let same = frame(4, 4, [0.; 3], AXIAL, [1., 1.], Some("2.25.1"), field);
    assert_eq!(
        resample_onto(&target, &same, 0., &|| true),
        Err(FusionError::Cancelled)
    );
    // Opposite normals are still parallel planes.
    let flipped = frame(
        4,
        4,
        [3., 0., 0.],
        [-1., 0., 0., 0., 1., 0.],
        [1., 1.],
        Some("2.25.1"),
        field,
    );
    let fused = resample_onto(&target, &flipped, 0., &|| false).unwrap();
    assert!(fused.valid.iter().all(|v| *v == 1));
    assert!((f64::from(fused.values[1]) - field([1., 0., 0.])).abs() < 1e-4);
}
