// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use std::{cell::Cell, collections::BTreeSet};

fn tensor(values: [i16; 8], p: [f64; 3], derivatives: u8) -> f64 {
    values
        .into_iter()
        .enumerate()
        .map(|(i, v)| {
            f64::from(v)
                * (0..3)
                    .map(|a| {
                        if derivatives & (1 << a) != 0 {
                            if i & (1 << a) == 0 { -1. } else { 1. }
                        } else if i & (1 << a) == 0 {
                            1. - p[a]
                        } else {
                            p[a]
                        }
                    })
                    .product::<f64>()
        })
        .sum()
}
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a.into_iter().zip(b).map(|(a, b)| a * b).sum()
}
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn direction(ray: AnchorRay) -> [f64; 3] {
    std::array::from_fn(|a| {
        if a == usize::from(ray.axis) {
            if ray.positive { 1. } else { -1. }
        } else {
            0.
        }
    })
}
fn finite(graph: &CriticalGraph, id: u8) -> AnchorStar {
    let result = graph.anchor_star(id, || false).unwrap();
    assert!(!matches!(result, AnchorStar::UnresolvedFamily { .. }));
    result
}
fn verify(graph: &CriticalGraph) -> (usize, usize) {
    let mut regular = 0;
    let mut doubles = 0;
    for (id, node) in graph.nodes() {
        let star = graph.anchor_star(id, || false).unwrap();
        if graph.requires_family_resolution() {
            assert_eq!(
                star,
                AnchorStar::UnresolvedFamily {
                    node: id,
                    boundary: node.boundary()
                }
            );
            continue;
        }
        let expected: BTreeSet<_> = graph
            .edges()
            .enumerate()
            .filter(|(_, e)| e.from == id || e.to == id)
            .map(|(i, _)| i as u8)
            .collect();
        let mut actual = BTreeSet::new();
        for ray in star.rings().iter().flat_map(|r| r.rays()) {
            assert!(actual.insert(ray.edge));
            let edge = graph.edge(ray.edge).unwrap();
            assert_eq!(if edge.from == id { edge.to } else { edge.from }, ray.to);
            let line = graph.lines().find(|(i, _)| *i == edge.line).unwrap().1;
            assert_eq!(usize::from(ray.axis), line.axis());
            assert_eq!(
                node.coordinates[usize::from(ray.axis)]
                    .compare_same_field(
                        graph.node(ray.to).unwrap().coordinates[usize::from(ray.axis)]
                    )
                    .unwrap()
                    == Ordering::Less,
                ray.positive
            );
        }
        assert_eq!(expected, actual);
        let at = node.approximate().unwrap();
        let gradient: [f64; 3] = std::array::from_fn(|a| tensor(graph.field.values, at, 1 << a));
        assert!(tensor(graph.field.values, at, 0).abs() < 1e-9);
        match star {
            AnchorStar::Regular {
                node: n,
                boundary,
                gradient_signs,
                ring,
            } => {
                regular += 1;
                assert_eq!(n, id);
                assert_eq!(boundary, node.boundary());
                for (sign, value) in gradient_signs.into_iter().zip(gradient) {
                    if sign == 0 {
                        assert!(value.abs() < 1e-8)
                    } else if value.abs() > 1e-9 {
                        assert_eq!(sign, value.signum() as i8)
                    }
                }
                assert!(!ring.rays().is_empty());
                let magnitude = dot(gradient, gradient).sqrt();
                assert!(magnitude > 1e-9);
                let normal = gradient.map(|v| -v / magnitude);
                let first = direction(ring.rays()[0]);
                let mut previous = -1.;
                for ray in ring.rays() {
                    let ray = direction(*ray);
                    assert!(dot(ray, normal).abs() < 1e-8);
                    let angle = dot(normal, cross(first, ray)).atan2(dot(first, ray));
                    let angle = if angle < -1e-12 {
                        angle + std::f64::consts::TAU
                    } else {
                        angle.max(0.)
                    };
                    assert!(angle > previous + 1e-12);
                    previous = angle;
                }
            }
            AnchorStar::OrdinaryDouble {
                node: n,
                boundary,
                mixed_signs,
                rings,
            } => {
                doubles += 1;
                assert_eq!(n, id);
                assert_eq!(boundary, node.boundary());
                assert!(gradient.into_iter().all(|v| v.abs() < 1e-8));
                for (sign, mask) in mixed_signs.into_iter().zip([3, 5, 6]) {
                    assert_eq!(sign, tensor(graph.field.values, at, mask).signum() as i8);
                    assert_ne!(sign, 0);
                }
                for ring in &rings {
                    assert!(ring.rays().len() <= 3);
                    assert_eq!(
                        ring.rays()
                            .iter()
                            .map(|r| r.axis)
                            .collect::<BTreeSet<_>>()
                            .len(),
                        ring.rays().len()
                    );
                    for a in ring.rays() {
                        for b in ring.rays().iter().filter(|b| b.axis != a.axis) {
                            // An independent rational graph arc connects these
                            // axial endpoints iff its two denominator signs agree.
                            let i = usize::from(a.axis);
                            let j = usize::from(b.axis);
                            let k = 3 - i - j;
                            let sa = direction(*a)[i];
                            let sb = direction(*b)[j];
                            let u = tensor(graph.field.values, at, (1 << i) | (1 << k)) * sa;
                            let v = tensor(graph.field.values, at, (1 << j) | (1 << k)) * sb;
                            assert!(u * v > 0.);
                            for t in [0.25, 0.5, 0.75] {
                                let mut p = at;
                                p[i] += sa * t / 65536.;
                                p[j] += sb * (1. - t) / 65536.;
                                let denominator = tensor(graph.field.values, p, 1 << k);
                                assert_ne!(denominator, 0.);
                                p[k] -= tensor(graph.field.values, p, 0) / denominator;
                                assert!(tensor(graph.field.values, p, 0).abs() < 1e-10);
                                assert!(p.into_iter().zip(at).all(|(a, b)| (a - b).abs() < 1e-3));
                            }
                        }
                    }
                    if ring.rays().len() == 3 {
                        for (a, b) in ring
                            .rays()
                            .iter()
                            .zip(ring.rays().iter().cycle().skip(1))
                            .take(3)
                        {
                            let radial = direction(*a);
                            let sample = std::array::from_fn(|i| at[i] + radial[i] / 65536.);
                            let outward = std::array::from_fn(|i| {
                                -tensor(graph.field.values, sample, 1 << i)
                            });
                            assert!(dot(cross(outward, radial), direction(*b)) > 0.);
                        }
                    }
                }
                for a in rings[0].rays() {
                    for b in rings[1].rays().iter().filter(|b| b.axis != a.axis) {
                        let i = usize::from(a.axis);
                        let j = usize::from(b.axis);
                        let k = 3 - i - j;
                        assert!(
                            tensor(graph.field.values, at, (1 << i) | (1 << k))
                                * direction(*a)[i]
                                * tensor(graph.field.values, at, (1 << j) | (1 << k))
                                * direction(*b)[j]
                                < 0.
                        );
                    }
                }
            }
            AnchorStar::UnresolvedFamily { .. } => panic!("Finite graph required"),
        }
    }
    (regular, doubles)
}
fn cone(mixed: [i16; 3], cubic: i16, center_twice: [i16; 3]) -> Trilinear {
    Trilinear::new(std::array::from_fn(|i| {
        let [x, y, z] = std::array::from_fn(|a| 2 * ((i >> a) & 1) as i16 - center_twice[a]);
        mixed[0] * x * y + mixed[1] * x * z + mixed[2] * y * z + cubic * x * y * z
    }))
    .unwrap()
}

