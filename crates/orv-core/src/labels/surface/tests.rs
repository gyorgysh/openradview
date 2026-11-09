// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use crate::{
    mask::MaskOwner,
    resources::MemoryBudget,
    study::{Geometry, demo_study},
    surface::{CellSurface, FaceSide, SurfaceLimits},
    vector::*,
    volume::Volume,
    volume_view::PhysicalClip,
};
use std::{cell::Cell, collections::BTreeSet};

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
    Arc::new(
        Volume::new(
            (0..dims[2])
                .map(|z| {
                    let mut f = (*template).clone();
                    f.width = dims[0] as u32;
                    f.height = dims[1] as u32;
                    f.pixels = vec![0.; dims[0] * dims[1]].into();
                    f.validity = missing.then(|| {
                        (0..dims[0] * dims[1])
                            .map(|i| {
                                u8::from(!matches!(
                                    [i % dims[0], i / dims[0], z],
                                    [4, 4, 1] | [3, 4, 0]
                                ))
                            })
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
                        frame_of_reference: Some("generated face erase".into()),
                        nifti_transform: None,
                    });
                    Arc::new(f)
                })
                .collect(),
        )
        .unwrap(),
    )
}
fn map(
    source: Arc<Volume>,
    budget: Arc<MemoryBudget>,
    value: impl Fn([usize; 3]) -> u8,
) -> Arc<Labels> {
    let [w, h, d] = source.dimensions();
    let mut map = Labels::empty_with_budget(source, budget).unwrap();
    for z in 0..d {
        for y in 0..h {
            for x in 0..w {
                map.set_value((z * h + y) * w + x, value([x, y, z]))
                    .unwrap();
            }
        }
    }
    Arc::new(map)
}
fn faces(map: &Arc<Labels>, number: u16) -> Vec<CellFace> {
    CellSurface::build(
        MaskOwner::Manual(map.clone()),
        number,
        None,
        SurfaceLimits::default(),
        || false,
    )
    .unwrap()
    .0
    .chunks()
    .flat_map(|c| c.faces().iter().copied())
    .collect()
}
fn brush() -> PhysicalBrush {
    PhysicalBrush {
        label: 7,
        action: BrushAction::Erase,
        radius_mm: 2.05,
        shape: BrushShape::Sphere,
        clip: None,
        protected_ids: [false; 256],
    }
}
fn propose(
    map: &Arc<Labels>,
    settings: &PhysicalBrush,
    points: &[V3],
    faces: &[CellFace],
    cancelled: impl Fn() -> bool,
) -> Result<FaceErase, &'static str> {
    let view = MaskView::manual(map, settings.label)?;
    map.propose_face_erase(
        settings,
        points,
        FaceEraseSelection {
            key: &view.key(),
            revision: view.revision(),
            faces,
        },
        cancelled,
    )
}
// Cross-product distance is independent of the production closest-point formula.
fn capsule(p: V3, a: V3, b: V3, radius: f64) -> bool {
    let d = sub(b, a);
    let q = sub(p, a);
    let t = dot(q, d);
    let length = norm(d);
    if length == 0. || t <= 0. {
        norm(q) <= radius
    } else if t >= length * length {
        norm(sub(p, b)) <= radius
    } else {
        norm(cross(q, d)) <= radius * length
    }
}

