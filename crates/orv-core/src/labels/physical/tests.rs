// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use crate::{
    resources::MemoryBudget,
    study::{Geometry, demo_study},
    vector::*,
    volume::Volume,
};
use std::cell::Cell;

fn affine(kind: usize) -> (V3, [V3; 3]) {
    match kind {
        1 => (
            [500000., -200000., 300000.],
            [[-0.75, 0., 0.], [0., 2., 0.], [0.25, 0.5, -2.]],
        ),
        2 => (
            [-40., 17., 90.],
            [[0.45, 0.6, 0.], [-1.2, 0.9, 0.], [0.125, 0.25, 1.5]],
        ),
        _ => ([0.; 3], [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]]),
    }
}
fn world(kind: usize, p: V3) -> V3 {
    let (o, b) = affine(kind);
    std::array::from_fn(|a| o[a] + b[0][a] * p[0] + b[1][a] * p[1] + b[2][a] * p[2])
}
fn source(dims: [usize; 3], kind: usize, missing: bool) -> Arc<Volume> {
    let template = demo_study().series.remove(0).frames.remove(0);
    let frames = (0..dims[2])
        .map(|z| {
            let mut f = (*template).clone();
            f.width = dims[0] as u32;
            f.height = dims[1] as u32;
            let count = dims[0] * dims[1];
            f.pixels = vec![0.; count].into();
            f.validity = missing.then(|| {
                (0..count)
                    .map(|i| u8::from(!(i + z * count).is_multiple_of(17)))
                    .collect::<Vec<_>>()
                    .into()
            });
            f.geometry = Some(Geometry {
                position: world(kind, [0., 0., z as f64]),
                orientation: match kind {
                    1 => [-1., 0., 0., 0., 1., 0.],
                    2 => [0.6, 0.8, 0., -0.8, 0.6, 0.],
                    _ => [1., 0., 0., 0., 1., 0.],
                },
                spacing: match kind {
                    1 => [2., 0.75],
                    2 => [1.5, 0.75],
                    _ => [1.; 2],
                },
                frame_of_reference: Some("generated physical brush".into()),
                nifti_transform: None,
            });
            Arc::new(f)
        })
        .collect();
    Arc::new(Volume::new(frames).unwrap())
}
fn brush() -> PhysicalBrush {
    PhysicalBrush {
        label: 7,
        action: BrushAction::Add,
        radius_mm: 1.375,
        shape: BrushShape::Sphere,
        clip: None,
        protected_ids: [false; 256],
    }
}
// Interior distance uses cross-product area, independently of the implementation's
// closest-point subtraction. Endpoint tests include the closed physical boundary.
fn capsule(p: V3, a: V3, b: V3, r: f64) -> bool {
    let d = sub(b, a);
    let q = sub(p, a);
    let t = dot(q, d);
    let length = norm(d);
    if t <= 0. || length == 0. {
        norm(q) <= r
    } else if t >= length * length {
        norm(sub(p, b)) <= r
    } else {
        norm(cross(q, d)) <= r * length
    }
}
fn oracle(p: V3, points: &[V3], settings: &PhysicalBrush) -> bool {
    if settings
        .clip
        .as_ref()
        .is_some_and(|c| (0..3).any(|a| p[a] < c.minimum_mm[a] || p[a] > c.maximum_mm[a]))
    {
        return false;
    }
    let project = |q| match settings.shape {
        BrushShape::Sphere => q,
        BrushShape::Plane {
            origin_mm, normal, ..
        } => {
            // An independent orthonormal 2D basis; do not reuse the production
            // normal projection or its bounding boxes.
            let n = mul(normal, 1. / norm(normal));
            let axis = if n[0].abs() < 0.8 {
                [1., 0., 0.]
            } else {
                [0., 1., 0.]
            };
            let u = cross(n, axis);
            let u = mul(u, 1. / norm(u));
            let v = cross(n, u);
            let q = sub(q, origin_mm);
            [dot(q, u), dot(q, v), 0.]
        }
    };
    if let BrushShape::Plane {
        origin_mm,
        normal,
        thickness_mm,
    } = settings.shape
        && dot(sub(p, origin_mm), normal).abs() > thickness_mm * 0.5 * norm(normal)
    {
        return false;
    }
    if points.len() == 1 {
        return capsule(
            project(p),
            project(points[0]),
            project(points[0]),
            settings.radius_mm,
        );
    }
    points
        .windows(2)
        .any(|s| capsule(project(p), project(s[0]), project(s[1]), settings.radius_mm))
}