#[test]
fn anchor_stars_cover_all_ternary_and_generated_amplitude_graphs() {
    let mut regular = 0;
    let mut doubles = 0;
    for mut code in 0..6561 {
        let field = Trilinear::new(std::array::from_fn(|_| {
            let value = code % 3;
            code /= 3;
            value as i16 - 1
        }))
        .unwrap();
        let (r, d) = verify(&field.critical_graph(|| false).unwrap());
        regular += r;
        doubles += d;
    }
    let mut seed = 173u32;
    for _ in 0..512 {
        let field = Trilinear::new(std::array::from_fn(|_| {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            (seed % 511) as i16 - 255
        }))
        .unwrap();
        let (r, d) = verify(&field.critical_graph(|| false).unwrap());
        regular += r;
        doubles += d;
    }
    assert!(regular > 1000);
    assert!(doubles > 0);
}

#[test]
fn anchor_stars_keep_two_cone_rings_with_cubic_terms_and_all_boundary_clipping() {
    let mut counts = [0usize; 7];
    for signs in 0..8 {
        for amplitudes in [[1, 1, 1], [1, 2, 3], [3, 1, 2], [2, 3, 1]] {
            let mixed =
                std::array::from_fn(|a| amplitudes[a] * if signs & (1 << a) == 0 { 1 } else { -1 });
            for cubic in [-3, 0, 3] {
                for x in 0..3 {
                    for y in 0..3 {
                        for z in 0..3 {
                            let field = cone(mixed, cubic, [x, y, z]);
                            let graph = field.critical_graph(|| false).unwrap();
                            assert!(!graph.requires_family_resolution());
                            let (_, doubles) = verify(&graph);
                            assert_eq!(doubles, 1);
                            let (id, node) =
                                graph.nodes().find(|(_, n)| n.critical_axes == 7).unwrap();
                            assert_eq!(
                                node.approximate().unwrap(),
                                [x, y, z].map(|v| f64::from(v) / 2.)
                            );
                            let star = finite(&graph, id);
                            counts[star.rings().iter().map(|r| r.rays().len()).sum::<usize>()] += 1;
                        }
                    }
                }
            }
        }
    }
    assert!(counts[3..=6].iter().all(|n| *n > 0));
}

