// SPDX-License-Identifier: GPL-3.0-or-later
use super::super::super::tests::{coord, positive_root};
use super::*;
use std::{
    cell::Cell,
    collections::{BTreeSet, VecDeque},
};

fn rational(n: i128, d: i128) -> ExactCoordinate {
    rational_coordinate(Rational::new(n, d).unwrap()).unwrap()
}
fn geometry_equal(a: &PlaneSubdivision, b: &PlaneSubdivision, complement: bool) {
    assert_eq!(a.nodes().count(), b.nodes().count());
    for ((ai, a), (bi, b)) in a.nodes().zip(b.nodes()) {
        assert_eq!(ai, bi);
        assert!(same_point(a.coordinates, b.coordinates).unwrap());
        assert_eq!(a.boundary, b.boundary);
        assert_eq!(
            a.coordinates.map(representation),
            b.coordinates.map(representation)
        );
    }
    assert_eq!(a.edge_count, b.edge_count);
    for ((ai, a), (bi, b)) in a.edges().zip(b.edges()) {
        assert_eq!(ai, bi);
        assert_eq!(
            (a.from, a.to, a.boundary, a.zero_curve, a.inside_forward),
            (b.from, b.to, b.boundary, b.zero_curve, b.inside_forward)
        );
        assert_eq!(
            b.right_sign,
            if complement {
                -a.right_sign
            } else {
                a.right_sign
            }
        );
    }
    assert_eq!(
        a.half_edges().collect::<Vec<_>>(),
        b.half_edges().collect::<Vec<_>>()
    );
    for ((ai, a), (bi, b)) in a.regions().zip(b.regions()) {
        assert_eq!(ai, bi);
        assert_eq!((a.first, a.edge_count), (b.first, b.edge_count));
        assert_eq!(b.sign, if complement { -a.sign } else { a.sign });
    }
    assert_eq!(a.regions().count(), b.regions().count());
}
fn verify_graph(sub: &PlaneSubdivision) {
    assert!((1..=4).contains(&sub.regions().count()));
    assert_eq!(sub.half_edges().count(), 2 * sub.edges().count());
    let mut incident = BTreeSet::new();
    for (id, edge) in sub.edges() {
        assert!(edge.from < edge.to);
        incident.insert(edge.from);
        incident.insert(edge.to);
        let halves: Vec<_> = sub.half_edges().filter(|(_, h)| h.edge == id).collect();
        assert_eq!(halves.len(), 2);
        assert_eq!(
            (halves[0].1.from, halves[0].1.to),
            (halves[1].1.to, halves[1].1.from)
        );
        if edge.boundary.is_some() {
            assert_eq!(halves.iter().filter(|(_, h)| h.region.is_some()).count(), 1);
        } else {
            assert!(edge.zero_curve);
            assert!(halves.iter().all(|(_, h)| h.region.is_some()));
            assert_ne!(halves[0].1.region, halves[1].1.region);
        }
    }
    assert_eq!(
        incident.len() + sub.regions().count(),
        sub.edges().count() + 1
    );
    let mut visited = BTreeSet::new();
    let mut queue = VecDeque::from([0u8]);
    while let Some(node) = queue.pop_front() {
        if !visited.insert(node) {
            continue;
        }
        for (_, h) in sub.half_edges().filter(|(_, h)| h.from == node) {
            queue.push_back(h.to);
        }
    }
    assert_eq!(visited, incident);
    for (id, half) in sub.half_edges() {
        let twin = sub.half_edge(half.twin).unwrap();
        assert_eq!(twin.twin, id);
        assert_eq!((half.from, half.to), (twin.to, twin.from));
        let next = sub.half_edge(half.next).unwrap();
        assert_eq!(next.from, half.to);
        assert_eq!(next.region, half.region);
    }
    let mut used = BTreeSet::new();
    for (id, region) in sub.regions() {
        let mut half = region.first;
        let mut count = 0;
        loop {
            assert!(used.insert(half));
            let at = sub.half_edge(half).unwrap();
            assert_eq!(at.region, Some(id));
            count += 1;
            half = at.next;
            if half == region.first {
                break;
            }
            assert!(count <= sub.half_edges().count());
        }
        assert_eq!(count, usize::from(region.edge_count));
    }
    assert_eq!(
        used.len(),
        sub.half_edges().filter(|(_, h)| h.region.is_some()).count()
    );
}
fn reference_value(field: Bilinear, [(u, uq), (v, vq)]: [(i128, i128); 2]) -> i128 {
    let [a, b, c, d] = field.values.map(i128::from);
    a * (uq - u) * (vq - v) + b * u * (vq - v) + c * u * v + d * (uq - u) * v
}
fn verify_region_signs(sub: &PlaneSubdivision) {
    // Every bilinear region touches a square edge. At a midpoint between its
    // rational breakpoints, move inward by2^-40. With <=255 corner magnitudes
    // and <=1020 boundary denominators this cannot pass another zero curve.
    for (region_id, region) in sub.regions() {
        let (_, half) = sub
            .half_edges()
            .find(|(_, h)| {
                h.region == Some(region_id) && sub.edge(h.edge).unwrap().boundary.is_some()
            })
            .unwrap();
        let edge = sub.edge(half.edge).unwrap();
        let a = sub
            .node(edge.from)
            .unwrap()
            .coordinates
            .map(|v| v.rational_value().unwrap());
        let b = sub
            .node(edge.to)
            .unwrap()
            .coordinates
            .map(|v| v.rational_value().unwrap());
        let mut p = std::array::from_fn(|i| {
            let n = i128::from(a[i].numerator) * i128::from(b[i].denominator)
                + i128::from(b[i].numerator) * i128::from(a[i].denominator);
            let d = 2 * i128::from(a[i].denominator) * i128::from(b[i].denominator);
            (n, d)
        });
        let epsilon = 1i128 << 40;
        match edge.boundary.unwrap() {
            0 => p[1] = (1, epsilon),
            1 => p[0] = (epsilon - 1, epsilon),
            2 => p[1] = (epsilon - 1, epsilon),
            _ => p[0] = (1, epsilon),
        }
        assert_eq!(
            reference_value(sub.field(), p).signum() as i8,
            region.sign,
            "{:?} {p:?}",
            sub.field()
        );
    }
}

