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
fn build(values: [[i16; 4]; 2], points: &[[ExactCoordinate; 2]]) -> JointSubdivision {
    let [a, b] = values.map(|v| Bilinear::new(v).unwrap());
    a.joint_subdivision(b, points, || false)
        .unwrap_or_else(|e| panic!("{values:?}: {e:?}"))
}
fn verify_graph(sub: &JointSubdivision) {
    assert!((1..=9).contains(&sub.regions().count()));
    assert_eq!(sub.half_edges().count(), 2 * sub.edges().count());
    let mut incident = BTreeSet::new();
    for (id, edge) in sub.edges() {
        assert!(edge.from < edge.to);
        incident.insert(edge.from);
        incident.insert(edge.to);
        let halves: Vec<_> = sub.half_edges().filter(|(_, h)| h.edge == id).collect();
        assert_eq!(halves.len(), 2);
        if edge.boundary.is_some() {
            assert_eq!(halves.iter().filter(|(_, h)| h.region.is_some()).count(), 1);
        } else {
            assert!(edge.curves != 0);
            assert!(halves.iter().all(|(_, h)| h.region.is_some()));
            assert_ne!(halves[0].1.region, halves[1].1.region);
        }
        for field in 0..2 {
            if edge.curves & (1 << field) != 0 {
                for id in [edge.from, edge.to] {
                    assert_eq!(sub.node_signs(id).unwrap()[field], 0);
                }
            }
        }
    }
    assert_eq!(
        incident.len() + sub.regions().count(),
        sub.edges().count() + 1
    );
    let mut queue = VecDeque::from([0u8]);
    let mut reached = BTreeSet::new();
    while let Some(n) = queue.pop_front() {
        if !reached.insert(n) {
            continue;
        }
        for (_, h) in sub.half_edges().filter(|(_, h)| h.from == n) {
            queue.push_back(h.to);
        }
    }
    assert_eq!(reached, incident);
    let mut visited = BTreeSet::new();
    for (id, region) in sub.regions() {
        let mut at = region.first;
        for _ in 0..region.edge_count {
            assert!(visited.insert(at));
            let h = sub.half_edge(at).unwrap();
            assert_eq!(h.region, Some(id));
            assert_eq!(sub.half_edge(h.next).unwrap().from, h.to);
            at = h.next;
        }
        assert_eq!(at, region.first);
        for (field, sign) in sub.fields.into_iter().zip(region.signs) {
            assert_eq!(sign == 0, field.values == [0; 4]);
        }
    }
    assert_eq!(
        visited.len(),
        sub.half_edges().filter(|(_, h)| h.region.is_some()).count()
    );
    for (id, node) in sub.nodes() {
        if incident.contains(&id) {
            assert_eq!(sub.interior_region(id), None);
        } else {
            assert_eq!(node.boundary, 0);
            let region = sub.regions[usize::from(sub.interior_region(id).unwrap())].unwrap();
            assert_eq!(sub.node_signs(id).unwrap(), region.signs);
        }
    }
}
fn equal(a: &JointSubdivision, b: &JointSubdivision, swap: bool, complement: bool) {
    assert_eq!(a.count, b.count);
    for ((id, p), (other, q)) in a.nodes().zip(b.nodes()) {
        assert_eq!(id, other);
        assert_eq!(
            p.coordinates.map(representation),
            q.coordinates.map(representation)
        );
        assert_eq!(p.boundary, q.boundary);
        assert_eq!(a.interior_region(id), b.interior_region(id));
    }
    assert_eq!(a.edge_count, b.edge_count);
    for ((id, p), (other, q)) in a.edges().zip(b.edges()) {
        assert_eq!(id, other);
        assert_eq!(
            (p.from, p.to, p.geometry, p.boundary, p.inside_forward),
            (q.from, q.to, q.geometry, q.boundary, q.inside_forward)
        );
        assert_eq!(
            q.curves,
            if swap {
                ((p.curves & 1) << 1) | ((p.curves & 2) >> 1)
            } else {
                p.curves
            }
        );
        let mut signs = p.right_signs;
        if swap {
            signs.swap(0, 1);
        }
        if complement {
            signs[0] = -signs[0];
        }
        assert_eq!(q.right_signs, signs);
    }
    assert_eq!(
        a.half_edges().collect::<Vec<_>>(),
        b.half_edges().collect::<Vec<_>>()
    );
    assert_eq!(a.region_count, b.region_count);
    for ((id, p), (other, q)) in a.regions().zip(b.regions()) {
        assert_eq!((id, p.first, p.edge_count), (other, q.first, q.edge_count));
        let mut signs = p.signs;
        if swap {
            signs.swap(0, 1);
        }
        if complement {
            signs[0] = -signs[0];
        }
        assert_eq!(q.signs, signs);
    }
}
fn reference(values: [i16; 4], u: i128, v: i128, q: i128) -> i8 {
    let [a, b, c, d] = values.map(i128::from);
    (a * (q - u) * (q - v) + b * u * (q - v) + c * u * v + d * (q - u) * v).signum() as i8
}
fn verify_grid(sub: &JointSubdivision) {
    for u in 1..4 {
        for v in 1..4 {
            let p = [rational(u, 4), rational(v, 4)];
            let (id, _) = sub
                .nodes()
                .find(|(_, n)| same_point(n.coordinates, p).unwrap())
                .unwrap();
            let signs = sub.fields.map(|f| reference(f.values, u, v, 4));
            assert_eq!(sub.node_signs(id).unwrap(), signs);
            let on_curve = sub
                .fields
                .into_iter()
                .zip(signs)
                .any(|(f, s)| f.values != [0; 4] && s == 0);
            if on_curve {
                assert_eq!(sub.interior_region(id), None);
            } else {
                let region = sub.interior_region(id).unwrap();
                assert_eq!(sub.regions[usize::from(region)].unwrap().signs, signs);
            }
        }
    }
}
#[test]
fn all_ternary_pairs_form_closed_exact_regions_and_argument_independent_geometry() {
    let fields: Vec<_> = (0..81)
        .map(|mut n| {
            std::array::from_fn(|_| {
                let v = n % 3;
                n /= 3;
                v as i16 - 1
            })
        })
        .collect();
    for a in &fields {
        for b in &fields {
            let sub = build([*a, *b], &[]);
            verify_graph(&sub);
            equal(&sub, &build([*b, *a], &[]), true, false);
        }
    }
}
#[test]
fn generated_region_signs_and_interior_constraints_match_independent_bernstein_values() {
    let points: Vec<_> = (1..4)
        .flat_map(|u| (1..4).map(move |v| [rational(u, 4), rational(v, 4)]))
        .collect();
    let mut state = 619u64;
    for case in 0..768 {
        let fields = std::array::from_fn(|slot| {
            std::array::from_fn(|corner| {
                if case < 256 {
                    return if case & (1 << (4 * slot + corner)) == 0 {
                        -255
                    } else {
                        255
                    };
                }
                state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
                (state % 511) as i16 - 255
            })
        });
        let sub = build(fields, &points);
        verify_graph(&sub);
        verify_grid(&sub);
        equal(
            &sub,
            &build(
                [fields[0].map(|v| -v), fields[1]],
                &points.iter().copied().rev().collect::<Vec<_>>(),
            ),
            false,
            true,
        );
    }
}

