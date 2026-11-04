// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use crate::{mask::MaskView, volume_view::PhysicalClip};

fn hit(
    view: MaskView<'_>,
    origin: [f64; 3],
    direction: [f64; 3],
    range: [f64; 2],
    clip: Option<&PhysicalClip>,
) -> Result<Option<InterfaceHit>, InterfaceRayError> {
    InterfaceRay::new(view, origin, direction, range, clip)?.first_hit(|| false)
}
fn cell_interval(
    source: &Volume,
    cell: [u32; 3],
    origin: V3,
    direction: V3,
    mut range: [f64; 2],
    clip: Option<&PhysicalClip>,
) -> Option<[f64; 2]> {
    let o = source.world_to_voxel(origin);
    let d = source.world_vector_to_voxel(direction);
    for a in 0..3 {
        let lo = f64::from(cell[a]) - 0.5;
        let hi = lo + 1.;
        if d[a] == 0. {
            if o[a] < lo || o[a] >= hi {
                return None;
            }
        } else {
            let p = (lo - o[a]) / d[a];
            let q = (hi - o[a]) / d[a];
            range[0] = range[0].max(p.min(q));
            range[1] = range[1].min(p.max(q));
        }
    }
    if let Some(clip) = clip {
        let q = clip.interval(origin, direction)?;
        range[0] = range[0].max(q[0]);
        range[1] = range[1].min(q[1]);
    }
    (range[0] < range[1]).then_some(range)
}
// Independent world-plane and parallelogram intersections. No CellRay, boundary
// time ordering, index-face plane equation or new interface classifier is used.
fn brute(
    surface: &CellSurface,
    origin: V3,
    direction: V3,
    range: [f64; 2],
    clip: Option<&PhysicalClip>,
) -> Option<(CellFace, f64, f64)> {
    let mut nearest = None;
    for face in surface.chunks().flat_map(|c| c.faces()) {
        let geometry = surface.physical_face(face).unwrap();
        let denominator = dot(geometry.normal, direction);
        if denominator == 0. {
            continue;
        }
        let t = dot(geometry.normal, sub(geometry.corners_mm[0], origin)) / denominator;
        if t < range[0] || t > range[1] {
            continue;
        }
        let point = add(origin, mul(direction, t));
        if clip.is_some_and(|clip| {
            (0..3).any(|a| point[a] < clip.minimum_mm[a] || point[a] > clip.maximum_mm[a])
        }) {
            continue;
        }
        let a = sub(geometry.corners_mm[1], geometry.corners_mm[0]);
        let b = sub(geometry.corners_mm[3], geometry.corners_mm[0]);
        let p = sub(point, geometry.corners_mm[0]);
        let aa = dot(a, a);
        let ab = dot(a, b);
        let bb = dot(b, b);
        let determinant = aa * bb - ab * ab;
        let u = (dot(p, a) * bb - dot(p, b) * ab) / determinant;
        let v = (dot(p, b) * aa - dot(p, a) * ab) / determinant;
        let edge = u.min(v).min(1. - u).min(1. - v);
        if edge < -1e-9 {
            continue;
        }
        let Some(occupied) = cell_interval(
            surface.source(),
            face.cell(),
            origin,
            direction,
            range,
            clip,
        ) else {
            continue;
        };
        if t < occupied[0] - 1e-8 || t > occupied[1] + 1e-8 {
            continue;
        }
        if nearest.is_none_or(|(_, old, _)| t < old) {
            nearest = Some((*face, t, edge));
        }
    }
    nearest
}

#[test]
fn bounded_interfaces_match_independent_world_faces_for_affine_masks_and_clips() {
    for shear in [false, true] {
        let source = source([5; 3], shear, false);
        for pattern in 0..3 {
            let map = labels(source.clone(), |[x, y, z]| {
                u8::from(match pattern {
                    0 => (1..=3).contains(&x) && (1..=3).contains(&y) && (1..=3).contains(&z),
                    1 => (x + 2 * y + 3 * z) % 7 < 4,
                    _ => x == 2 || z == 2,
                })
            });
            let (surface, _) = build(MaskOwner::Manual(map.clone()), 1, None);
            let view = MaskView::manual(&map, 1).unwrap();
            let center = source.center();
            for clip in [
                None,
                Some(PhysicalClip {
                    minimum_mm: center.map(|v| v - 1.7),
                    maximum_mm: center.map(|v| v + 1.7),
                }),
            ] {
                for i in 0..193usize {
                    let origin = source.voxel_to_world([
                        -2.125,
                        (i * 37 % 127) as f64 / 19. - 1.137,
                        (i * 53 % 131) as f64 / 23. - 0.213,
                    ]);
                    let d = source.voxel_vector_to_world([
                        1.,
                        ((i * 11 % 31) as f64 - 15.3) / 27.,
                        ((i * 17 % 37) as f64 - 18.7) / 31.,
                    ]);
                    let direction = mul(d, 1. / norm(d));
                    let range = [-100., 100.];
                    let expected = brute(&surface, origin, direction, range, clip.as_ref());
                    let actual = hit(view, origin, direction, range, clip.as_ref());
                    match (expected, actual) {
                        (None, Ok(None)) => {}
                        (Some((face, t, edge)), Ok(Some(actual))) => {
                            assert!(
                                (actual.parameter_mm - t).abs() < 1e-8,
                                "{actual:?} versus {face:?}/{t}"
                            );
                            if edge > 1e-7 {
                                assert_eq!(actual.face, face);
                            }
                            assert_eq!(actual.key, view.key());
                            assert_eq!(&actual.revision, view.revision());
                            assert!(
                                norm(sub(actual.position_mm, add(origin, mul(direction, t))))
                                    < 1e-8
                            );
                        }
                        (Some((_, _, edge)), Err(InterfaceRayError::Ambiguous)) => {
                            assert!(edge < 1e-7)
                        }
                        other => panic!("Ray {i}, pattern{pattern}, shear{shear}: {other:?}"),
                    }
                }
            }
        }
    }
}

