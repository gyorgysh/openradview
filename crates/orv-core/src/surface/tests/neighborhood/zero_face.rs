// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
mod mesh;

fn point(u: i64, v: i64, q: i128) -> [ExactCoordinate; 2] {
    [u, v].map(|n| {
        let field = Trilinear::new(std::array::from_fn(|i| {
            if i & 1 == 0 {
                n as i16
            } else {
                (i128::from(n) - q) as i16
            }
        }))
        .unwrap();
        let EdgeZero::Point(value) = field.edge(0, 0).unwrap() else {
            panic!("Finite rational required")
        };
        ExactCoordinate::from_rational(value).unwrap().unwrap()
    })
}
fn expected(signs: [i8; 2], constraint: bool) -> ZeroFacePhase {
    if constraint {
        return ZeroFacePhase::Constraint;
    }
    match signs.map(|s| s >= 0) {
        [true, true] => ZeroFacePhase::Interior,
        [true, false] => ZeroFacePhase::Interface { normal: 1 },
        [false, true] => ZeroFacePhase::Interface { normal: -1 },
        [false, false] => ZeroFacePhase::Sheet,
    }
}
#[test]
fn all_constant_inward_phases_keep_selected_ties_sheets_and_normal_orientation() {
    for axis in 0..3 {
        let mut dimensions = [2; 3];
        dimensions[axis] = 3;
        let source = source(dimensions, true, false);
        for negative in [-2, 0, 2] {
            for positive in [-2, 0, 2] {
                let field = fractional(source.clone(), |p| match p[axis] {
                    0 => negative,
                    1 => 0,
                    _ => positive,
                });
                let mut origin = [0; 3];
                origin[axis] = 1;
                let face = field
                    .source_face(
                        FaceKey {
                            origin,
                            axis: axis as u8,
                        },
                        || false,
                    )
                    .unwrap();
                let zero = face.whole_zero_face(|| false).unwrap().unwrap();
                assert!(Arc::ptr_eq(zero.field().source(), &source));
                assert_eq!(zero.field().key(), field.key());
                assert_eq!(zero.field().revision(), field.revision());
                assert_eq!(zero.key(), face.key());
                assert_eq!(
                    *zero.inward_fields(),
                    [
                        Some(Bilinear::new([negative; 4]).unwrap()),
                        Some(Bilinear::new([positive; 4]).unwrap())
                    ]
                );
                let sample = zero.sample(point(1, 2, 3), || false).unwrap();
                let signs = [negative.signum() as i8, positive.signum() as i8];
                assert_eq!(sample.inward_signs, signs.map(Some));
                assert_eq!(sample.phase, expected(signs, false));
                for (i, value) in [negative, positive].into_iter().enumerate() {
                    let mut p = [0.5; 3];
                    let [u, v] = transverse(axis);
                    p[u] = 0.5;
                    p[v] = 0.5;
                    p[axis] = if i == 0 { 0.75 } else { 0.25 };
                    assert_eq!(
                        zero.cells()[i].interpolant().unwrap().evaluate(p),
                        Some(f64::from(value) / 4.)
                    );
                }
                // Original fractional half-level source nodes remain selected.
                let MaskOwner::Supplied(map) = field.owner() else {
                    panic!("Supplied mask required")
                };
                assert_eq!(
                    map.mask(65535)
                        .unwrap()
                        .raw_value(origin.map(|v| v as usize)),
                    Some(100)
                );
                assert_eq!(
                    map.mask(65535).unwrap().value(origin.map(|v| v as usize)),
                    Some(true)
                );
            }
        }
    }
}
#[test]
fn source_inward_fields_and_intersections_agree_with_independent_tensor_samples() {
    let values = [[-2, -2, 4, 2], [2, -6, 2, -6]];
    for axis in 0..3 {
        let mut dimensions = [2; 3];
        dimensions[axis] = 3;
        let [u, v] = transverse(axis);
        let field = fractional(source(dimensions, false, false), |p| {
            if p[axis] == 1 {
                0
            } else {
                values[usize::from(p[axis] == 2)][[0, 1, 3, 2][p[u] + 2 * p[v]]]
            }
        });
        let mut origin = [0; 3];
        origin[axis] = 1;
        let face = field
            .source_face(
                FaceKey {
                    origin,
                    axis: axis as u8,
                },
                || false,
            )
            .unwrap();
        let zero = face.whole_zero_face(|| false).unwrap().unwrap();
        assert_eq!(zero.inward_fields().map(|v| v.unwrap().values()), values);
        for u in 0..=6 {
            for v in 0..=6 {
                let sign = |values: [i16; 4]| {
                    let [a, b, c, d] = values.map(i64::from);
                    (a * (6 - u) * (6 - v) + b * u * (6 - v) + c * u * v + d * (6 - u) * v).signum()
                        as i8
                };
                let signs = values.map(sign);
                let sample = zero.sample(point(u, v, 6), || false).unwrap();
                assert_eq!(sample.inward_signs, signs.map(Some));
                assert_eq!(sample.phase, expected(signs, signs.contains(&0)));
            }
        }
        for p in zero.intersections().unwrap().points() {
            assert_eq!(
                zero.sample(*p.coordinates(), || false).unwrap().phase,
                ZeroFacePhase::Constraint
            );
        }
    }
}
#[test]
fn outside_unknown_and_invalid_neighbors_never_become_empty_or_capped() {
    let key = FaceKey {
        origin: [0, 0, 2],
        axis: 2,
    };
    for invalid in [false, true] {
        let source = source([2, 2, 4], false, invalid);
        let field = if invalid {
            fractional(source, |p| if p[2] == 2 { 0 } else { 2 })
        } else {
            fractional_slices(source, &[0, 2, 3], |p| if p[2] == 2 { 0 } else { 2 })
        };
        let face = field.source_face(key, || false).unwrap();
        let zero = face.whole_zero_face(|| false).unwrap().unwrap();
        let CellNeighborhood::Unavailable {
            invalid_corners,
            unknown_corners,
        } = zero.cells()[0].state()
        else {
            panic!("Unavailable neighbor required")
        };
        assert_eq!(invalid_corners != 0, invalid);
        assert_eq!(unknown_corners != 0, !invalid);
        assert_eq!(zero.inward_fields()[0], None);
        assert!(zero.intersections().is_none());
        assert_eq!(
            zero.sample(point(1, 1, 2), || false).unwrap(),
            ZeroFaceSample {
                inward_signs: [None, Some(1)],
                phase: ZeroFacePhase::Unresolved
            }
        );
    }
    let field = fractional(source([2, 2, 2], false, false), |p| {
        if p[2] == 0 { 0 } else { -2 }
    });
    let face = field
        .source_face(
            FaceKey {
                origin: [0; 3],
                axis: 2,
            },
            || false,
        )
        .unwrap();
    let zero = face.whole_zero_face(|| false).unwrap().unwrap();
    assert_eq!(zero.cells()[0].state(), CellNeighborhood::Outside);
    assert_eq!(
        zero.sample(point(1, 1, 2), || false).unwrap().phase,
        ZeroFacePhase::Unresolved
    );
    let field = fractional(source([2, 2, 2], false, true), |_| 0);
    assert!(
        field
            .source_face(
                FaceKey {
                    origin: [0, 0, 1],
                    axis: 2
                },
                || false
            )
            .unwrap()
            .whole_zero_face(|| false)
            .unwrap()
            .is_none()
    );
    let field = fractional(source([2, 2, 2], false, false), |_| 2);
    assert!(
        field
            .source_face(
                FaceKey {
                    origin: [0; 3],
                    axis: 2
                },
                || false
            )
            .unwrap()
            .whole_zero_face(|| false)
            .unwrap()
            .is_none()
    );
}
#[test]
fn source_owned_zero_face_survives_original_drop_and_cancels_without_partial_owners() {
    let field = fractional(source([2, 2, 3], false, false), |p| {
        if p[2] == 1 {
            0
        } else {
            let values = if p[2] == 0 {
                [0, -2, 0, 2]
            } else {
                [4, -12, 4, -12]
            };
            values[[0, 1, 3, 2][p[0] + 2 * p[1]]]
        }
    });
    let face = field
        .source_face(
            FaceKey {
                origin: [0, 0, 1],
                axis: 2,
            },
            || false,
        )
        .unwrap();
    let calls = Cell::new(0usize);
    let prior = face
        .whole_zero_face(|| {
            calls.set(calls.get() + 1);
            false
        })
        .unwrap()
        .unwrap();
    let count = calls.get();
    let MaskOwner::Supplied(map) = field.owner() else {
        panic!("Supplied mask required")
    };
    let weak = Arc::downgrade(map);
    let baseline = Arc::strong_count(map);
    for stop in 1..=count {
        calls.set(0);
        assert!(matches!(
            face.whole_zero_face(|| {
                calls.set(calls.get() + 1);
                calls.get() == stop
            }),
            Err(FieldError::Cancelled)
        ));
        let MaskOwner::Supplied(map) = field.owner() else {
            panic!("Supplied mask required")
        };
        assert_eq!(Arc::strong_count(map), baseline);
    }
    let saved_revision = field.revision().clone();
    drop(face);
    drop(field);
    assert!(weak.upgrade().is_some());
    assert_eq!(prior.field().revision(), &saved_revision);
    assert!(std::mem::size_of::<WholeZeroFace>() < 4096);
    calls.set(0);
    let sample = prior
        .sample(point(1, 1, 2), || {
            calls.set(calls.get() + 1);
            false
        })
        .unwrap();
    for stop in 1..=calls.get() {
        let n = Cell::new(0usize);
        assert_eq!(
            prior.sample(point(1, 1, 2), || {
                n.set(n.get() + 1);
                n.get() == stop
            }),
            Err(FieldError::Cancelled)
        );
        assert_eq!(prior.sample(point(1, 1, 2), || false).unwrap(), sample);
    }
    drop(prior);
    assert!(weak.upgrade().is_none());
}

