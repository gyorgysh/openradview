// SPDX-License-Identifier: GPL-3.0-or-later
use super::super::tests::{coord, positive_root, rational};
use super::*;
use std::{cell::Cell, collections::BTreeSet};

fn rational_point(node: &ContourNode) -> [(i128, i128); 2] {
    node.coordinates.map(|v| {
        let r = v.rational_value().unwrap();
        (i128::from(r.numerator), i128::from(r.denominator))
    })
}
fn value([a, b, c, d]: [i32; 4], [(u, uq), (v, vq)]: [(i128, i128); 2]) -> i128 {
    i128::from(a) * uq * vq
        + i128::from(b) * u * vq
        + i128::from(c) * v * uq
        + i128::from(d) * u * v
}
fn rational_equal(a: (i128, i128), b: (i128, i128)) -> bool {
    a.0 * b.1 == b.0 * a.1
}
fn same(a: &FaceArrangement, b: &FaceArrangement) {
    assert_eq!(a.is_whole_zero(), b.is_whole_zero());
    assert_eq!(a.nodes().count(), b.nodes().count());
    for ((aid, a), (bid, b)) in a.nodes().zip(b.nodes()) {
        assert_eq!(aid, bid);
        assert!(same_point(a.coordinates, b.coordinates).unwrap());
        assert_eq!(a.boundary, b.boundary);
        for (a, b) in a.coordinates.into_iter().zip(b.coordinates) {
            assert_eq!(a.parameter, b.parameter);
            assert_eq!(a.numerator, b.numerator);
            assert_eq!(a.denominator, b.denominator);
            assert_eq!(
                a.approximate().map(f64::to_bits),
                b.approximate().map(f64::to_bits)
            );
        }
    }
    assert_eq!(segment_set(a), segment_set(b));
}
fn segment_set(face: &FaceArrangement) -> BTreeSet<(u8, u8)> {
    face.segments().map(|s| (s.from, s.to)).collect()
}
fn verify_rational(face: &FaceArrangement) {
    // Derive power coefficients independently from four tensor basis values.
    let f = face.field.values.map(i32::from);
    let polynomial = [f[0], f[1] - f[0], f[3] - f[0], f[2] - f[1] - f[3] + f[0]];
    for (id, node) in face.nodes() {
        let point = rational_point(node);
        assert_eq!(value(polynomial, point), 0);
        for (axis, (n, d)) in point.into_iter().enumerate() {
            assert!((0..=d).contains(&n));
            assert_eq!(node.boundary & (1 << (axis * 2)) != 0, n == 0);
            assert_eq!(node.boundary & (1 << (axis * 2 + 1)) != 0, n == d);
        }
        let degree = face
            .segments()
            .filter(|s| s.from == id || s.to == id)
            .count();
        if !face.whole {
            let [(u, uq), (v, vq)] = point;
            let [_, b, c, d] = polynomial.map(i128::from);
            let fu = b * vq + d * v;
            let fv = c * uq + d * u;
            let expected = if fu == 0 && fv == 0 {
                usize::from(u > 0) + usize::from(u < uq) + usize::from(v > 0) + usize::from(v < vq)
            } else if node.boundary.count_ones() == 0 {
                2
            } else if node.boundary.count_ones() == 1 {
                let along_edge = if u == 0 || u == uq { fv } else { fu };
                if along_edge == 0 { 2 } else { 1 }
            } else {
                let inward_u = fu.signum() * if u == 0 { 1 } else { -1 };
                let inward_v = fv.signum() * if v == 0 { 1 } else { -1 };
                usize::from(inward_u * inward_v <= 0)
            };
            assert_eq!(
                degree, expected,
                "Missing/extra curve at {point:?} in {:?}",
                face.field
            );
        }
    }
    for segment in face.segments() {
        let start = rational_point(face.node(segment.from).unwrap());
        let end = rational_point(face.node(segment.to).unwrap());
        assert_ne!(segment.from, segment.to);
        let varying = usize::from(rational_equal(start[0], end[0]));
        let other = 1 - varying;
        let delta = end[varying].0 * start[varying].1 - start[varying].0 * end[varying].1;
        assert_ne!(delta, 0);
        for k in 1..4 {
            let n =
                (4 - k) * start[varying].0 * end[varying].1 + k * end[varying].0 * start[varying].1;
            let q = 4 * start[varying].1 * end[varying].1;
            let [a, b, c, d] = polynomial.map(i128::from);
            let (constant, slope) = if varying == 0 {
                (a * q + b * n, c * q + d * n)
            } else {
                (a * q + c * n, b * q + d * n)
            };
            let mut point = [(0, 1); 2];
            point[varying] = (n, q);
            if slope != 0 {
                point[other] = (-constant * slope.signum(), slope.abs());
            } else {
                assert_eq!(constant, 0);
                assert!(rational_equal(start[other], end[other]));
                point[other] = start[other];
            }
            assert!(point.into_iter().all(|(n, q)| 0 <= n && n <= q));
            assert_eq!(value(polynomial, point), 0);
            let [(u, uq), (v, vq)] = point;
            let tangent = if varying == 0 {
                -(c * uq + d * u)
            } else {
                b * vq + d * v
            };
            assert!(
                delta.signum() * tangent.signum() > 0,
                "Wrong contour orientation for {:?}",
                face.field
            );
        }
    }
    // Each original edge crossing/contact must appear, including zero endpoints.
    let corners = [
        [(0, 1), (0, 1)],
        [(1, 1), (0, 1)],
        [(1, 1), (1, 1)],
        [(0, 1), (1, 1)],
    ];
    for edge in 0..4 {
        let a = i128::from(f[edge]);
        let b = i128::from(f[(edge + 1) % 4]);
        if a == 0 {
            assert!(
                face.nodes()
                    .any(|(_, n)| rational_point(n) == corners[edge])
            );
        }
        if a * b < 0 {
            let axis = usize::from(edge % 2 == 1);
            let mut t = (a, a - b);
            if t.1 < 0 {
                t = (-t.0, -t.1);
            }
            if edge >= 2 {
                t.0 = t.1 - t.0;
            }
            let mut point = corners[edge];
            point[axis] = t;
            assert!(face.nodes().any(|(_, n)| {
                rational_point(n)
                    .into_iter()
                    .zip(point)
                    .all(|(a, b)| rational_equal(a, b))
            }));
        }
    }
}