#[test]
fn distinct_constraints_that_round_to_one_float_keep_separate_interior_ids() {
    let n = (1 << 20) - 3;
    let x = coord(positive_root(n, n, -1), [1, 1], [2, 0]);
    let y = coord(positive_root(n + 2, n, -1), [1, 1], [2, 0]);
    assert_eq!(x.approximate(), y.approximate());
    let points = [[x, rational(1, 2)], [y, rational(1, 2)]];
    let sub = build([[0; 4], [1; 4]], &points);
    verify_graph(&sub);
    let ids = points.map(|p| {
        sub.nodes()
            .find(|(_, n)| same_point(n.coordinates, p).unwrap())
            .unwrap()
            .0
    });
    assert_ne!(ids[0], ids[1]);
    assert_eq!(sub.interior_region(ids[0]), Some(0));
    assert_eq!(sub.interior_region(ids[1]), Some(0));
    equal(
        &sub,
        &build([[0; 4], [1; 4]], &[points[1], points[0]]),
        false,
        false,
    );
}
#[test]
fn separate_curved_arcs_with_identical_endpoints_retain_an_enclosed_region() {
    let fields = [[0, -1, 0, 1], [3, -29, 3, 19]];
    let sub = build(fields, &[]);
    verify_graph(&sub);
    assert_eq!(sub.region_count, 5);
    let mut parallel = false;
    for (id, a) in sub.edges() {
        for (other, b) in sub.edges() {
            if id >= other {
                continue;
            }
            if (a.from, a.to) == (b.from, b.to) {
                assert_ne!(a.geometry, b.geometry);
                parallel = true;
            }
        }
    }
    assert!(parallel);
    assert!(sub.regions().any(|(id, _)| {
        sub.half_edges()
            .filter(|(_, h)| h.region == Some(id))
            .all(|(_, h)| sub.edge(h.edge).unwrap().boundary.is_none())
    }));
}
#[test]
fn tangent_contacts_keep_both_sides_and_order_same_octant_rays() {
    let fields = [[0, -1, 0, 1], [1, -7, 1, 5]];
    let points = [
        [rational(1, 2), rational(7, 16)],
        [rational(1, 2), rational(9, 16)],
    ];
    let sub = build(fields, &points);
    verify_graph(&sub);
    assert_eq!(sub.region_count, 4);
    equal(&sub, &build([fields[1], fields[0]], &points), true, false);
    let (_, contact) = sub
        .nodes()
        .find(|(_, n)| same_point(n.coordinates, [rational(1, 2); 2]).unwrap())
        .unwrap();
    assert_eq!(contact.boundary, 0);
}
#[test]
fn two_factor_crosses_reach_nine_regions_and_preserve_full_boundary_union() {
    let fields = [[1, -2, 4, -2], [4, -2, 1, -2]];
    let mut points = Vec::new();
    for side in 0..4 {
        for k in 1..=20 {
            let t = rational(k, 23);
            points.push(match side {
                0 => [t, rational(0, 1)],
                1 => [rational(1, 1), t],
                2 => [t, rational(1, 1)],
                _ => [rational(0, 1), t],
            });
        }
    }
    let sub = build(fields, &points);
    verify_graph(&sub);
    assert_eq!(sub.region_count, 9);
    assert_eq!(sub.count, 96);
    assert_eq!(sub.edge_count, 104);
    assert!(std::mem::size_of::<JointSubdivision>() < 32768);
    let points: Vec<_> = (1..=80)
        .map(|k| [rational(k, 101), rational(83 - k, 103)])
        .collect();
    let sub = build([[0; 4], [1; 4]], &points);
    verify_graph(&sub);
    assert_eq!(sub.interior.iter().flatten().count(), 80);
}
#[test]
fn unrelated_irrational_constraints_and_shared_curves_keep_exact_identity() {
    let a = positive_root(2, 0, -1);
    let b = positive_root(3, 0, -1);
    let points = [
        [coord(a, [0, 1], [1, 0]), coord(a, [1, -1], [1, 0])],
        [coord(b, [0, 1], [1, 0]), rational(0, 1)],
        [coord(a, [1, 0], [2, 1]), coord(b, [1, 0], [2, 1])],
    ];
    for fields in [
        [[-1, 0, 1, 0], [2, 0, -2, 0]],
        [[0; 4], [0; 4]],
        [[1, -1, 1, -1], [-1, 1, 1, -1]],
    ] {
        let sub = build(fields, &points);
        verify_graph(&sub);
        equal(
            &sub,
            &build(fields, &[points[2], points[1], points[0]]),
            false,
            false,
        );
    }
}
#[test]
fn joint_capacity_and_every_cancellation_return_no_partial_graph() {
    let [a, b] = [[0, -1, 0, 1], [3, -29, 3, 19]].map(|v| Bilinear::new(v).unwrap());
    let points = [[rational(1, 2), rational(3, 4)]];
    let calls = Cell::new(0usize);
    let prior = a
        .joint_subdivision(b, &points, || {
            calls.set(calls.get() + 1);
            false
        })
        .unwrap();
    let count = calls.get();
    for stop in 1..=count {
        let n = Cell::new(0usize);
        assert!(matches!(
            a.joint_subdivision(b, &points, || {
                n.set(n.get() + 1);
                n.get() == stop
            }),
            Err(FieldError::Cancelled)
        ));
        verify_graph(&prior);
    }
    assert!(matches!(
        a.joint_subdivision(b, &[[rational(1, 2); 2]; 81], || false),
        Err(FieldError::Capacity)
    ));
}

