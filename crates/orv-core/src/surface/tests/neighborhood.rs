// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use crate::surface::field::*;
use std::{cell::Cell, collections::BTreeMap};

fn transverse(axis: usize) -> [usize; 2] {
    [usize::from(axis == 0), if axis == 2 { 1 } else { 2 }]
}
fn fractional(source: Arc<Volume>, value: impl Fn([usize; 3]) -> i16) -> MaskField {
    let slices: Vec<_> = (0..source.dimensions()[2]).collect();
    fractional_slices(source, &slices, value)
}
fn fractional_slices(
    source: Arc<Volume>,
    slices: &[usize],
    value: impl Fn([usize; 3]) -> i16,
) -> MaskField {
    let [w, h, _] = source.dimensions();
    let mut payload = Vec::new();
    for z in slices {
        for y in 0..h {
            for x in 0..w {
                let v = value([x, y, *z]);
                assert!((-200..=200).contains(&v) && v & 1 == 0);
                payload.push(((v + 200) / 2) as u8);
            }
        }
    }
    if !payload.len().is_multiple_of(2) {
        payload.push(0);
    }
    let frames: Vec<_> = slices.iter().map(|z| (65535, *z)).collect();
    let masks = Arc::new(
        Segments::from_fractional(
            source,
            &[65535],
            &frames,
            &payload,
            FractionalEncoding {
                kind: FractionalKind::Probability,
                maximum: 200,
            },
            Some(true),
            || false,
        )
        .unwrap(),
    );
    MaskField::new(MaskOwner::Supplied(masks), 65535).unwrap()
}
fn eight_roots(axis: usize, p: [usize; 3]) -> i16 {
    let [u, v] = transverse(axis);
    let x = p[u] as i32 - 1;
    let y = p[v] as i32 - 1;
    let s = p[axis];
    let b = if x == 0 {
        0
    } else if s == 0 {
        -2
    } else if x > 0 {
        4
    } else {
        6
    };
    let c = if y == 0 {
        0
    } else if s == 0 {
        -2
    } else if y > 0 {
        8
    } else {
        10
    };
    let d = if x == 0 || y == 0 {
        0
    } else if s == 0 {
        -2
    } else {
        12 + 2 * (usize::from(x < 0) + 2 * usize::from(y < 0)) as i16
    };
    b + c + d
}
fn signature(edge: &EdgePartition) -> Vec<(i64, u64, u8)> {
    edge.points()
        .map(|p| {
            (
                p.position().numerator(),
                p.position().denominator(),
                p.forms(),
            )
        })
        .collect()
}
type FacePointSignature = ([Option<Rational>; 2], [Option<u64>; 2], u8);
fn face_points(face: &SourceFace) -> Vec<FacePointSignature> {
    face.contour()
        .map(|f| {
            f.nodes()
                .map(|(_, n)| {
                    (
                        n.coordinates().map(ExactCoordinate::rational_value),
                        n.coordinates().map(|v| v.approximate().map(f64::to_bits)),
                        n.boundary(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}
fn assert_edge_points_on_face(face: &SourceFace) {
    let Some(contour) = face.contour() else {
        return;
    };
    let [u, v] = transverse(usize::from(face.key().axis));
    for edge in face.edges() {
        let varying = usize::from(usize::from(edge.key().axis) == v);
        let fixed = 1 - varying;
        let global_axis = if fixed == 0 { u } else { v };
        let fixed_value = edge.key().origin[global_axis] - face.key().origin[global_axis];
        for point in edge.points() {
            assert!(
                contour.nodes().any(|(_, n)| {
                    let at = n.coordinates();
                    at[varying].rational_value() == Some(point.position())
                        && at[fixed].rational_value().is_some_and(|r| {
                            i128::from(r.numerator())
                                == i128::from(fixed_value) * i128::from(r.denominator())
                        })
                }),
                "Missing edge point {:?} on face {:?}",
                point,
                face.key()
            );
        }
    }
}

#[test]
fn canonical_edge_partition_covers_all_eight_coefficient_roots_and_inward_expansions() {
    for axis in 0..3 {
        let [u, v] = transverse(axis);
        let mut dimensions = [3; 3];
        dimensions[axis] = 2;
        let source = source(dimensions, true, false);
        let field = fractional(source.clone(), |p| eight_roots(axis, p));
        let mut origin = [1; 3];
        origin[axis] = 0;
        let edge = field
            .edge_partition(
                EdgeKey {
                    origin,
                    axis: axis as u8,
                },
                || false,
            )
            .unwrap();
        assert!(Arc::ptr_eq(edge.field().source(), &source));
        assert_eq!(edge.field().key(), field.key());
        assert_eq!(edge.field().revision(), field.revision());
        assert_eq!(edge.zero(), Some(EdgeZero::Entire));
        assert!(
            edge.cells()
                .iter()
                .all(|c| c.state() == CellNeighborhood::Known)
        );
        assert_eq!(edge.points().count(), 10);
        for (form, denominator) in (3..=10).enumerate() {
            let point = edge
                .points()
                .find(|p| {
                    p.position().numerator() == 1 && p.position().denominator() == denominator
                })
                .unwrap();
            assert_eq!(point.forms(), 1 << form);
        }
        let parameters: Vec<_> = edge.points().map(|p| p.position().as_f64()).collect();
        assert!(parameters.windows(2).all(|p| p[0] < p[1]));
        for (slot, cell) in edge.cells().iter().enumerate() {
            let values = cell.interpolant().unwrap().values();
            for s in [0., 0.25, 0.5, 0.75, 1.] {
                let lerp = |f: [i16; 2]| f64::from(f[0]) * (1. - s) + f64::from(f[1]) * s;
                let b = lerp(edge.normal_forms()[usize::from(slot & 1 != 0)].unwrap());
                let c = lerp(edge.normal_forms()[2 + usize::from(slot & 2 != 0)].unwrap());
                let d = lerp(edge.mixed_forms()[slot].unwrap());
                for r in [0., 0.25, 0.5, 0.75, 1.] {
                    for t in [0., 0.25, 0.5, 0.75, 1.] {
                        let mut local = [0.; 3];
                        local[axis] = s;
                        local[u] = if slot & 1 == 0 { r } else { 1. - r };
                        local[v] = if slot & 2 == 0 { t } else { 1. - t };
                        let tensor: f64 = values
                            .into_iter()
                            .enumerate()
                            .map(|(i, f)| {
                                f64::from(f)
                                    * (0..3)
                                        .map(|a| {
                                            if i & (1 << a) == 0 {
                                                1. - local[a]
                                            } else {
                                                local[a]
                                            }
                                        })
                                        .product::<f64>()
                            })
                            .sum();
                        assert_eq!(tensor, b * r + c * t + d * r * t);
                    }
                }
            }
        }
    }
}

#[test]
fn intrinsic_and_higher_order_roots_survive_degenerate_cell_graphs() {
    for mixed in [false, true] {
        let field = fractional(source([2; 3], false, false), |[u, s, v]| {
            2 * (2 * s as i16 - 1)
                * if mixed {
                    (u * v) as i16
                } else {
                    (u + v) as i16
                }
        });
        let graph = field
            .cell([0; 3])
            .unwrap()
            .interpolant()
            .unwrap()
            .critical_graph(|| false)
            .unwrap();
        assert!(graph.requires_family_resolution());
        assert_eq!(graph.nodes().count(), 0);
        let edge = field
            .edge_partition(
                EdgeKey {
                    origin: [0; 3],
                    axis: 1,
                },
                || false,
            )
            .unwrap();
        assert_eq!(edge.points().count(), 3);
        let middle = edge
            .points()
            .find(|p| p.position().numerator() == 1 && p.position().denominator() == 2)
            .unwrap();
        assert_eq!(
            middle.forms(),
            if mixed { 1 << 4 } else { (1 << 0) | (1 << 2) }
        );
        assert_eq!(
            edge.normal_forms()[0],
            Some(if mixed { [0, 0] } else { [-2, 2] })
        );
        assert_eq!(
            edge.mixed_forms()[0],
            Some(if mixed { [-2, 2] } else { [0, 0] })
        );
        let face = field
            .source_face(
                FaceKey {
                    origin: [0; 3],
                    axis: 0,
                },
                || false,
            )
            .unwrap();
        assert!(
            face.graphs()
                .iter()
                .flatten()
                .any(CriticalGraph::requires_family_resolution)
        );
        assert_eq!(face.contour().unwrap().is_whole_zero(), mixed);
        assert_edge_points_on_face(&face);
    }
}

#[test]
fn known_half_faces_keep_crossings_when_cell_diagonals_are_missing() {
    let base = source([3, 3, 2], false, false);
    let frames = base
        .frames()
        .iter()
        .map(|frame| {
            let mut f = (**frame).clone();
            f.validity = Some(
                (0..9)
                    .map(|i| u8::from(i % 3 == 1 || i / 3 == 1))
                    .collect::<Vec<_>>()
                    .into(),
            );
            Arc::new(f)
        })
        .collect();
    let field = fractional(Arc::new(Volume::new(frames).unwrap()), |p| {
        eight_roots(2, p)
    });
    let edge = field
        .edge_partition(
            EdgeKey {
                origin: [1, 1, 0],
                axis: 2,
            },
            || false,
        )
        .unwrap();
    assert!(edge.cells().iter().all(|c|matches!(c.state(),CellNeighborhood::Unavailable{invalid_corners,unknown_corners:0} if invalid_corners!=0)));
    assert!(edge.normal_forms().iter().all(Option::is_some));
    assert!(edge.mixed_forms().iter().all(Option::is_none));
    assert_eq!(edge.points().count(), 6);
    let face = field
        .source_face(
            FaceKey {
                origin: [1, 1, 0],
                axis: 0,
            },
            || false,
        )
        .unwrap();
    assert!(face.contour().is_some());
    assert!(face.graphs().iter().all(Option::is_none));
    assert_edge_points_on_face(&face);
}

#[test]
fn unknown_coverage_and_boundary_stencils_remain_distinct_from_known_zero_forms() {
    let scalar = |[x, y, _]: [usize; 3]| 2 * (y as i16 - 1) * (2 * x as i16 - 1);
    let sparse = fractional_slices(source([3, 3, 2], false, false), &[0], scalar);
    let key = EdgeKey {
        origin: [0, 1, 0],
        axis: 0,
    };
    let edge = sparse.edge_partition(key, || false).unwrap();
    assert!(
        matches!(edge.cells()[0].state(),CellNeighborhood::Unavailable{invalid_corners:0,unknown_corners} if unknown_corners!=0)
    );
    assert_eq!(edge.cells()[2].state(), CellNeighborhood::Outside);
    assert!(edge.normal_forms()[0].is_some());
    assert!(edge.normal_forms()[1].is_some());
    assert!(edge.normal_forms()[2].is_none());
    assert!(
        edge.normal_samples()[2]
            .unwrap()
            .iter()
            .all(|s| s.value.is_none() && s.source_valid)
    );
    assert_eq!(edge.normal_samples()[3], None);
    assert!(edge.mixed_forms().iter().all(Option::is_none));
    let face = sparse
        .source_face(
            FaceKey {
                origin: [0, 1, 0],
                axis: 2,
            },
            || false,
        )
        .unwrap();
    assert!(face.contour().is_some());
    assert_edge_points_on_face(&face);
    let unknown = sparse
        .source_face(
            FaceKey {
                origin: [0, 1, 1],
                axis: 2,
            },
            || false,
        )
        .unwrap();
    assert!(unknown.contour().is_none());
    let boundary = fractional(source([3, 2, 3], false, false), |[x, _, z]| {
        2 * (z as i16 - 1) * (2 * x as i16 - 1)
    });
    let edge = boundary
        .edge_partition(
            EdgeKey {
                origin: [0, 0, 1],
                axis: 0,
            },
            || false,
        )
        .unwrap();
    assert!(
        edge.cells()
            .iter()
            .any(|c| c.state() == CellNeighborhood::Outside)
    );
    assert!(
        edge.cells()
            .iter()
            .any(|c| c.state() == CellNeighborhood::Known)
    );
    assert_eq!(edge.normal_forms()[0], Some([0, 0]));
    assert_eq!(edge.normal_samples()[1], None);
    assert_eq!(edge.points().count(), 3);
    let face = boundary
        .source_face(
            FaceKey {
                origin: [0, 0, 1],
                axis: 1,
            },
            || false,
        )
        .unwrap();
    assert!(face.graphs()[0].is_none());
    assert!(face.graphs()[1].is_some());
    assert_edge_points_on_face(&face);
}

#[test]
fn source_faces_share_exact_edge_unions_across_axes_boundaries_and_repeated_owners() {
    let mut state = 193u64;
    for mode in 0usize..8 {
        let dimensions = if mode == 0 { [3, 4, 2] } else { [4; 3] };
        let count = dimensions.into_iter().product();
        let values: Vec<i16> = (0..count)
            .map(|_| {
                state = state
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                if state.is_multiple_of(3) {
                    0
                } else {
                    2 * ((state % 201) as i16 - 100)
                }
            })
            .collect();
        let field = fractional(
            source(dimensions, mode.is_multiple_of(2), false),
            |[x, y, z]| values[x + dimensions[0] * (y + dimensions[1] * z)],
        );
        let mut seen = BTreeMap::new();
        for axis in 0..3 {
            let [u, v] = transverse(axis);
            for plane in 0..dimensions[axis] {
                for i in 0..dimensions[u] - 1 {
                    for j in 0..dimensions[v] - 1 {
                        let mut origin = [0; 3];
                        origin[axis] = plane as u32;
                        origin[u] = i as u32;
                        origin[v] = j as u32;
                        let key = FaceKey {
                            origin,
                            axis: axis as u8,
                        };
                        let face = field.source_face(key, || false).unwrap();
                        let duplicate = field.source_face(key, || false).unwrap();
                        assert_eq!(face.key(), key);
                        assert_eq!(face.field().key(), field.key());
                        assert_eq!(face.field().revision(), field.revision());
                        for (index, cell) in face.cells().iter().enumerate() {
                            match cell.interpolant() {
                                Some(original) => {
                                    let factors = face.factorizations()[index].unwrap();
                                    assert_eq!(factors.original(), original);
                                    assert_eq!(
                                        factors.reconstruct_values().unwrap(),
                                        original.values()
                                    );
                                    assert_eq!(cell.phase(), Some(original.phase()));
                                }
                                None => {
                                    assert!(face.factorizations()[index].is_none());
                                    assert_eq!(cell.phase(), None);
                                }
                            }
                        }
                        assert_eq!(face_points(&face), face_points(&duplicate));
                        assert_eq!(
                            face.contour().unwrap().segments().collect::<Vec<_>>(),
                            duplicate.contour().unwrap().segments().collect::<Vec<_>>()
                        );
                        assert_edge_points_on_face(&face);
                        for edge in face.edges() {
                            let sig = signature(edge);
                            if let Some(previous) = seen.insert(edge.key(), sig.clone()) {
                                assert_eq!(previous, sig);
                            }
                            let direct = field.edge_partition(edge.key(), || false).unwrap();
                            assert_eq!(signature(&direct), sig);
                            assert_eq!(edge.normal_forms(), direct.normal_forms());
                            assert_eq!(edge.mixed_forms(), direct.mixed_forms());
                            assert_eq!(edge.cells(), direct.cells());
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn source_owners_keep_old_revisions_and_selected_ids_alive_without_label_changes() {
    let source = source([4; 3], false, false);
    let map = labels(
        source.clone(),
        |p| if p[0].is_multiple_of(2) { 1 } else { 2 },
    );
    let weak = Arc::downgrade(&map);
    let field = MaskField::new(MaskOwner::Manual(map.clone()), 1).unwrap();
    let other = MaskField::new(MaskOwner::Manual(map.clone()), 2).unwrap();
    let key = FaceKey {
        origin: [1, 1, 1],
        axis: 2,
    };
    let face = field.source_face(key, || false).unwrap();
    let second = other.source_face(key, || false).unwrap();
    assert_ne!(face.field().key(), second.field().key());
    assert!(Arc::ptr_eq(face.field().source(), &source));
    let before = face_points(&face);
    let mut next = map.try_clone(|| false).unwrap();
    next.paint(1, &[[2., 2.]], 1, 2, || false).unwrap();
    assert_ne!(next.revision(), face.field().revision());
    assert_eq!(face_points(&face), before);
    assert_eq!(map.value([2, 2, 1]), Some(1));
    drop(map);
    drop(field);
    drop(other);
    drop(second);
    assert!(weak.upgrade().is_some());
    drop(face);
    assert!(weak.upgrade().is_none());
}

#[test]
fn source_neighborhood_bounds_and_every_cancellation_publish_no_partial_owner() {
    let field = fractional(source([2; 3], false, false), |[x, y, z]| {
        2 * (x as i16 + y as i16 + z as i16 - 1)
    });
    let key = FaceKey {
        origin: [0; 3],
        axis: 0,
    };
    let calls = Cell::new(0);
    let prior = field
        .source_face(key, || {
            calls.set(calls.get() + 1);
            false
        })
        .unwrap();
    let signature = face_points(&prior);
    let MaskOwner::Supplied(owner) = field.owner() else {
        panic!("Supplied field required");
    };
    let strong = Arc::strong_count(owner);
    assert!(std::mem::size_of::<EdgePartition>() <= 2048);
    assert!(std::mem::size_of::<SourceFace>() <= 32768);
    for cancel_at in 0..calls.get() {
        let at = Cell::new(0);
        assert!(matches!(
            field.source_face(key, || {
                let n = at.get();
                at.set(n + 1);
                n == cancel_at
            }),
            Err(FieldError::Cancelled)
        ));
        assert_eq!(Arc::strong_count(owner), strong);
        assert_eq!(face_points(&prior), signature);
    }
    for key in [
        FaceKey {
            origin: [0; 3],
            axis: 3,
        },
        FaceKey {
            origin: [u32::MAX; 3],
            axis: 0,
        },
        FaceKey {
            origin: [0, 1, 0],
            axis: 0,
        },
    ] {
        assert!(matches!(
            field.source_face(key, || false),
            Err(FieldError::Bounds)
        ));
    }
    for key in [
        EdgeKey {
            origin: [0; 3],
            axis: 3,
        },
        EdgeKey {
            origin: [u32::MAX; 3],
            axis: 0,
        },
        EdgeKey {
            origin: [1, 0, 0],
            axis: 0,
        },
    ] {
        assert!(matches!(
            field.edge_partition(key, || false),
            Err(FieldError::Bounds)
        ));
    }
    let unavailable = fractional(source([3; 3], false, true), |_| 0);
    let edge = unavailable
        .edge_partition(
            EdgeKey {
                origin: [1, 1, 0],
                axis: 2,
            },
            || false,
        )
        .unwrap();
    assert!(edge.zero().is_none());
    assert_eq!(edge.points().count(), 0);
}