#[test]
fn physical_sweeps_match_independent_full_grid_geometry_under_affines_clips_and_missing_data() {
    let dims = [23, 17, 11];
    for kind in 0..3 {
        for shape in 0..3 {
            let mut base = Labels::empty(source(dims, kind, true)).unwrap();
            for i in 0..dims.into_iter().product() {
                if i % 53 == 0 {
                    base.set_value(i, 9).unwrap();
                } else if i % 47 == 0 {
                    base.set_value(i, 7).unwrap();
                }
            }
            let base = Arc::new(base);
            let before = base.values().iter().copied().collect::<Vec<_>>();
            let mut settings = brush();
            settings.radius_mm = 2.137;
            let origin = world(kind, [10.1, 8.3, 4.7]);
            if shape != 0 {
                settings.shape = BrushShape::Plane {
                    origin_mm: origin,
                    normal: if shape == 1 {
                        [0., 0., 1.]
                    } else {
                        [1., 2., 2.]
                    },
                    thickness_mm: 1.73,
                };
            }
            let [lo, hi] = base.source.bounds();
            settings.clip = Some(PhysicalClip {
                minimum_mm: std::array::from_fn(|a| lo[a] + 0.137),
                maximum_mm: std::array::from_fn(|a| hi[a] - 0.271),
            });
            let points =
                [[-2., 2.3, 2.7], [10.1, 8.3, 4.7], [24., 14.7, 8.3]].map(|p| world(kind, p));
            let proposal = base
                .propose_physical_stroke(&settings, &points, || false)
                .unwrap();
            let mut changed = 0;
            for z in 0..dims[2] {
                for y in 0..dims[1] {
                    for x in 0..dims[0] {
                        let index = (z * dims[1] + y) * dims[0] + x;
                        let old = before[index];
                        let hit = oracle(
                            world(kind, [x as f64, y as f64, z as f64]),
                            &points,
                            &settings,
                        );
                        let expected = if old == 0 && !index.is_multiple_of(17) && hit {
                            7
                        } else {
                            old
                        };
                        changed += usize::from(expected != old);
                        assert_eq!(
                            proposal.value([x, y, z]),
                            Some(expected),
                            "kind={kind} shape={shape} cell={:?}",
                            [x, y, z]
                        );
                    }
                }
            }
            assert!(changed > 0);
            assert_eq!(proposal.summary().changed_voxels, changed);
            assert_eq!(
                proposal.summary().changed_tiles,
                proposal.changed_tiles().count()
            );
            let next = proposal.apply(&base, &settings, &points, || false).unwrap();
            assert_eq!(next.object_id(), base.object_id());
            assert_eq!(next.parent_revision(), Some(base.revision()));
            assert_ne!(next.revision(), base.revision());
            assert!(base.values().iter().copied().eq(before));
        }
    }
}

#[test]
fn continuous_capsules_cover_fast_drags_without_recounting_overlaps_or_depends_on_subdivision() {
    let base = Arc::new(Labels::empty(source([64, 8, 8], 0, false)).unwrap());
    let mut settings = brush();
    settings.radius_mm = 0.6;
    let sparse = [[1., 3., 3.], [60., 3., 3.]];
    let dense = (1..=60).map(|x| [x as f64, 3., 3.]).collect::<Vec<_>>();
    let repeated = [[60., 3., 3.], [1., 3., 3.], [1., 3., 3.], [60., 3., 3.]];
    let first = base
        .propose_physical_stroke(&settings, &sparse, || false)
        .unwrap();
    assert_eq!(first.summary().changed_voxels, 60);
    assert_eq!(
        first.summary().changed_bounds,
        Some([[1, 3, 3], [61, 4, 4]])
    );
    for points in [&dense[..], &repeated[..]] {
        let next = base
            .propose_physical_stroke(&settings, points, || false)
            .unwrap();
        assert_eq!(next.summary().changed_voxels, 60);
        for x in 0..64 {
            for z in 0..8 {
                for y in 0..8 {
                    assert_eq!(first.value([x, y, z]), next.value([x, y, z]));
                }
            }
        }
    }
    for x in 1..=60 {
        assert_eq!(first.value([x, 3, 3]), Some(7));
    }
    assert_eq!(first.value([30, 4, 3]), Some(0));
}