#[test]
fn selected_face_erase_matches_independent_affine_sweeps_and_preserves_hidden_layers() {
    for kind in 0..3 {
        for missing in [false, true] {
            for clipped in [false, true] {
                let dims = [35, 9, 7];
                let budget = MemoryBudget::new(16 * 1024 * 1024);
                let map = map(source(dims, kind, missing), budget.clone(), |[x, y, z]| {
                    if (1..=33).contains(&x) && (1..=7).contains(&y) && (1..=5).contains(&z) {
                        7
                    } else if x == 34 {
                        4
                    } else {
                        0
                    }
                });
                let mut selected = faces(&map, 7);
                selected.retain(|f| matches!(f.side(), FaceSide::ZMinus | FaceSide::XMinus));
                let unique: BTreeSet<_> = selected.iter().map(CellFace::cell).collect();
                selected.extend_from_within(..);
                selected.reverse();
                let points = [
                    world(kind, [0.8, 3.2, 1.1]),
                    world(kind, [33.2, 3.2, 1.1]),
                    world(kind, [33.2, 3.2, 1.1]),
                ];
                let mut settings = brush();
                settings.protected_ids[4] = true;
                settings.protected_ids[0] = true;
                if clipped {
                    let p = world(kind, [17., 3., 1.]);
                    settings.clip = Some(PhysicalClip {
                        minimum_mm: p.map(|v| v - 8.),
                        maximum_mm: p.map(|v| v + 8.),
                    });
                }
                let before_bytes = budget.used();
                let proposal = propose(&map, &settings, &points, &selected, || false).unwrap();
                let mut expected = Vec::new();
                for z in 0..dims[2] {
                    for y in 0..dims[1] {
                        for x in 0..dims[0] {
                            let cell = [x, y, z];
                            let p = world(kind, cell.map(|v| v as f64));
                            let inside_clip = settings.clip.as_ref().is_none_or(|c| {
                                (0..3).all(|a| p[a] >= c.minimum_mm[a] && p[a] <= c.maximum_mm[a])
                            });
                            let erased = unique.contains(&cell.map(|v| v as u32))
                                && inside_clip
                                && points
                                    .windows(2)
                                    .any(|s| capsule(p, s[0], s[1], settings.radius_mm));
                            assert_eq!(
                                proposal.preview().value(cell),
                                if erased { Some(0) } else { map.value(cell) },
                                "kind={kind} missing={missing} clipped={clipped} cell={cell:?}"
                            );
                            if erased {
                                expected.push(cell);
                            }
                        }
                    }
                }
                assert!(!expected.is_empty());
                assert_eq!(proposal.summary().changed_voxels, expected.len());
                assert_eq!(proposal.summary().candidate_faces, selected.len());
                assert_eq!(proposal.summary().candidate_tests, selected.len() * 2);
                let lo = std::array::from_fn(|a| expected.iter().map(|p| p[a]).min().unwrap());
                let hi = std::array::from_fn(|a| expected.iter().map(|p| p[a] + 1).max().unwrap());
                assert_eq!(proposal.summary().changed_bounds, Some([lo, hi]));
                assert_eq!(proposal.preview().parent_revision(), Some(map.revision()));
                assert_eq!(proposal.preview().value([17, 3, 2]), Some(7));
                if missing {
                    assert_eq!(proposal.preview().value([4, 4, 1]), Some(7));
                    assert_eq!(proposal.preview().value([3, 4, 1]), Some(7));
                }
                assert_eq!(map.value([17, 3, 1]), Some(7));
                let changed_tiles = proposal.changed_tiles().count();
                assert_eq!(proposal.summary().changed_tiles, changed_tiles);
                assert!(changed_tiles > 0);
                assert_eq!(proposal.faces(), selected);
                assert_eq!(proposal.points_mm(), points);
                assert!(Arc::ptr_eq(proposal.base(), &map));
                assert_eq!(proposal.settings(), &settings);
                let result = proposal.apply(&map, &settings, &points, || false).unwrap();
                assert_eq!(result.changed_tiles().count(), changed_tiles);
                assert_ne!(result.revision(), map.revision());
                drop(result);
                assert_eq!(budget.used(), before_bytes);
            }
        }
    }
}

#[test]
fn grid_faces_duplicates_noop_and_empty_selections_keep_exact_cow_identity() {
    let budget = MemoryBudget::new(1024 * 1024);
    let map = map(source([5; 3], 0, false), budget.clone(), |_| 7);
    let selected: Vec<_> = faces(&map, 7)
        .into_iter()
        .filter(|f| f.cell() == [0, 0, 0])
        .collect();
    assert_eq!(selected.len(), 3);
    assert!(selected.iter().all(|f| f.boundary() == FaceBoundary::Grid));
    let points = [[0.; 3]];
    let settings = brush();
    let p = propose(&map, &settings, &points, &selected, || false).unwrap();
    assert_eq!(p.summary().changed_voxels, 1);
    assert_eq!(p.summary().candidate_tests, 3);
    let result = p.apply(&map, &settings, &points, || false).unwrap();
    assert_eq!(result.value([0; 3]), Some(0));
    assert_eq!(result.value([1, 0, 0]), Some(7));
    drop(result);
    let held = budget.used();
    for selection in [selected.as_slice(), &[]] {
        let points = [[100.; 3]];
        let p = propose(&map, &settings, &points, selection, || false).unwrap();
        assert_eq!(p.summary().changed_voxels, 0);
        assert_eq!(p.summary().changed_tiles, 0);
        assert_eq!(p.summary().changed_bounds, None);
        assert_eq!(p.changed_tiles().count(), 0);
        assert!(Arc::ptr_eq(&map, p.preview()));
        let result = p.apply(&map, &settings, &points, || false).unwrap();
        assert!(Arc::ptr_eq(&map, &result));
        drop(result);
        assert_eq!(budget.used(), held);
    }
}