#[test]
fn source_joint_regions_retain_all_original_constraints_and_exact_phase_owners() {
    for axis in 0..3 {
        let mut dimensions = [2; 3];
        dimensions[axis] = 3;
        let [u, v] = transverse(axis);
        for (values, regions) in [
            ([[2; 4], [-2; 4]], 1),
            ([[-2; 4], [-2; 4]], 1),
            ([[0; 4], [0; 4]], 1),
            ([[2, -4, 8, -4], [8, -4, 2, -4]], 9),
            ([[0, -2, 0, 2], [6, -58, 6, 38]], 5),
        ] {
            let field = fractional(source(dimensions, true, false), |p| {
                if p[axis] == 1 {
                    0
                } else {
                    values[usize::from(p[axis] == 2)][[0, 1, 3, 2][p[u] + 2 * p[v]]]
                }
            });
            let mut origin = [0; 3];
            origin[axis] = 1;
            let face = field
                .source_face(
                    FaceKey {
                        origin,
                        axis: axis as u8,
                    },
                    || false,
                )
                .unwrap();
            let partition = face.zero_subdivision(|| false).unwrap().unwrap();
            assert_eq!(partition.face().field().key(), field.key());
            assert_eq!(partition.face().field().revision(), field.revision());
            assert_eq!(partition.face().key(), face.key());
            let graph = partition.subdivision();
            assert_eq!(graph.fields().map(Bilinear::values), values);
            assert_eq!(graph.regions().count(), regions);
            for (_, original) in face.contour().unwrap().nodes() {
                assert!(graph.nodes().any(|(_, n)| {
                    n.coordinates()
                        .iter()
                        .zip(original.coordinates())
                        .all(|(a, b)| a.compare(*b).unwrap() == std::cmp::Ordering::Equal)
                }));
            }
            for (id, region) in graph.regions() {
                assert_eq!(
                    partition.region_phase(id),
                    Some(expected(region.signs, false))
                );
            }
            assert_eq!(partition.region_phase(255), None);
        }
    }
}