#[test]
fn closed_sphere_and_plane_boundaries_use_physical_centres_and_explicit_thickness() {
    let base = Arc::new(Labels::empty(source([7; 3], 0, false)).unwrap());
    let mut settings = brush();
    settings.radius_mm = 1.;
    let centre = [[3.; 3]];
    let p = base
        .propose_physical_stroke(&settings, &centre, || false)
        .unwrap();
    assert_eq!(p.summary().changed_voxels, 7);
    settings.radius_mm = 1f64.next_down();
    assert_eq!(
        base.propose_physical_stroke(&settings, &centre, || false)
            .unwrap()
            .summary()
            .changed_voxels,
        1
    );
    settings.radius_mm = 1.;
    settings.shape = BrushShape::Plane {
        origin_mm: [0., 0., 3.],
        normal: [0., 0., 4.],
        thickness_mm: 2.,
    };
    let p = base
        .propose_physical_stroke(&settings, &[[3., 3., 200.]], || false)
        .unwrap();
    assert_eq!(p.summary().changed_voxels, 15);
    assert_eq!(p.value([3, 3, 2]), Some(7));
    assert_eq!(p.value([3, 3, 4]), Some(7));
    assert_eq!(p.value([3, 3, 5]), Some(0));
    settings.shape = BrushShape::Plane {
        origin_mm: [0., 0., 3.],
        normal: [0., 0., f64::from_bits(1)],
        thickness_mm: 1.,
    };
    assert_eq!(
        base.propose_physical_stroke(&settings, &centre, || false)
            .unwrap()
            .summary()
            .changed_voxels,
        5
    );
}

#[test]
fn erase_and_add_preserve_validity_competing_ids_protection_and_clip() {
    let points = [[4.; 3]];
    let mut base = Labels::empty(source([9; 3], 0, true)).unwrap();
    for i in 0..729 {
        base.set_value(i, if i % 11 == 0 { 9 } else { 7 }).unwrap();
    }
    let base = Arc::new(base);
    let mut settings = brush();
    settings.radius_mm = 100.;
    settings.action = BrushAction::Erase;
    settings.clip = Some(PhysicalClip {
        minimum_mm: [2.; 3],
        maximum_mm: [6.; 3],
    });
    let p = base
        .propose_physical_stroke(&settings, &[[4.; 3]], || false)
        .unwrap();
    let mut changes = 0;
    for z in 0..9 {
        for y in 0..9 {
            for x in 0..9 {
                let i = (z * 9 + y) * 9 + x;
                let old = base.value([x, y, z]).unwrap();
                let remove = old == 7
                    && !i.is_multiple_of(17)
                    && [x, y, z].into_iter().all(|n| (2..=6).contains(&n));
                assert_eq!(p.value([x, y, z]), Some(if remove { 0 } else { old }));
                changes += usize::from(remove);
            }
        }
    }
    assert_eq!(p.summary().changed_voxels, changes);
    let next = p.apply(&base, &settings, &points, || false).unwrap();
    settings.action = BrushAction::Add;
    settings.protected_ids[0] = true;
    let no_op = next
        .propose_physical_stroke(&settings, &[[4.; 3]], || false)
        .unwrap();
    assert_eq!(no_op.summary().changed_voxels, 0);
    assert!(Arc::ptr_eq(
        &no_op.apply(&next, &settings, &points, || false).unwrap(),
        &next
    ));
    settings.protected_ids[7] = true;
    assert!(
        next.propose_physical_stroke(&settings, &[[4.; 3]], || false)
            .is_err()
    );
}