#[test]
fn grid_faces_negative_parameters_and_clip_or_range_entries_preserve_real_boundaries() {
    let source = source([5; 3], false, false);
    let map = labels(source, |_| 1);
    let view = MaskView::manual(&map, 1).unwrap();
    let front = hit(view, [2.; 3], [0., 1., 0.], [-100., 100.], None)
        .unwrap()
        .unwrap();
    assert_eq!(front.face.side(), FaceSide::YMinus);
    assert_eq!(front.face.boundary(), FaceBoundary::Grid);
    assert_eq!(front.parameter_mm, -2.5);
    let clip = PhysicalClip {
        minimum_mm: [-10., 1.125, -10.],
        maximum_mm: [10.; 3],
    };
    let back = hit(view, [2.; 3], [0., 1., 0.], [-100., 100.], Some(&clip))
        .unwrap()
        .unwrap();
    assert_eq!(back.face.side(), FaceSide::YPlus);
    assert_eq!(back.parameter_mm, 2.5);
    let range = hit(view, [2.; 3], [0., 1., 0.], [0., 100.], None)
        .unwrap()
        .unwrap();
    assert_eq!(range.face, back.face);
    assert!(
        hit(view, [2.; 3], [0., 1., 0.], [-0.2, 0.2], None)
            .unwrap()
            .is_none()
    );
    let inside = PhysicalClip {
        minimum_mm: [1.; 3],
        maximum_mm: [3.; 3],
    };
    assert!(
        hit(view, [2.; 3], [0., 1., 0.], [-100., 100.], Some(&inside))
            .unwrap()
            .is_none()
    );
    assert!(
        hit(view, [20.; 3], [0., 1., 0.], [-100., 100.], None)
            .unwrap()
            .is_none()
    );
}

#[test]
fn exact_edges_corners_and_tangents_never_become_unambiguous_picks() {
    let source = source([5; 3], false, false);
    let map = labels(source, |p| u8::from(p == [2; 3]));
    let view = MaskView::manual(&map, 1).unwrap();
    for (origin, direction) in [([2., 2., -2.], [0., 0., 1.]), ([2., -2., 2.], [0., 1., 0.])] {
        assert!(
            hit(view, origin, direction, [-100., 100.], None)
                .unwrap()
                .is_some()
        );
    }
    for (origin, direction) in [
        ([1.5, 2., -2.], [0., 0., 1.]),
        ([1.5, 1.5, -2.], [0., 0., 1.]),
        ([-2., -2., 2.], [1., 1., 0.]),
        ([-2.; 3], [1.; 3]),
    ] {
        let direction = mul(direction, 1. / norm(direction));
        assert_eq!(
            hit(view, origin, direction, [-100., 100.], None),
            Err(InterfaceRayError::Ambiguous)
        );
    }
    let interior = hit(
        view,
        [1.5 + 1e-9, 2., -2.],
        [0., 0., 1.],
        [-100., 100.],
        None,
    )
    .unwrap()
    .unwrap();
    assert_eq!(interior.face.side(), FaceSide::ZMinus);
    assert!(
        hit(
            view,
            [1.5 - 1e-9, 2., -2.],
            [0., 0., 1.],
            [-100., 100.],
            None
        )
        .unwrap()
        .is_none()
    );
    // A zero-length diagonal touch does not enter an occupied source cell.
    assert!(
        hit(
            view,
            [0., 5., 2.],
            mul([1., -1., 0.], 1. / 2f64.sqrt()),
            [-100., 100.],
            None
        )
        .unwrap()
        .is_none()
    );
}