#[test]
fn candidate_identity_known_boundaries_and_source_validity_are_rechecked() {
    let budget = MemoryBudget::new(1024 * 1024);
    let volume = source([7; 3], 0, false);
    let make = |source| {
        map(
            source,
            budget.clone(),
            |p| if p == [3, 4, 1] { 7 } else { 0 },
        )
    };
    let base = make(volume.clone());
    let selected = faces(&base, 7);
    let face = *selected
        .iter()
        .find(|f| f.side() == FaceSide::ZMinus)
        .unwrap();
    let old = MaskView::manual(&base, 7).unwrap();
    let points = [[3., 4., 1.]];
    let settings = brush();
    let other = make(volume.clone());
    assert!(
        other
            .propose_face_erase(
                &settings,
                &points,
                FaceEraseSelection {
                    key: &old.key(),
                    revision: old.revision(),
                    faces: &selected,
                },
                || false
            )
            .is_err()
    );
    let mut edited = base.try_clone(|| false).unwrap();
    edited.set_value(0, 4).unwrap();
    assert!(
        Arc::new(edited)
            .propose_face_erase(
                &settings,
                &points,
                FaceEraseSelection {
                    key: &old.key(),
                    revision: old.revision(),
                    faces: &selected,
                },
                || false
            )
            .is_err()
    );
    let wrong = MaskView::manual(&base, 4).unwrap();
    assert!(
        base.propose_face_erase(
            &settings,
            &points,
            FaceEraseSelection {
                key: &wrong.key(),
                revision: wrong.revision(),
                faces: &selected,
            },
            || false
        )
        .is_err()
    );
    // Relabelling a descriptor with a different map's key is not enough: its
    // selected cell and known neighbour must still agree with current membership.
    for (voxel, value) in [([3, 4, 1], 4), ([3, 4, 0], 7)] {
        let mut altered = base.try_clone(|| false).unwrap();
        let [x, y, z] = voxel;
        altered.set_value((z * 7 + y) * 7 + x, value).unwrap();
        assert!(propose(&Arc::new(altered), &settings, &points, &[face], || false).is_err());
    }
    let invalid = make(source([7; 3], 0, true));
    assert!(propose(&invalid, &settings, &points, &[face], || false).is_err());
    let valid_cell = map(
        volume,
        budget.clone(),
        |p| {
            if p == [4, 4, 1] { 7 } else { 0 }
        },
    );
    let invalid_cell_face = faces(&valid_cell, 7)[0];
    let invalid_cell = map(source([7; 3], 0, true), budget.clone(), |p| {
        if p == [4, 4, 1] { 7 } else { 0 }
    });
    assert!(
        propose(
            &invalid_cell,
            &settings,
            &points,
            &[invalid_cell_face],
            || false
        )
        .is_err()
    );
    let bigger = map(source([9; 3], 0, false), budget.clone(), |p| {
        if p == [8; 3] { 7 } else { 0 }
    });
    let outside = faces(&bigger, 7);
    assert!(propose(&base, &settings, &points, &outside, || false).is_err());
    assert_eq!(base.value([3, 4, 1]), Some(7));
}

#[test]
fn apply_requires_exact_base_settings_path_and_live_cancellation() {
    let budget = MemoryBudget::new(1024 * 1024);
    let map = map(source([5; 3], 0, false), budget, |p| {
        u8::from(p == [2; 3]) * 7
    });
    let selected = faces(&map, 7);
    let settings = brush();
    let points = [[2.; 3]];
    let copied = Arc::new(map.try_clone(|| false).unwrap());
    let p = propose(&map, &settings, &points, &selected, || false).unwrap();
    assert!(p.is_current(&map, &settings, &points));
    assert!(!p.is_current(&copied, &settings, &points));
    assert!(p.apply(&copied, &settings, &points, || false).is_err());
    for change in 0..5 {
        let mut altered = settings.clone();
        match change {
            0 => altered.radius_mm += 1.,
            1 => altered.protected_ids[4] = true,
            2 => {
                altered.clip = Some(PhysicalClip {
                    minimum_mm: [0.; 3],
                    maximum_mm: [4.; 3],
                })
            }
            3 => altered.label = 4,
            _ => altered.action = BrushAction::Add,
        }
        let p = propose(&map, &settings, &points, &selected, || false).unwrap();
        assert!(!p.is_current(&map, &altered, &points));
        assert!(p.apply(&map, &altered, &points, || false).is_err());
    }
    let p = propose(&map, &settings, &points, &selected, || false).unwrap();
    assert!(
        p.apply(&map, &settings, &[[2., 2., 2.01]], || false)
            .is_err()
    );
    let p = propose(&map, &settings, &points, &selected, || false).unwrap();
    assert!(p.apply(&map, &settings, &points, || true).is_err());
    assert_eq!(map.value([2; 3]), Some(7));
}