#[test]
fn maximum_insertion_union_fits_fixed_storage_and_preserves_all_cross_branches() {
    let mut insertions = Vec::new();
    for k in 1..=32 {
        insertions.push([rational(k, 65), rational(1, 2)]);
        insertions.push([rational(1, 2), rational(k, 65)]);
    }
    assert_eq!(insertions.len(), FaceArrangement::MAX_INSERTIONS);
    let field = Bilinear::new([1, -1, 1, -1]).unwrap();
    let face = field.arrangement(&insertions, || false).unwrap();
    assert_eq!(face.nodes().count(), 69);
    assert_eq!(face.segments().count(), 68);
    verify_rational(&face);
    insertions.reverse();
    same(&face, &field.arrangement(&insertions, || false).unwrap());
}

#[test]
fn equivalent_insertions_retain_identical_expressions_and_approximations() {
    let a = positive_root(2, 0, -1);
    let b = positive_root(8, 0, -1);
    let x = [coord(a, [0, 1], [1, 0]), coord(a, [1, -1], [1, 0])];
    let y = [coord(b, [0, 2], [1, 0]), coord(b, [1, -2], [1, 0])];
    let field = Bilinear::new([-1, 0, 1, 0]).unwrap();
    let first = field.arrangement(&[x, y], || false).unwrap();
    same(&first, &field.arrangement(&[y, x], || false).unwrap());
    assert_eq!(first.nodes().count(), 3);
}

#[test]
fn face_arrangements_preserve_contacts_lines_crosses_and_whole_faces() {
    for (values, nodes, segments, whole) in [
        ([1, 1, 1, 1], 0, 0, false),
        ([0, 1, 2, 1], 1, 0, false),
        ([0, 1, 0, 1], 2, 0, false),
        ([-1, 0, 1, 0], 2, 1, false),
        ([-1, 1, 1, -1], 2, 1, false),
        ([-1, -1, 1, 1], 2, 1, false),
        ([1, -1, 1, -1], 5, 4, false),
        ([0, -1, 1, 0], 4, 3, false),
        ([0, 0, 1, 0], 3, 2, false),
        ([0, 0, 0, 0], 4, 0, true),
    ] {
        let face = Bilinear::new(values)
            .unwrap()
            .arrangement(&[], || false)
            .unwrap();
        assert_eq!(face.nodes().count(), nodes, "{values:?}");
        assert_eq!(face.segments().count(), segments, "{values:?}");
        assert_eq!(face.is_whole_zero(), whole);
        verify_rational(&face);
    }
}

