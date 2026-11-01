// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use crate::{
    segments::tests::{DATA, FRAMES, source},
    volume::Volume,
};
use std::cell::Cell;

fn masks() -> Segments {
    Segments::from_binary(source(3, 2), &[9, 2, 42], &FRAMES, &DATA, None, || false).unwrap()
}
fn image(
    masks: &Segments,
    size: [u32; 2],
    position: [f64; 3],
    orientation: [f64; 6],
    spacing: [f64; 2],
) -> Frame {
    let mut frame = (*masks.source.frames()[0]).clone();
    frame.width = size[0];
    frame.height = size[1];
    frame.pixels = vec![20.; size[0] as usize * size[1] as usize].into();
    frame.validity = None;
    let geometry = frame.geometry.as_mut().unwrap();
    geometry.position = position;
    geometry.orientation = orientation;
    geometry.spacing = spacing;
    frame
}
#[test]
fn source_and_reconstructed_planes_preserve_independent_overlap_coverage_and_validity() {
    let masks = masks();
    let sampled = masks
        .sample_plane(&masks.source.frames()[0], || false)
        .unwrap();
    assert_eq!(sampled.dimensions(), [3, 2]);
    assert_eq!(sampled.numbers(), [9, 2, 42]);
    assert_eq!(sampled.positive_bits(), [1, 3, 0, 3, 0, 0]);
    assert_eq!(sampled.supplied_bits(), [3; 6]);
    assert_eq!(sampled.eligibility(), [1, 0, 1, 1, 1, 1]);
    let coronal = image(
        &masks,
        [3, 3],
        [0., 0., 0.],
        [1., 0., 0., 0., 0., 1.],
        [2., 1.],
    );
    let sampled = masks.sample_plane(&coronal, || false).unwrap();
    assert_eq!(sampled.positive_bits(), [1, 3, 0, 0, 0, 2, 1, 0, 0]);
    assert_eq!(sampled.supplied_bits(), [3, 3, 3, 2, 2, 2, 1, 1, 1]);
    assert_eq!(sampled.eligibility(), [1, 0, 1, 1, 1, 1, 1, 1, 1]);
    let sagittal = image(
        &masks,
        [2, 3],
        [1., 0., 0.],
        [0., 1., 0., 0., 0., 1.],
        [2., 2.],
    );
    let sampled = masks.sample_plane(&sagittal, || false).unwrap();
    assert_eq!(sampled.positive_bits(), [3, 0, 0, 2, 0, 0]);
    assert_eq!(sampled.supplied_bits(), [3, 3, 2, 2, 1, 1]);
    let angle = std::f64::consts::FRAC_1_SQRT_2;
    let mut oblique = image(
        &masks,
        [2, 2],
        [0., 0., 0.],
        [angle, angle, 0., -angle, angle, 0.],
        [2f64.sqrt(); 2],
    );
    oblique.pixels = vec![20., 20., 20., f32::NAN].into();
    let sampled = masks.sample_plane(&oblique, || false).unwrap();
    assert_eq!(sampled.positive_bits(), [1, 0, 0, 3]);
    assert_eq!(sampled.supplied_bits(), [3, 3, 0, 3]);
    assert_eq!(sampled.eligibility(), [1, 1, 0, 0]);
    assert_eq!(masks.overlap_cells(), 2);
}
#[test]
fn categorical_bits_follow_reversed_sheared_cells_and_half_open_boundaries() {
    let original = source(3, 2);
    let frames = original
        .frames()
        .iter()
        .rev()
        .map(|frame| {
            let mut frame = (**frame).clone();
            frame.geometry.as_mut().unwrap().orientation[0] = -1.;
            Arc::new(frame)
        })
        .collect();
    let src = Arc::new(Volume::new(frames).unwrap());
    assert!(src.voxel_to_world([0., 0., 1.])[2] < src.voxel_to_world([0., 0., 0.])[2]);
    let masks = Segments::from_binary(src, &[9, 2], &FRAMES, &DATA, None, || false).unwrap();
    for (x, bits, coverage) in [
        (-0.500001, 0, 0),
        (-0.5, 1, 3),
        (0.5, 3, 3),
        (2.499999, 0, 3),
        (2.5, 0, 0),
    ] {
        let world = masks.source.voxel_to_world([x, 0., 0.]);
        let plane = image(&masks, [1, 1], world, [1., 0., 0., 0., 1., 0.], [1., 1.]);
        let sampled = masks.sample_plane(&plane, || false).unwrap();
        assert_eq!(sampled.positive_bits(), [bits], "source x {x}");
        assert_eq!(sampled.supplied_bits(), [coverage], "source x {x}");
    }
    for (z, bits, coverage) in [(0., 3, 3), (1., 0, 2), (2., 0, 1)] {
        let plane = image(
            &masks,
            [1, 1],
            masks.source.voxel_to_world([1., 0., z]),
            [1., 0., 0., 0., 1., 0.],
            [1., 1.],
        );
        let sampled = masks.sample_plane(&plane, || false).unwrap();
        assert_eq!(sampled.positive_bits(), [bits]);
        assert_eq!(sampled.supplied_bits(), [coverage]);
    }
}
#[test]
fn all_64_segment_positions_and_large_numeric_ids_remain_distinct() {
    let numbers = (1..=63)
        .chain(std::iter::once(u16::MAX))
        .collect::<Vec<_>>();
    let masks = Segments::from_binary(
        source(3, 2),
        &numbers,
        &[(1, 0), (u16::MAX, 0)],
        &[0x41, 0x08],
        None,
        || false,
    )
    .unwrap();
    let sampled = masks
        .sample_plane(&masks.source.frames()[0], || false)
        .unwrap();
    let high = 1u64 << 63;
    assert_eq!(sampled.numbers(), numbers);
    assert_eq!(sampled.positive_bits(), [1 | high, 0, 0, 0, 0, high]);
    assert_eq!(sampled.supplied_bits(), [1 | high; 6]);
    let omitted = masks
        .sample_plane(&masks.source.frames()[1], || false)
        .unwrap();
    assert_eq!(omitted.supplied_bits(), [0; 6]);
    assert_eq!(omitted.positive_bits(), [0; 6]);
}
#[test]
fn sampling_refuses_invalid_planes_and_releases_admitted_buffers_on_failure_or_cancel() {
    let masks = masks();
    let original = &masks.source.frames()[0];
    let budget = MemoryBudget::new(101);
    assert!(
        masks
            .sample_with_budget(original, &|| false, &budget)
            .is_err()
    );
    assert_eq!(budget.used(), 0);
    budget.set_limit(102);
    let sampled = masks
        .sample_with_budget(original, &|| false, &budget)
        .unwrap();
    assert_eq!(budget.used(), 102);
    drop(sampled);
    assert_eq!(budget.used(), 0);
    let mut wrong = (**original).clone();
    wrong.geometry.as_mut().unwrap().frame_of_reference = Some("other-generated-reference".into());
    assert!(masks.sample_plane(&wrong, || false).is_err());
    let mut slab = (**original).clone();
    slab.slab = Some(crate::slab::Slab {
        projection: crate::slab::SlabProjection::Maximum,
        thickness_mm: 2.,
        intervals: 4,
    });
    assert!(masks.sample_plane(&slab, || false).is_err());
    let mut malformed = (**original).clone();
    malformed.pixels = Arc::new([]);
    assert!(masks.sample_plane(&malformed, || false).is_err());
    malformed.width = crate::plane_sampling::MAX_PIXELS as u32 + 1;
    malformed.height = 1;
    assert!(masks.sample_plane(&malformed, || false).is_err());
    let large = image(
        &masks,
        [2048, 2],
        [0.; 3],
        [1., 0., 0., 0., 1., 0.],
        [1.; 2],
    );
    budget.set_limit(large.pixels.len() as u64 * 17);
    let calls = Cell::new(0);
    let result = masks.sample_with_budget(
        &large,
        &|| {
            calls.set(calls.get() + 1);
            calls.get() >= 3
        },
        &budget,
    );
    assert!(matches!(result, Err("Cancelled")));
    assert!(calls.get() >= 3);
    assert_eq!(budget.used(), 0);
    assert!(masks.sample_plane(original, || true).is_err());
}