#[test]
fn apply_requires_exact_base_and_all_current_edit_settings_and_preserves_noop_history() {
    let base = Arc::new(Labels::empty(source([9; 3], 0, false)).unwrap());
    let settings = brush();
    let points = [[4.; 3]];
    let build = || {
        base.propose_physical_stroke(&settings, &points, || false)
            .unwrap()
    };
    let p = build();
    let preview = p.preview().clone();
    assert!(p.is_current(&base, &settings, &points));
    let checkpoint = Arc::new(base.try_clone(|| false).unwrap());
    assert_eq!(checkpoint.revision(), base.revision());
    assert!(!p.is_current(&checkpoint, &settings, &points));
    assert!(
        build()
            .apply(&checkpoint, &settings, &points, || false)
            .is_err()
    );
    let replacement = Arc::new(Labels::empty(base.source_arc()).unwrap());
    assert!(!p.is_current(&replacement, &settings, &points));
    let other_source = Arc::new(Labels::empty(source([9; 3], 0, false)).unwrap());
    assert!(!p.is_current(&other_source, &settings, &points));
    for field in 0..7 {
        let mut changed = settings.clone();
        match field {
            0 => changed.label = 8,
            1 => changed.radius_mm = 2.,
            2 => changed.action = BrushAction::Erase,
            3 => {
                changed.shape = BrushShape::Plane {
                    origin_mm: [0.; 3],
                    normal: [0., 0., 1.],
                    thickness_mm: 1.,
                }
            }
            4 => changed.protected_ids[9] = true,
            5 => changed.protected_ids[0] = true,
            _ => {
                changed.clip = Some(PhysicalClip {
                    minimum_mm: [0.; 3],
                    maximum_mm: [8.; 3],
                })
            }
        }
        assert!(!p.is_current(&base, &changed, &points));
        assert!(build().apply(&base, &changed, &points, || false).is_err());
    }
    assert!(build().apply(&base, &settings, &points, || true).is_err());
    let next = p.apply(&base, &settings, &points, || false).unwrap();
    assert!(Arc::ptr_eq(&next, &preview));
    assert!(Arc::ptr_eq(&next.source_arc(), &base.source_arc()));
    assert_eq!(base.value([4; 3]), Some(0));
    let noop = next
        .propose_physical_stroke(&settings, &points, || false)
        .unwrap();
    assert!(Arc::ptr_eq(noop.preview(), &next));
    assert_eq!(noop.summary().changed_voxels, 0);
    assert_eq!(noop.summary().changed_bounds, None);
    assert_eq!(noop.changed_tiles().count(), 0);
    let same = noop.apply(&next, &settings, &points, || false).unwrap();
    assert!(Arc::ptr_eq(&same, &next));
    assert_eq!(same.parent_revision(), Some(base.revision()));
}

#[test]
fn changed_ordered_input_paths_cannot_apply_an_older_prepared_preview() {
    let base = Arc::new(Labels::empty(source([9; 3], 0, false)).unwrap());
    let settings = brush();
    let original = [[2., 3., 4.], [6., 3., 4.]];
    let mut input = original.to_vec();
    let p = base
        .propose_physical_stroke(&settings, &input, || false)
        .unwrap();
    assert_eq!(p.points_mm(), original);
    input[1] = [7., 3., 4.];
    assert_eq!(
        p.points_mm(),
        original,
        "The proposal owns its immutable path"
    );
    for changed in [
        input,
        vec![original[1], original[0]],
        vec![original[0]],
        vec![],
        vec![[f64::NAN; 3]],
    ] {
        assert!(!p.is_current(&base, &settings, &changed));
        let old = base
            .propose_physical_stroke(&settings, &original, || false)
            .unwrap();
        assert!(old.apply(&base, &settings, &changed, || false).is_err());
        assert!(base.values().iter().all(|v| *v == 0));
    }
    let preview = p.preview().clone();
    let next = p.apply(&base, &settings, &original, || false).unwrap();
    assert!(Arc::ptr_eq(&next, &preview));
    assert_eq!(base.value([4, 3, 4]), Some(0));
    assert_eq!(next.value([4, 3, 4]), Some(7));
}

#[test]
fn cancelled_or_unadmitted_private_strokes_release_resources_without_partial_base_edits() {
    let budget = MemoryBudget::new(4 * 1024 * 1024);
    let base =
        Arc::new(Labels::empty_with_budget(source([67, 35, 5], 0, false), budget.clone()).unwrap());
    let before = budget.used();
    let mut settings = brush();
    settings.radius_mm = 256.;
    let points = [[30., 15., 2.]];
    for at in [0, 1, 2, 3, 4, 5, 12, 40, 150] {
        let calls = Cell::new(0);
        let outcome = base.propose_physical_stroke(&settings, &points, || {
            let n = calls.get();
            calls.set(n + 1);
            n >= at
        });
        assert!(outcome.is_err());
        assert_eq!(budget.used(), before);
        assert!(base.values().iter().all(|v| *v == 0));
    }
    let p = base
        .propose_physical_stroke(&settings, &points, || false)
        .unwrap();
    let peak = budget.used();
    drop(p);
    assert_eq!(budget.used(), before);
    for limit in [before, before + 100, before + 1024, peak - 1] {
        budget.set_limit(limit);
        assert!(
            base.propose_physical_stroke(&settings, &points, || false)
                .is_err()
        );
        assert_eq!(budget.used(), before);
    }
    budget.set_limit(4 * 1024 * 1024);
    let cut = Cell::new(false);
    let overhead = (std::mem::size_of::<PhysicalStroke>()
        + std::mem::size_of::<Segment>()
        + std::mem::size_of_val(&points)) as u64
        + base.storage_statistics().directory_bytes;
    assert!(
        base.propose_physical_stroke(&settings, &points, || {
            if budget.used() > before + overhead && !cut.replace(true) {
                budget.set_limit(budget.used());
            }
            false
        })
        .is_err()
    );
    assert!(cut.get());
    assert_eq!(budget.used(), before);
    budget.set_limit(4 * 1024 * 1024);
    let result = base
        .propose_physical_stroke(&settings, &points, || false)
        .unwrap()
        .apply(&base, &settings, &points, || false)
        .unwrap();
    assert_eq!(result.changed_tiles().count(), 6);
    drop(result);
    assert_eq!(budget.used(), before);
    drop(base);
    assert_eq!(budget.used(), 0);
}

