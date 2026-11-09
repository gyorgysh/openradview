// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use crate::{
    study::{Geometry, demo_study},
    volume_view::PhysicalClip,
};
use std::sync::Arc;
fn source(sheared: bool) -> Arc<Volume> {
    let template = demo_study().series.remove(0).frames.remove(0);
    Arc::new(
        Volume::new(
            (0..4)
                .map(|z| {
                    let mut frame = (*template).clone();
                    frame.width = 5;
                    frame.height = 4;
                    frame.pixels = vec![10.; 20].into();
                    let mut validity = vec![1; 20];
                    if z == 0 {
                        validity[1] = 0;
                    }
                    frame.validity = Some(validity.into());
                    frame.geometry = Some(Geometry {
                        position: if sheared {
                            [
                                500000. + f64::from(z) * 0.25,
                                -200000. + f64::from(z) * 0.5,
                                300000. + f64::from(z) * 2.,
                            ]
                        } else {
                            [0., 0., f64::from(z)]
                        },
                        orientation: if sheared {
                            [-1., 0., 0., 0., 1., 0.]
                        } else {
                            [1., 0., 0., 0., 1., 0.]
                        },
                        spacing: if sheared { [2., 0.75] } else { [1., 1.] },
                        frame_of_reference: Some("generated exact mask ray".into()),
                        nifti_transform: None,
                    });
                    Arc::new(frame)
                })
                .collect(),
        )
        .unwrap(),
    )
}
fn cells(mut ray: CellRay) -> Vec<CellInterval> {
    let mut result = Vec::new();
    while let Some(cell) = ray.next_cell().unwrap() {
        result.push(cell);
    }
    assert!(ray.next_cell().unwrap().is_none());
    result
}
fn normalized(v: [f64; 3]) -> [f64; 3] {
    let length = v.iter().map(|v| v * v).sum::<f64>().sqrt();
    v.map(|v| v / length)
}
#[test]
fn selected_ids_preserve_object_revision_coverage_overlap_and_source_validity() {
    let source = source(false);
    let mut labels = Labels::empty(source.clone()).unwrap();
    labels.paint(0, &[[1., 0.]], 1, 9, || false).unwrap();
    let old = labels.try_clone(|| false).unwrap();
    let view = MaskView::manual(&labels, 9).unwrap();
    let sample = view.sample([1, 0, 0]).unwrap();
    assert_eq!(
        sample,
        Membership {
            known: true,
            present: true,
            source_valid: false
        }
    );
    assert!(!sample.eligible());
    assert!(view.sample([5, 0, 0]).is_none());
    assert_eq!(
        view.sample([4, 3, 3]).unwrap(),
        Membership {
            known: true,
            present: false,
            source_valid: true
        }
    );
    assert!(MaskView::manual(&labels, 0).is_err());
    assert_eq!(view.key(), MaskView::manual(&old, 9).unwrap().key());
    assert_eq!(view.revision(), old.revision());
    let other = Labels::empty(source.clone()).unwrap();
    assert_ne!(view.key(), MaskView::manual(&other, 9).unwrap().key());
    let old_revision = view.revision().clone();
    labels.paint(3, &[[4., 3.]], 1, 9, || false).unwrap();
    assert_ne!(
        MaskView::manual(&labels, 9).unwrap().revision(),
        &old_revision
    );
    // Two separate packed segments occupy the same cell; another source plane
    // is supplied zero for one and unknown for the other.
    let masks = crate::segments::Segments::from_binary(
        source,
        &[9, 40000],
        &[(9, 0), (40000, 0), (9, 1)],
        &[2, 0, 32, 0, 0, 0, 0, 0],
        Some(true),
        || false,
    )
    .unwrap();
    let a = MaskView::supplied(masks.mask(9).unwrap());
    let b = MaskView::supplied(masks.mask(40000).unwrap());
    assert!(masks.mask(65535).is_none());
    assert_ne!(a.key(), b.key());
    assert_eq!(a.key().object, b.key().object);
    assert!(a.sample([1, 0, 0]).unwrap().present && b.sample([1, 0, 0]).unwrap().present);
    assert!(!a.sample([1, 0, 0]).unwrap().source_valid);
    assert_eq!(
        a.sample([0, 0, 1]).unwrap(),
        Membership {
            known: true,
            present: false,
            source_valid: true
        }
    );
    assert_eq!(
        b.sample([0, 0, 1]).unwrap(),
        Membership {
            known: false,
            present: false,
            source_valid: true
        }
    );
}
#[test]
fn axis_rays_report_each_physical_cell_once_in_both_directions() {
    let source = source(false);
    for (origin, direction, expected) in [
        ([-2., 1., 1.], [1., 0., 0.], vec![0, 1, 2, 3, 4]),
        ([7., 1., 1.], [-1., 0., 0.], vec![4, 3, 2, 1, 0]),
    ] {
        let result = cells(CellRay::new(&source, origin, direction, [0., 20.], None).unwrap());
        assert_eq!(
            result.iter().map(|c| c.voxel[0]).collect::<Vec<_>>(),
            expected
        );
        assert!(
            result
                .iter()
                .all(|c| c.voxel[1..] == [1, 1]
                    && (c.range_mm[1] - c.range_mm[0] - 1.).abs() < 1e-12)
        );
        assert!(
            result
                .windows(2)
                .all(|p| p[0].range_mm[1] == p[1].range_mm[0])
        );
    }
    let inside =
        cells(CellRay::new(&source, [2., 1., 1.], [1., 0., 0.], [-10., 10.], None).unwrap());
    assert_eq!(
        inside.iter().map(|cell| cell.voxel[0]).collect::<Vec<_>>(),
        vec![0, 1, 2, 3, 4]
    );
    assert_eq!(inside.first().unwrap().range_mm[0], -2.5);
    assert_eq!(inside.last().unwrap().range_mm[1], 2.5);
    let partial =
        cells(CellRay::new(&source, [2., 1., 1.], [1., 0., 0.], [-0.2, 0.3], None).unwrap());
    assert_eq!(
        partial,
        vec![CellInterval {
            voxel: [2, 1, 1],
            range_mm: [-0.2, 0.3]
        }]
    );
    let positive =
        cells(CellRay::new(&source, [0.5, 1., 1.], [1., 0., 0.], [0., 1.], None).unwrap());
    let negative =
        cells(CellRay::new(&source, [0.5, 1., 1.], [-1., 0., 0.], [0., 1.], None).unwrap());
    assert_eq!(positive[0].voxel, [1, 1, 1]);
    assert_eq!(negative[0].voxel, [0, 1, 1]);
    // A parallel ray exactly on the upper domain face is outside, while a
    // parallel internal half tie uses the larger source-cell index.
    assert!(
        cells(CellRay::new(&source, [-2., 3.5, 1.], [1., 0., 0.], [0., 20.], None).unwrap())
            .is_empty()
    );
    assert!(
        cells(CellRay::new(&source, [-2., 0.5, 1.], [1., 0., 0.], [0., 20.], None).unwrap())
            .iter()
            .all(|c| c.voxel[1] == 1)
    );
}
#[test]
fn corner_ties_advance_all_axes_and_clips_keep_partial_cell_lengths() {
    let source = source(false);
    let dir = normalized([1.; 3]);
    let diagonal = cells(CellRay::new(&source, [-1.; 3], dir, [0., 20.], None).unwrap());
    assert_eq!(
        diagonal.iter().map(|c| c.voxel).collect::<Vec<_>>(),
        vec![[0, 0, 0], [1, 1, 1], [2, 2, 2], [3, 3, 3]]
    );
    let clip = PhysicalClip {
        minimum_mm: [0.25, 0., 0.],
        maximum_mm: [2.75, 2., 2.],
    };
    let clipped =
        cells(CellRay::new(&source, [-2., 1., 1.], [1., 0., 0.], [0., 20.], Some(&clip)).unwrap());
    assert_eq!(
        clipped.iter().map(|c| c.voxel[0]).collect::<Vec<_>>(),
        vec![0, 1, 2, 3]
    );
    for (cell, length) in clipped.iter().zip([0.25, 1., 1., 0.25]) {
        assert!((cell.range_mm[1] - cell.range_mm[0] - length).abs() < 1e-12);
    }
    let reverse =
        cells(CellRay::new(&source, [7., 1., 1.], [-1., 0., 0.], [0., 20.], Some(&clip)).unwrap());
    assert_eq!(
        reverse.iter().map(|c| c.voxel[0]).collect::<Vec<_>>(),
        vec![3, 2, 1, 0]
    );
    let missed = PhysicalClip {
        minimum_mm: [10.; 3],
        maximum_mm: [11.; 3],
    };
    assert!(
        cells(
            CellRay::new(
                &source,
                [-2., 1., 1.],
                [1., 0., 0.],
                [0., 20.],
                Some(&missed)
            )
            .unwrap()
        )
        .is_empty()
    );
}
// Independent exhaustive cell/box intersections, sorted after evaluating every
// source cell. There is no incremental cursor or neighboring-cell step here.
fn exhaustive(
    source: &Volume,
    origin: [f64; 3],
    direction: [f64; 3],
    range: [f64; 2],
) -> Vec<CellInterval> {
    let o = source.world_to_voxel(origin);
    let d = source.world_vector_to_voxel(direction);
    let [w, h, depth] = source.dimensions();
    let mut result = Vec::new();
    for z in 0..depth {
        for y in 0..h {
            for x in 0..w {
                let voxel = [x, y, z];
                let mut near = range[0];
                let mut far = range[1];
                let mut possible = true;
                for a in 0..3 {
                    let low = voxel[a] as f64 - 0.5;
                    let high = low + 1.;
                    if d[a] == 0. {
                        possible &= o[a] >= low && o[a] < high;
                    } else {
                        let ta = (low - o[a]) / d[a];
                        let tb = (high - o[a]) / d[a];
                        near = near.max(ta.min(tb));
                        far = far.min(ta.max(tb));
                    }
                }
                if possible && near < far {
                    result.push(CellInterval {
                        voxel,
                        range_mm: [near, far],
                    });
                }
            }
        }
    }
    result.sort_by(|a, b| a.range_mm[0].total_cmp(&b.range_mm[0]));
    result
}
#[test]
fn affine_traversal_matches_exhaustive_intersections_on_reversed_sheared_large_coordinates() {
    for sheared in [false, true] {
        let source = source(sheared);
        for i in 0..64 {
            let origin = source.voxel_to_world([
                -3. + f64::from(i % 3) * 0.375,
                -2. + f64::from(i % 5) * 0.7,
                -1. + f64::from(i % 7) * 0.4,
            ]);
            let target = source.voxel_to_world([
                f64::from(i % 5),
                f64::from((i * 3) % 4),
                f64::from((i * 5) % 4),
            ]);
            let direction = normalized(std::array::from_fn(|a| target[a] - origin[a]));
            let range = [-10., 40.];
            let expected = exhaustive(&source, origin, direction, range);
            let actual = cells(CellRay::new(&source, origin, direction, range, None).unwrap());
            assert_eq!(
                actual.len(),
                expected.len(),
                "case {sheared}/{i}: {actual:?} vs {expected:?}"
            );
            for (actual, expected) in actual.iter().zip(expected) {
                assert_eq!(actual.voxel, expected.voxel, "case {sheared}/{i}");
                for (a, b) in actual.range_mm.into_iter().zip(expected.range_mm) {
                    assert!((a - b).abs() < 1e-9);
                }
            }
        }
    }
}
#[test]
fn picks_skip_missing_source_and_unknown_coverage_but_keep_single_cell_regions() {
    let source = source(false);
    let mut labels = Labels::empty(source.clone()).unwrap();
    labels.paint(0, &[[1., 0.]], 1, 9, || false).unwrap();
    let view = MaskView::manual(&labels, 9).unwrap();
    let mut ray = view
        .ray([1., 0., -2.], [0., 0., 1.], [0., 20.], None)
        .unwrap();
    assert!(ray.next_cell().unwrap().unwrap().membership.present);
    assert!(!view.sample([1, 0, 0]).unwrap().eligible());
    assert!(ray.first_eligible_hit(|| false).unwrap().is_none());
    let masks = crate::segments::Segments::from_binary(
        source,
        &[9],
        &[(9, 1)],
        &[2, 0, 0, 0],
        None,
        || false,
    )
    .unwrap();
    let view = MaskView::supplied(masks.mask(9).unwrap());
    let mut ray = view
        .ray([1., 0., -2.], [0., 0., 1.], [0., 20.], None)
        .unwrap();
    let first = ray.next_cell().unwrap().unwrap();
    assert!(!first.membership.known);
    let hit = ray.first_eligible_hit(|| false).unwrap().unwrap();
    assert_eq!(hit.key, view.key());
    assert_eq!(&hit.revision, view.revision());
    assert_eq!(hit.voxel, [1, 0, 1]);
    assert_eq!(hit.range_mm, [2.5, 3.5]);
    assert_eq!(hit.position_mm, [1., 0., 0.5]);
    assert_eq!(
        view.ray([1., 0., -2.], [0., 0., 1.], [0., 20.], None)
            .unwrap()
            .first_eligible_hit(|| true),
        Err("Cancelled")
    );
}
#[test]
fn invalid_rays_are_refused_and_misses_are_empty() {
    let source = source(false);
    for (origin, direction, range) in [
        ([f64::NAN, 0., 0.], [1., 0., 0.], [0., 20.]),
        ([0.; 3], [0.; 3], [0., 20.]),
        ([0.; 3], [2., 0., 0.], [0., 20.]),
        ([0.; 3], [1., 0., 0.], [1., 1.]),
        ([0.; 3], [1., 0., 0.], [0., f64::INFINITY]),
    ] {
        assert!(CellRay::new(&source, origin, direction, range, None).is_err());
    }
    for (origin, direction, range) in [
        ([-2., 8., 1.], [1., 0., 0.], [0., 20.]),
        ([-2., 1., 1.], [-1., 0., 0.], [0., 20.]),
        ([-2., 1., 1.], [1., 0., 0.], [-2., 0.]),
    ] {
        assert!(cells(CellRay::new(&source, origin, direction, range, None).unwrap()).is_empty());
    }
}