#[test]
fn all_square_symmetries_preserve_tangent_lens_and_factor_connectivity() {
    let corners = [[0, 0], [1, 0], [1, 1], [0, 1]];
    for (fields, regions) in [
        ([[0, -1, 0, 1], [1, -7, 1, 5]], 4),
        ([[0, -1, 0, 1], [3, -29, 3, 19]], 5),
        ([[1, -2, 4, -2], [4, -2, 1, -2]], 9),
    ] {
        let original = build(fields, &[]);
        for swap in [false, true] {
            for flip in 0..4 {
                let transform = |mut p: [ExactCoordinate; 2]| {
                    if swap {
                        p.swap(0, 1);
                    }
                    for (axis, v) in p.iter_mut().enumerate() {
                        if flip & (1 << axis) != 0 {
                            *v = ExactCoordinate::unit(
                                v.parameter,
                                v.denominator.subtract(v.numerator),
                                v.denominator,
                            )
                            .unwrap()
                            .unwrap();
                        }
                    }
                    p
                };
                let transformed = fields.map(|values| {
                    let mut out = [0; 4];
                    for (i, mut p) in corners.into_iter().enumerate() {
                        if swap {
                            p.swap(0, 1);
                        }
                        for (axis, v) in p.iter_mut().enumerate() {
                            if flip & (1 << axis) != 0 {
                                *v = 1 - *v;
                            }
                        }
                        out[corners.iter().position(|q| *q == p).unwrap()] = values[i];
                    }
                    out
                });
                let result = build(transformed, &[]);
                verify_graph(&result);
                assert_eq!(result.region_count, regions);
                assert_eq!(result.count, original.count);
                assert_eq!(result.edge_count, original.edge_count);
                let mut remap = [0u8; JOINT_NODES];
                for (id, node) in original.nodes() {
                    remap[usize::from(id)] = result
                        .nodes()
                        .find(|(_, n)| {
                            same_point(n.coordinates, transform(node.coordinates)).unwrap()
                        })
                        .unwrap()
                        .0;
                }
                for (_, edge) in original.edges() {
                    let (a, b) = (remap[usize::from(edge.from)], remap[usize::from(edge.to)]);
                    assert!(
                        result
                            .edges()
                            .any(|(_, e)| (e.from, e.to) == (a.min(b), a.max(b))
                                && e.curves == edge.curves
                                && e.boundary.is_some() == edge.boundary.is_some())
                    );
                }
            }
        }
    }
}