#[test]
fn source_joint_regions_leave_unavailable_cases_open_and_cancel_without_new_owners() {
    let key = FaceKey {
        origin: [0, 0, 2],
        axis: 2,
    };
    let field = fractional(source([2, 2, 4], false, true), |p| {
        if p[2] == 2 { 0 } else { 2 }
    });
    assert!(
        field
            .source_face(key, || false)
            .unwrap()
            .zero_subdivision(|| false)
            .unwrap()
            .is_none()
    );
    let field = fractional(source([2, 2, 2], false, false), |_| 0);
    assert!(
        field
            .source_face(
                FaceKey {
                    origin: [0; 3],
                    axis: 2
                },
                || false
            )
            .unwrap()
            .zero_subdivision(|| false)
            .unwrap()
            .is_none()
    );
    let field = fractional(source([2, 2, 3], false, false), |_| 0);
    let face = field
        .source_face(
            FaceKey {
                origin: [0, 0, 1],
                axis: 2,
            },
            || false,
        )
        .unwrap();
    let calls = Cell::new(0usize);
    let prior = face
        .zero_subdivision(|| {
            calls.set(calls.get() + 1);
            false
        })
        .unwrap()
        .unwrap();
    let count = calls.get();
    let MaskOwner::Supplied(map) = field.owner() else {
        panic!("Supplied owner required")
    };
    let weak = Arc::downgrade(map);
    let baseline = Arc::strong_count(map);
    for stop in 1..=count {
        let n = Cell::new(0usize);
        assert!(matches!(
            face.zero_subdivision(|| {
                n.set(n.get() + 1);
                n.get() == stop
            }),
            Err(FieldError::Cancelled)
        ));
        let MaskOwner::Supplied(map) = field.owner() else {
            panic!("Supplied owner required")
        };
        assert_eq!(Arc::strong_count(map), baseline);
    }
    drop(face);
    drop(field);
    assert!(weak.upgrade().is_some());
    assert_eq!(prior.region_phase(0), Some(ZeroFacePhase::Interior));
    drop(prior);
    assert!(weak.upgrade().is_none());
}