#[test]
fn bounded_preflight_refuses_invalid_inputs_and_excess_work_without_editing() {
    let budget = MemoryBudget::new(4 * 1024 * 1024);
    let base =
        Arc::new(Labels::empty_with_budget(source([67, 35, 5], 0, false), budget.clone()).unwrap());
    let before = budget.used();
    for r in [f64::NAN, f64::INFINITY, 0., -1., 1e-12, 256.001] {
        let mut s = brush();
        s.radius_mm = r;
        assert!(
            base.propose_physical_stroke(&s, &[[0.; 3]], || false)
                .is_err()
        );
    }
    let mut s = brush();
    for points in [
        &[][..],
        &[[f64::NAN, 0., 0.]][..],
        &[[f64::INFINITY, 0., 0.]][..],
        &[[1e10, 0., 0.]][..],
    ] {
        assert!(base.propose_physical_stroke(&s, points, || false).is_err());
    }
    assert!(
        base.propose_physical_stroke(&s, &vec![[0.; 3]; MAX_STROKE_POINTS + 1], || false)
            .is_err()
    );
    for (normal, t) in [
        ([0.; 3], 1.),
        ([f64::NAN, 0., 1.], 1.),
        ([0., 0., 1.], 0.),
        ([0., 0., 1.], 1e-12),
        ([0., 0., 1.], 513.),
    ] {
        s.shape = BrushShape::Plane {
            origin_mm: [0.; 3],
            normal,
            thickness_mm: t,
        };
        assert!(
            base.propose_physical_stroke(&s, &[[0.; 3]], || false)
                .is_err()
        );
    }
    s = brush();
    s.radius_mm = 256.;
    let many = vec![[20., 20., 2.]; MAX_STROKE_POINTS];
    let err = base
        .propose_physical_stroke(&s, &many, || false)
        .err()
        .unwrap();
    assert!(err.contains("too large"));
    s = brush();
    s.clip = Some(PhysicalClip {
        minimum_mm: [1.; 3],
        maximum_mm: [0.; 3],
    });
    assert!(
        base.propose_physical_stroke(&s, &[[0.; 3]], || false)
            .is_err()
    );
    assert_eq!(budget.used(), before);
    assert!(base.values().iter().all(|v| *v == 0));
}

#[test]
fn numeric_pruning_fallback_and_empty_outside_strokes_keep_the_same_membership_rules() {
    let base = Arc::new(Labels::empty(source([9; 3], 0, false)).unwrap());
    let mut s = brush();
    s.radius_mm = 2.;
    let p = base
        .propose_physical_stroke(&s, &[[-30.; 3]], || false)
        .unwrap();
    assert_eq!(p.summary().candidate_visits, 0);
    assert_eq!(p.summary().changed_voxels, 0);
    s.shape = BrushShape::Plane {
        origin_mm: [1e9, -1e9, 4.],
        normal: [0., 0., 1.],
        thickness_mm: 1.,
    };
    let p = base
        .propose_physical_stroke(&s, &[[4.; 3]], || false)
        .unwrap();
    assert_eq!(p.summary().candidate_visits, 729);
    assert_eq!(p.summary().changed_voxels, 13);
    for z in 0..9 {
        for y in 0..9 {
            for x in 0..9 {
                assert_eq!(
                    p.value([x, y, z]),
                    Some(
                        if z == 4 && (x as f64 - 4.).powi(2) + (y as f64 - 4.).powi(2) <= 4. {
                            7
                        } else {
                            0
                        }
                    )
                );
            }
        }
    }
}