#[test]
fn subdivisions_keep_all_regions_for_lines_crosses_contacts_and_whole_fields() {
    for (values, signs) in [
        ([1; 4], vec![1]),
        ([-1; 4], vec![-1]),
        ([0; 4], vec![0]),
        ([-1, 1, 1, -1], vec![-1, 1]),
        ([-1, -1, 1, 1], vec![-1, 1]),
        ([1, -1, 1, -1], vec![-1, -1, 1, 1]),
        ([0, -1, 1, 0], vec![-1, 1]),
        ([0, 0, 1, 0], vec![1]),
        ([0, 1, 0, 1], vec![1]),
        ([0, 1, 2, 1], vec![1]),
        ([1, -1, 2, -1], vec![-1, -1, 1]),
        ([1, -2, 1, -2], vec![-1, 1, 1]),
    ] {
        let sub = Bilinear::new(values)
            .unwrap()
            .subdivision(&[], &[], || false)
            .unwrap();
        let mut actual: Vec<_> = sub.regions().map(|(_, r)| r.sign).collect();
        actual.sort();
        assert_eq!(actual, signs, "{values:?}");
        verify_graph(&sub);
        verify_region_signs(&sub);
    }
}

#[test]
fn generated_subdivisions_preserve_closed_loops_euler_signs_and_complement() {
    let mut fields = Vec::new();
    for mut code in 0..81 {
        fields.push(std::array::from_fn(|_| {
            let v = code % 3;
            code /= 3;
            v as i16 - 1
        }));
    }
    let mut state = 149u64;
    for _ in 0..2048 {
        fields.push(std::array::from_fn(|_| {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (state % 511) as i16 - 255
        }));
    }
    for values in fields {
        let sub = Bilinear::new(values)
            .unwrap()
            .subdivision(&[], &[], || false)
            .unwrap();
        verify_graph(&sub);
        verify_region_signs(&sub);
        let complement = Bilinear::new(values.map(|v| -v))
            .unwrap()
            .subdivision(&[], &[], || false)
            .unwrap();
        geometry_equal(&sub, &complement, true);
    }
}