fn signature(star: AnchorStar, perm: [usize; 3], flips: u8, reverse: bool) -> Vec<Vec<(u8, bool)>> {
    let mut rings: Vec<_> = star
        .rings()
        .iter()
        .map(|r| {
            let mut rays: Vec<_> = r
                .rays()
                .iter()
                .map(|r| {
                    let axis = perm.iter().position(|a| *a == usize::from(r.axis)).unwrap();
                    (axis as u8, r.positive != (flips & (1 << axis) != 0))
                })
                .collect();
            if reverse {
                rays.reverse();
            }
            if let Some((first, _)) = rays.iter().enumerate().min_by_key(|(_, r)| **r) {
                rays.rotate_left(first);
            }
            rays
        })
        .collect();
    rings.sort();
    rings
}
#[test]
fn anchor_stars_transform_exactly_under_axes_reflections_and_complement() {
    for field in [
        Trilinear::new([13, -3, -3, -3, -3, -3, -3, 13]).unwrap(),
        cone([-1, 2, 3], 3, [1; 3]),
        cone([1, -2, 3], -3, [0, 1, 2]),
    ] {
        let graph = field.critical_graph(|| false).unwrap();
        for perm in [
            [0, 1, 2],
            [0, 2, 1],
            [1, 0, 2],
            [1, 2, 0],
            [2, 0, 1],
            [2, 1, 0],
        ] {
            let inversions = (0..3)
                .flat_map(|a| (a + 1..3).map(move |b| (a, b)))
                .filter(|(a, b)| perm[*a] > perm[*b])
                .count();
            for flips in 0..8u8 {
                for complement in [false, true] {
                    let mut values = [0; 8];
                    for (i, v) in field.values.into_iter().enumerate() {
                        let j = (0..3)
                            .map(|a| {
                                (((i >> perm[a]) & 1) ^ usize::from(flips & (1 << a) != 0)) << a
                            })
                            .sum::<usize>();
                        values[j] = if complement { -v } else { v };
                    }
                    let other = Trilinear::new(values)
                        .unwrap()
                        .critical_graph(|| false)
                        .unwrap();
                    verify(&other);
                    for (id, node) in graph.nodes() {
                        let transformed = std::array::from_fn::<_, 3, _>(|a| {
                            let c = node.coordinates[perm[a]];
                            if flips & (1 << a) == 0 {
                                c
                            } else {
                                ExactCoordinate::unit(
                                    c.parameter,
                                    c.denominator.subtract(c.numerator),
                                    c.denominator,
                                )
                                .unwrap()
                                .unwrap()
                            }
                        });
                        let (other_id, _) = other
                            .nodes()
                            .find(|(_, n)| {
                                n.coordinates.into_iter().zip(transformed).all(|(a, b)| {
                                    a.compare_same_field(b).unwrap() == Ordering::Equal
                                })
                            })
                            .unwrap();
                        let reverse =
                            !(inversions + flips.count_ones() as usize + usize::from(complement))
                                .is_multiple_of(2);
                        assert_eq!(
                            signature(finite(&graph, id), perm, flips, reverse),
                            signature(finite(&other, other_id), [0, 1, 2], 0, false)
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn anchor_stars_preserve_graphs_at_every_cancellation_and_invalid_request() {
    for field in [
        Trilinear::new([13, -3, -3, -3, -3, -3, -3, 13]).unwrap(),
        cone([-1, 2, 3], 3, [1; 3]),
    ] {
        let graph = field.critical_graph(|| false).unwrap();
        let edges: Vec<_> = graph.edges().copied().collect();
        for (id, _) in graph.nodes() {
            let calls = Cell::new(0);
            let prior = graph
                .anchor_star(id, || {
                    calls.set(calls.get() + 1);
                    false
                })
                .unwrap();
            let count = calls.get();
            for stop in 1..=count {
                calls.set(0);
                assert_eq!(
                    graph.anchor_star(id, || {
                        calls.set(calls.get() + 1);
                        calls.get() == stop
                    }),
                    Err(FieldError::Cancelled)
                );
                assert_eq!(graph.anchor_star(id, || false).unwrap(), prior);
                assert_eq!(graph.edges().copied().collect::<Vec<_>>(), edges);
            }
        }
        assert_eq!(graph.anchor_star(255, || false), Err(FieldError::Bounds));
        assert_eq!(graph.anchor_star(255, || true), Err(FieldError::Cancelled));
        assert_eq!(graph.edge(255), None);
    }
    assert!(std::mem::size_of::<AnchorStar>() < 128);
}

#[test]
fn anchor_stars_keep_family_diagnostics_explicit() {
    for field in [cone([0, 1, 1], 2, [1; 3]), Trilinear::new([0; 8]).unwrap()] {
        let graph = field.critical_graph(|| false).unwrap();
        assert!(graph.requires_family_resolution());
        verify(&graph);
    }
    let mut graph = cone([1, 2, 3], 1, [1; 3]).critical_graph(|| false).unwrap();
    graph.projection_families = 1;
    for (id, node) in graph.nodes() {
        assert_eq!(
            graph.anchor_star(id, || false).unwrap(),
            AnchorStar::UnresolvedFamily {
                node: id,
                boundary: node.boundary()
            }
        );
        assert_eq!(graph.anchor_star(id, || true), Err(FieldError::Cancelled));
    }
}

#[test]
fn anchor_stars_refuse_missing_duplicate_and_inconsistent_incidence() {
    for defect in 0..7 {
        let mut graph = cone([1, 2, 3], 1, [1; 3]).critical_graph(|| false).unwrap();
        let id = graph.nodes().find(|(_, n)| n.critical_axes == 7).unwrap().0;
        let edge_index = graph
            .edges
            .iter()
            .position(|e| e.is_some_and(|e| e.from == id || e.to == id))
            .unwrap();
        let edge = graph.edges[edge_index].unwrap();
        match defect {
            0 => graph.edges[edge_index] = None,
            1 => graph.edges[graph.edge_count] = Some(edge),
            2 => graph.edges[edge_index].as_mut().unwrap().line = 255,
            3 => {
                let edge = graph.edges[edge_index].as_mut().unwrap();
                if edge.from == id {
                    edge.to = 255;
                } else {
                    edge.from = 255;
                }
            }
            4 => {
                let to = if edge.from == id { edge.to } else { edge.from };
                graph.nodes[usize::from(to)].as_mut().unwrap().coordinates =
                    graph.node(id).unwrap().coordinates;
            }
            5 => graph.nodes[usize::from(id)].as_mut().unwrap().boundary = 3,
            _ => graph.field = Trilinear::new([1; 8]).unwrap(),
        }
        assert!(matches!(
            graph.anchor_star(id, || false),
            Err(FieldError::Arithmetic)
        ));
    }
}