#[test]
fn sparse_fractional_unknown_boundaries_and_missing_source_do_not_create_caps() {
    let source = source([5; 3], false, false);
    let mut payload = vec![0; 100];
    payload[..25].fill(100);
    payload[50..75].fill(200);
    let segments = Arc::new(
        Segments::from_fractional(
            source,
            &[65535, 9],
            &[(65535, 1), (65535, 2), (9, 1), (9, 2)],
            &payload,
            FractionalEncoding {
                maximum: 200,
                kind: FractionalKind::Occupancy,
            },
            Some(true),
            || false,
        )
        .unwrap(),
    );
    for number in [65535, 9] {
        let owner = MaskOwner::Supplied(segments.clone());
        let view = owner.view(number).unwrap();
        let actual = hit(view, [2., 2., -2.], [0., 0., 1.], [-100., 100.], None)
            .unwrap()
            .unwrap();
        assert_eq!(actual.face.cell(), [2, 2, 1]);
        assert_eq!(actual.face.side(), FaceSide::ZPlus);
        assert_eq!(actual.face.boundary(), FaceBoundary::Mask);
        assert_eq!(actual.key.number, number);
        assert_eq!(actual.parameter_mm, 3.5);
        let closed = view
            .ray([2., 2., -2.], [0., 0., 1.], [-100., 100.], None)
            .unwrap()
            .first_eligible_hit(|| false)
            .unwrap()
            .unwrap();
        assert_eq!(closed.range_mm[0], 2.5);
    }
    let source = super::source([5; 3], false, true);
    let map = labels(source, |p| u8::from(p == [1; 3]));
    assert!(
        hit(
            MaskView::manual(&map, 1).unwrap(),
            [1., 1., -2.],
            [0., 0., 1.],
            [-100., 100.],
            None
        )
        .unwrap()
        .is_none()
    );
    assert_eq!(
        segments.mask(65535).unwrap().raw_value([2, 2, 1]),
        Some(100)
    );
    assert_eq!(segments.mask(65535).unwrap().raw_value([2, 2, 0]), None);
}

#[test]
fn revisions_replacements_and_undo_keep_distinct_membership_authority() {
    let source = source([5; 3], false, false);
    let original = labels(source.clone(), |p| u8::from(p == [2; 3]));
    let old = hit(
        MaskView::manual(&original, 1).unwrap(),
        [2., 2., -2.],
        [0., 0., 1.],
        [-100., 100.],
        None,
    )
    .unwrap()
    .unwrap();
    let mut changed = original.try_clone(|| false).unwrap();
    changed.paint(1, &[[2., 2.]], 1, 1, || false).unwrap();
    let edited = hit(
        MaskView::manual(&changed, 1).unwrap(),
        [2., 2., -2.],
        [0., 0., 1.],
        [-100., 100.],
        None,
    )
    .unwrap()
    .unwrap();
    assert_eq!(old.key, edited.key);
    assert_ne!(old.revision, edited.revision);
    assert!(edited.parameter_mm < old.parameter_mm);
    let undo = hit(
        MaskView::manual(&original, 1).unwrap(),
        [2., 2., -2.],
        [0., 0., 1.],
        [-100., 100.],
        None,
    )
    .unwrap()
    .unwrap();
    assert_eq!(undo, old);
    let replaced = labels(source, |p| u8::from(p == [2; 3]));
    let other = hit(
        MaskView::manual(&replaced, 1).unwrap(),
        [2., 2., -2.],
        [0., 0., 1.],
        [-100., 100.],
        None,
    )
    .unwrap()
    .unwrap();
    assert_eq!(other.face, old.face);
    assert_ne!(other.key, old.key);
}

#[test]
fn long_rays_are_cancellable_allocation_free_and_reject_invalid_inputs() {
    use std::cell::Cell;
    let source = source([3, 3, 512], false, false);
    let map = labels(source, |p| if p == [1, 1, 510] { 7 } else { 0 });
    let view = MaskView::manual(&map, 7).unwrap();
    let budget = MemoryBudget::volumes();
    let used = budget.used();
    let limit = budget.limit();
    budget.set_limit(0);
    let result = hit(view, [1., 1., -2.], [0., 0., 1.], [-1000., 1000.], None);
    budget.set_limit(limit);
    let result = result.unwrap().unwrap();
    assert_eq!(result.face.cell(), [1, 1, 510]);
    assert_eq!(budget.used(), used);
    let calls = Cell::new(0);
    let ray = InterfaceRay::new(view, [1., 1., -2.], [0., 0., 1.], [-1000., 1000.], None).unwrap();
    assert_eq!(
        ray.first_hit(|| {
            calls.set(calls.get() + 1);
            calls.get() >= 20
        }),
        Err(InterfaceRayError::Cancelled)
    );
    assert_eq!(calls.get(), 20);
    for (origin, direction, range) in [
        ([f64::NAN; 3], [0., 0., 1.], [-1., 1.]),
        ([1.; 3], [0.; 3], [-1., 1.]),
        ([1.; 3], [0., 0., 2.], [-1., 1.]),
        ([1.; 3], [0., 0., 1.], [1., 1.]),
    ] {
        assert!(InterfaceRay::new(view, origin, direction, range, None).is_err());
    }
    let clip = PhysicalClip {
        minimum_mm: [1.; 3],
        maximum_mm: [0.; 3],
    };
    assert!(InterfaceRay::new(view, [1.; 3], [0., 0., 1.], [-1., 1.], Some(&clip)).is_err());
    assert_eq!(budget.used(), used);
}