#[test]
fn generated_face_contours_have_exact_curves_boundary_coverage_and_orientation() {
    let mut fixtures = Vec::new();
    for mut code in 0..81 {
        fixtures.push(std::array::from_fn(|_| {
            let v = code % 3;
            code /= 3;
            v as i16 - 1
        }));
    }
    let mut state = 11u64;
    for _ in 0..1024 {
        fixtures.push(std::array::from_fn(|_| {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (state % 511) as i16 - 255
        }));
    }
    for values in fixtures {
        let face = Bilinear::new(values)
            .unwrap()
            .arrangement(&[], || false)
            .unwrap();
        verify_rational(&face);
        let complement = Bilinear::new(values.map(|v| -v))
            .unwrap()
            .arrangement(&[], || false)
            .unwrap();
        assert_eq!(face.nodes().count(), complement.nodes().count());
        for ((_, a), (_, b)) in face.nodes().zip(complement.nodes()) {
            assert_eq!(rational_point(a), rational_point(b));
        }
        assert_eq!(
            segment_set(&face),
            complement.segments().map(|s| (s.to, s.from)).collect()
        );
    }
}

#[test]
fn exact_insertions_split_lines_and_curves_independent_of_input_order() {
    let a = positive_root(2, 0, -1);
    let b = positive_root(3, 0, -1);
    for (values, insertions) in [
        (
            [-1, 0, 1, 0],
            vec![
                [coord(a, [0, 1], [1, 0]), coord(a, [1, -1], [1, 0])],
                [coord(b, [0, 1], [1, 0]), coord(b, [1, -1], [1, 0])],
                [rational(1, 2), rational(1, 2)],
            ],
        ),
        (
            [-1, -1, 2, 1],
            vec![
                [coord(a, [0, 1], [1, 0]), coord(a, [1, 0], [2, 1])],
                [coord(b, [0, 1], [1, 0]), coord(b, [1, 0], [2, 1])],
            ],
        ),
    ] {
        let field = Bilinear::new(values).unwrap();
        let original = field.arrangement(&insertions, || false).unwrap();
        assert_eq!(original.nodes().count(), 2 + insertions.len());
        assert_eq!(original.segments().count(), 1 + insertions.len());
        for (_, node) in original.nodes() {
            assert_eq!(bilinear_sign(coefficients(field), node.coordinates), Ok(0));
        }
        let mut reordered = insertions.clone();
        reordered.reverse();
        reordered.extend(insertions);
        same(&original, &field.arrangement(&reordered, || false).unwrap());
        // Canonical lexicographic IDs must retain every inserted point, and no
        // segment may jump over it along these monotone single branches.
        let mut edges = segment_set(&original)
            .into_iter()
            .map(|(a, b)| (a.min(b), a.max(b)))
            .collect::<Vec<_>>();
        edges.sort();
        assert_eq!(
            edges,
            (1..original.nodes().count())
                .map(|i| ((i - 1) as u8, i as u8))
                .collect::<Vec<_>>()
        );
    }
}

#[test]
fn entire_zero_edge_insertions_keep_all_points_and_split_at_the_singular_centre() {
    let insertions = [positive_root(2, 0, -1), positive_root(3, 0, -1)]
        .map(|r| [rational(0, 1), coord(r, [0, 1], [1, 0])]);
    let face = Bilinear::new([0, -1, 1, 0])
        .unwrap()
        .arrangement(&insertions, || false)
        .unwrap();
    assert_eq!(face.nodes().count(), 6);
    assert_eq!(face.segments().count(), 5);
    let centre = face
        .nodes()
        .find(|(_, n)| {
            n.coordinates[0].compare(rational(0, 1)).unwrap() == Ordering::Equal
                && n.coordinates[1].compare(rational(1, 2)).unwrap() == Ordering::Equal
        })
        .unwrap()
        .0;
    assert_eq!(
        face.segments()
            .filter(|s| s.from == centre || s.to == centre)
            .count(),
        3
    );
    for insertion in insertions {
        assert!(
            face.nodes()
                .any(|(_, n)| same_point(n.coordinates, insertion).unwrap())
        );
    }
}