#[test]
fn nonzero_boundary_points_and_unrelated_curve_points_survive_input_reordering() {
    let a = positive_root(2, 0, -1);
    let b = positive_root(3, 0, -1);
    let boundary = [
        [rational(1, 4), rational(0, 1)],
        [rational(1, 1), coord(a, [0, 1], [1, 0])],
        [rational(1, 3), rational(1, 1)],
        [rational(0, 1), coord(b, [0, 1], [1, 0])],
    ];
    let contour = [
        [coord(a, [0, 1], [1, 0]), coord(a, [1, -1], [1, 0])],
        [coord(b, [0, 1], [1, 0]), coord(b, [1, -1], [1, 0])],
    ];
    let field = Bilinear::new([-1, 0, 1, 0]).unwrap();
    let sub = field.subdivision(&boundary, &contour, || false).unwrap();
    verify_graph(&sub);
    assert_eq!(sub.nodes().count(), 10);
    assert_eq!(sub.regions().count(), 2);
    assert!(
        sub.nodes()
            .any(|(id, n)| same_point(n.coordinates, boundary[0]).unwrap()
                && sub.node_sign(id) == Ok(-1))
    );
    assert_eq!(sub.node_sign(u8::MAX), Err(FieldError::Bounds));
    for point in boundary.into_iter().chain(contour) {
        assert!(
            sub.nodes()
                .any(|(_, n)| same_point(n.coordinates, point).unwrap())
        );
    }
    let mut reordered = boundary;
    reordered.reverse();
    let mut reversed = contour;
    reversed.reverse();
    geometry_equal(
        &sub,
        &field.subdivision(&reordered, &reversed, || false).unwrap(),
        false,
    );
    for (_, r) in sub.regions() {
        assert!(matches!(r.sign, -1 | 1));
    }
}

#[test]
fn whole_zero_interior_constraints_remain_isolated_without_invented_cuts() {
    let points = [
        [rational(1, 3), rational(2, 3)],
        [rational(3, 4), rational(1, 4)],
    ];
    let sub = Bilinear::new([0; 4])
        .unwrap()
        .subdivision(&[], &points, || false)
        .unwrap();
    assert_eq!(sub.nodes().count(), 6);
    assert_eq!(sub.edges().count(), 4);
    assert_eq!(sub.regions().count(), 1);
    assert_eq!(sub.regions().next().unwrap().1.sign, 0);
    for point in points {
        let id = sub
            .nodes()
            .find(|(_, n)| same_point(n.coordinates, point).unwrap())
            .unwrap()
            .0;
        assert!(sub.half_edges().all(|(_, h)| h.from != id && h.to != id));
    }
    verify_graph(&sub);
    verify_region_signs(&sub);
}

#[test]
fn maximum_boundary_union_reaches_fixed_node_edge_and_half_edge_bounds() {
    let boundary: Vec<_> = (1..=80)
        .map(|k| [rational(0, 1), rational(k, 81)])
        .collect();
    let field = Bilinear::new([1, -1, 1, -1]).unwrap();
    let sub = field.subdivision(&boundary, &[], || false).unwrap();
    assert_eq!(sub.nodes().count(), 89);
    assert_eq!(sub.edges().count(), 92);
    assert_eq!(sub.half_edges().count(), 184);
    assert_eq!(sub.regions().count(), 4);
    verify_graph(&sub);
    verify_region_signs(&sub);
    let mut reversed = boundary;
    reversed.reverse();
    geometry_equal(
        &sub,
        &field.subdivision(&reversed, &[], || false).unwrap(),
        false,
    );
}

#[test]
fn subdivision_refuses_invalid_inputs_and_cancels_without_partial_publication() {
    let field = Bilinear::new([-1, 0, 1, 0]).unwrap();
    let boundary = [[rational(1, 4), rational(0, 1)]];
    let contour = [[rational(1, 2); 2]];
    let calls = Cell::new(0);
    let prior = field
        .subdivision(&boundary, &contour, || {
            calls.set(calls.get() + 1);
            false
        })
        .unwrap();
    assert!(std::mem::size_of::<PlaneSubdivision>() <= 16384);
    for cancel_at in 0..calls.get() {
        let call = Cell::new(0);
        assert!(matches!(
            field.subdivision(&boundary, &contour, || {
                let n = call.get();
                call.set(n + 1);
                n == cancel_at
            }),
            Err(FieldError::Cancelled)
        ));
        geometry_equal(
            &prior,
            &field.subdivision(&boundary, &contour, || false).unwrap(),
            false,
        );
    }
    assert!(matches!(
        field.subdivision(&[[rational(1, 4); 2]], &[], || false),
        Err(FieldError::Contour)
    ));
    assert!(matches!(
        field.subdivision(&[], &[[rational(0, 1); 2]], || false),
        Err(FieldError::Contour)
    ));
    assert!(matches!(
        field.subdivision(&[boundary[0]; 81], &[], || false),
        Err(FieldError::Capacity)
    ));
}