#[test]
fn scalar_validity_and_color_alpha_gate_the_same_physical_edits() {
    let cpu = source([5; 3], 0, false);
    for color in [false, true] {
        let frames = cpu
            .frames()
            .iter()
            .map(|f| {
                let mut f = (**f).clone();
                if color {
                    f.pixels = Arc::from([]);
                    let mut rgba = vec![255; 100];
                    rgba[12 * 4 + 3] = 0;
                    rgba[7 * 4 + 3] = 0;
                    f.rgba = Some(rgba.into());
                } else {
                    let mut valid = vec![1; 25];
                    valid[12] = 0;
                    valid[7] = 0;
                    f.validity = Some(valid.into());
                }
                Arc::new(f)
            })
            .collect();
        let base = Arc::new(Labels::empty(Arc::new(Volume::new(frames).unwrap())).unwrap());
        let mut s = brush();
        s.radius_mm = 100.;
        let p = base
            .propose_physical_stroke(&s, &[[2.; 3]], || false)
            .unwrap();
        assert_eq!(p.summary().changed_voxels, 23 * 5);
        for z in 0..5 {
            assert_eq!(p.value([2, 2, z]), Some(0));
            assert_eq!(p.value([2, 1, z]), Some(0));
            assert_eq!(p.value([1, 1, z]), Some(7));
        }
    }
}

#[test]
fn scaled_extreme_plane_normals_keep_the_same_direction_without_overflow_or_underflow() {
    let base = Arc::new(Labels::empty(source([9; 3], 0, false)).unwrap());
    let mut s = brush();
    s.shape = BrushShape::Plane {
        origin_mm: [4.; 3],
        normal: [1., 1., 0.],
        thickness_mm: 1.,
    };
    let reference = base
        .propose_physical_stroke(&s, &[[4.; 3]], || false)
        .unwrap();
    for size in [f64::from_bits(1), 1e-300, 1e300, 1e308] {
        s.shape = BrushShape::Plane {
            origin_mm: [4.; 3],
            normal: [size, size, 0.],
            thickness_mm: 1.,
        };
        let p = base
            .propose_physical_stroke(&s, &[[4.; 3]], || false)
            .unwrap();
        for z in 0..9 {
            for y in 0..9 {
                for x in 0..9 {
                    assert_eq!(p.value([x, y, z]), reference.value([x, y, z]));
                }
            }
        }
    }
}

#[test]
fn physical_edits_and_undo_preserve_incremental_surface_owners_and_exact_faces() {
    let points = [[31., 16., 2.]];
    use crate::{
        mask::MaskOwner,
        surface::{CellSurface, SurfaceLimits},
    };
    let mut labels = Labels::empty(source([129, 33, 5], 0, false)).unwrap();
    labels.set_value((2 * 33 + 16) * 129 + 112, 7).unwrap();
    let base = Arc::new(labels);
    let build = |map: Arc<Labels>, previous: Option<&Arc<CellSurface>>| {
        CellSurface::build(
            MaskOwner::Manual(map),
            7,
            previous,
            SurfaceLimits::default(),
            || false,
        )
        .unwrap()
    };
    let faces = |s: &CellSurface| {
        let mut f = s
            .chunks()
            .flat_map(|c| c.faces().iter().copied())
            .collect::<Vec<_>>();
        f.sort();
        f
    };
    let (old, _) = build(base.clone(), None);
    let mut s = brush();
    s.radius_mm = 2.3;
    let next = base
        .propose_physical_stroke(&s, &[[31., 16., 2.]], || false)
        .unwrap()
        .apply(&base, &s, &points, || false)
        .unwrap();
    assert_eq!(next.changed_tiles().count(), 2);
    let (updated, delta) = build(next.clone(), Some(&old));
    let (fresh, _) = build(next.clone(), None);
    assert!(delta.reused_chunks > 0);
    assert!(Arc::ptr_eq(
        old.chunk([3, 0, 0]).unwrap(),
        updated.chunk([3, 0, 0]).unwrap()
    ));
    assert_eq!(faces(&updated), faces(&fresh));
    assert_eq!(updated.revision(), next.revision());
    let (undo, _) = build(base.clone(), Some(&updated));
    assert_eq!(faces(&undo), faces(&old));
    s.action = BrushAction::Erase;
    let erased = next
        .propose_physical_stroke(&s, &[[31., 16., 2.]], || false)
        .unwrap()
        .apply(&next, &s, &points, || false)
        .unwrap();
    let (erased_surface, _) = build(erased, Some(&updated));
    assert_eq!(faces(&erased_surface), faces(&old));
    assert_eq!(old.face_count(), 6);
    assert_eq!(base.value([31, 16, 2]), Some(0));
}