#[test]
fn both_incident_cell_graphs_make_identical_shared_face_arrangements() {
    let mut state = 109u64;
    let mut next = || {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (state % 511) as i16 - 255
    };
    let mut enlarged = 0;
    for axis in 0..3 {
        for upper in [false, true] {
            for _ in 0..128 {
                let face: [i16; 4] = std::array::from_fn(|_| next());
                let mut left = std::array::from_fn(|_| next());
                let mut right = std::array::from_fn(|_| next());
                for (index, value) in face_indices(axis, upper).unwrap().into_iter().zip(face) {
                    left[index] = value;
                }
                for (index, value) in face_indices(axis, !upper).unwrap().into_iter().zip(face) {
                    right[index] = value;
                }
                let left = Trilinear::new(left)
                    .unwrap()
                    .critical_graph(|| false)
                    .unwrap();
                let right = Trilinear::new(right)
                    .unwrap()
                    .critical_graph(|| false)
                    .unwrap();
                let combined = left
                    .face_arrangement(axis, upper, Some(&right), &[], || false)
                    .unwrap();
                same(
                    &combined,
                    &right
                        .face_arrangement(axis, !upper, Some(&left), &[], || false)
                        .unwrap(),
                );
                enlarged += usize::from(
                    combined.nodes().count()
                        > left
                            .face_arrangement(axis, upper, None, &[], || false)
                            .unwrap()
                            .nodes()
                            .count(),
                );
                for segment in combined.segments() {
                    let a = segment.for_cell_face(axis, upper).unwrap();
                    let b = segment.for_cell_face(axis, !upper).unwrap();
                    assert_eq!((a.from, a.to), (b.to, b.from));
                }
            }
        }
    }
    assert!(
        enlarged > 20,
        "Neighbor insertions must change actual shared faces"
    );
}

#[test]
fn face_limits_mismatches_and_every_cancellation_leave_prior_results_intact() {
    let field = Bilinear::new([-1, 0, 1, 0]).unwrap();
    let point = [rational(1, 2), rational(1, 2)];
    let count = Cell::new(0);
    let prior = field
        .arrangement(&[point], || {
            count.set(count.get() + 1);
            false
        })
        .unwrap();
    assert!(std::mem::size_of::<FaceArrangement>() <= 8192);
    assert!(matches!(
        field.arrangement(&[point; 65], || false),
        Err(FieldError::Capacity)
    ));
    assert!(matches!(
        field.arrangement(&[[rational(0, 1); 2]], || false),
        Err(FieldError::Contour)
    ));
    for cancel_at in 0..count.get() {
        let calls = Cell::new(0);
        assert!(matches!(
            field.arrangement(&[point], || {
                let at = calls.get();
                calls.set(at + 1);
                at == cancel_at
            }),
            Err(FieldError::Cancelled)
        ));
        same(&prior, &field.arrangement(&[point], || false).unwrap());
    }
    let a = Trilinear::new([-1; 8])
        .unwrap()
        .critical_graph(|| false)
        .unwrap();
    let b = Trilinear::new([1; 8])
        .unwrap()
        .critical_graph(|| false)
        .unwrap();
    assert!(matches!(
        a.face_arrangement(0, true, Some(&b), &[], || false),
        Err(FieldError::Contour)
    ));
    assert!(matches!(
        a.face_arrangement(3, true, None, &[], || false),
        Err(FieldError::Bounds)
    ));
    assert!(
        ContourSegment { from: 0, to: 1 }
            .for_cell_face(3, true)
            .is_none()
    );
}

#[test]
fn cell_face_orientation_agrees_with_outward_phase_normal_and_cube_normal() {
    let face = Bilinear::new([-1, 1, 1, -1])
        .unwrap()
        .arrangement(&[], || false)
        .unwrap();
    let segment = *face.segments().next().unwrap();
    let cross = |a: [i32; 3], b: [i32; 3]| {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    };
    for axis in 0..3 {
        let [u, v] = transverse_axes(axis);
        let mut normal = [0; 3];
        normal[u] = -2;
        for upper in [false, true] {
            let segment = segment.for_cell_face(axis, upper).unwrap();
            let from = rational_point(face.node(segment.from).unwrap());
            let to = rational_point(face.node(segment.to).unwrap());
            let mut tangent = [0; 3];
            tangent[u] = (to[0].0 * from[0].1 - from[0].0 * to[0].1).signum() as i32;
            tangent[v] = (to[1].0 * from[1].1 - from[1].0 * to[1].1).signum() as i32;
            let mut cube_normal = [0; 3];
            cube_normal[axis] = if upper { 1 } else { -1 };
            let expected = cross(normal, cube_normal);
            assert!(
                tangent
                    .into_iter()
                    .zip(expected)
                    .map(|(a, b)| a * b)
                    .sum::<i32>()
                    > 0
            );
        }
    }
}

#[test]
fn whole_zero_face_retains_interior_insertions_without_inventing_a_curve() {
    let point =
        [positive_root(2, 0, -1), positive_root(3, 0, -1)].map(|r| coord(r, [0, 1], [1, 0]));
    let face = Bilinear::new([0; 4])
        .unwrap()
        .arrangement(&[point], || false)
        .unwrap();
    assert!(face.is_whole_zero());
    assert_eq!(face.nodes().count(), 5);
    assert_eq!(face.segments().count(), 0);
    assert!(
        face.nodes()
            .any(|(_, node)| same_point(node.coordinates, point).unwrap())
    );
}