#[test]
fn bounds_invalid_input_and_work_limits_refuse_before_publication() {
    let budget = MemoryBudget::new(1024 * 1024);
    let map = map(source([5; 3], 0, false), budget.clone(), |p| {
        u8::from(p == [2; 3]) * 7
    });
    let selected = faces(&map, 7);
    let points = [[2.; 3]];
    let held = budget.used();
    for change in 0..7 {
        let mut settings = brush();
        match change {
            0 => settings.radius_mm = f64::NAN,
            1 => settings.radius_mm = 0.,
            2 => settings.label = 0,
            3 => settings.action = BrushAction::Add,
            4 => settings.protected_ids[7] = true,
            5 => {
                settings.shape = BrushShape::Plane {
                    origin_mm: [0.; 3],
                    normal: [0., 0., 1.],
                    thickness_mm: 1.,
                }
            }
            _ => {
                settings.clip = Some(PhysicalClip {
                    minimum_mm: [3.; 3],
                    maximum_mm: [1.; 3],
                })
            }
        }
        assert!(propose(&map, &settings, &points, &selected, || false).is_err());
        assert_eq!(budget.used(), held);
    }
    let settings = brush();
    for points in [
        vec![],
        vec![[f64::NAN; 3]],
        vec![[f64::INFINITY; 3]],
        vec![[1e10; 3]],
        vec![[2.; 3]; MAX_STROKE_POINTS + 1],
    ] {
        assert!(propose(&map, &settings, &points, &selected, || false).is_err());
        assert_eq!(budget.used(), held);
    }
    let too_many = vec![selected[0]; MAX_ERASE_FACES + 1];
    assert!(propose(&map, &settings, &points, &too_many, || false).is_err());
    let overwork = vec![selected[0]; MAX_ERASE_FACES];
    assert!(propose(&map, &settings, &vec![[2.; 3]; 1026], &overwork, || false).is_err());
    assert_eq!(budget.used(), held);
}

#[test]
fn cancellation_and_admission_failure_release_every_private_change_and_retry() {
    let budget = MemoryBudget::new(16 * 1024 * 1024);
    let map = map(source([65, 9, 7], 0, false), budget.clone(), |_| 7);
    let selected: Vec<_> = faces(&map, 7)
        .into_iter()
        .filter(|f| f.side() == FaceSide::ZMinus)
        .collect();
    let mut settings = brush();
    settings.radius_mm = 80.;
    let points = [[32., 4., 0.]];
    let held = budget.used();
    let calls = Cell::new(0);
    let good = propose(&map, &settings, &points, &selected, || {
        calls.set(calls.get() + 1);
        false
    })
    .unwrap();
    assert_eq!(good.summary().changed_voxels, 65 * 9);
    drop(good);
    assert_eq!(budget.used(), held);
    for stop in 0..calls.get() {
        let count = Cell::new(0);
        let result = propose(&map, &settings, &points, &selected, || {
            let n = count.get();
            count.set(n + 1);
            n == stop
        });
        assert!(result.is_err(), "cancellation callback {stop}");
        assert_eq!(budget.used(), held, "cancellation callback {stop}");
        assert_eq!(map.value([32, 4, 0]), Some(7));
    }
    let mut failures = 0;
    for headroom in [0, 64, 512, 2048, 16384, 65536] {
        budget.set_limit(held + headroom);
        let result = propose(&map, &settings, &points, &selected, || false);
        if result.is_err() {
            failures += 1;
        }
        drop(result);
        assert_eq!(budget.used(), held);
        assert_eq!(map.value([32, 4, 0]), Some(7));
    }
    assert!(failures >= 4);
    budget.set_limit(16 * 1024 * 1024);
    let good = propose(&map, &settings, &points, &selected, || false).unwrap();
    assert_eq!(good.summary().changed_voxels, 65 * 9);
    drop(good);
    assert_eq!(budget.used(), held);
}
