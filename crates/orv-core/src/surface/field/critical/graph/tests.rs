// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use std::{cell::Cell, collections::BTreeSet};

fn positive_root(a: i64, b: i64, c: i64) -> AlgebraicRoot {
    let Roots::Finite(values) = roots(SectionDeterminant {
        quadratic: a,
        linear: b,
        constant: c,
    })
    .unwrap() else {
        panic!("Finite roots required")
    };
    values
        .into_iter()
        .flatten()
        .find(|r| r.sign_linear(0, 1) > 0)
        .unwrap()
}
fn coord(root: AlgebraicRoot, numerator: [i32; 2], denominator: [i32; 2]) -> ExactCoordinate {
    ExactCoordinate::unit(
        root,
        Linear {
            constant: numerator[0],
            slope: numerator[1],
        },
        Linear {
            constant: denominator[0],
            slope: denominator[1],
        },
    )
    .unwrap()
    .unwrap()
}
fn rational(n: i64, d: i128) -> ExactCoordinate {
    ExactCoordinate::parameter(AlgebraicRoot(Root::Rational(
        Rational::new(i128::from(n), d).unwrap(),
    )))
    .unwrap()
    .unwrap()
}

#[test]
fn equivalent_coordinates_join_across_quadratic_scalings_and_denominator_signs() {
    let a = positive_root(2, 0, -1);
    let b = positive_root(8, 0, -1);
    let x = coord(a, [0, 1], [1, 0]);
    let y = coord(b, [0, 2], [1, 0]);
    let negative = coord(b, [0, -2], [-1, 0]);
    assert_eq!(x.compare_same_field(y), Ok(Ordering::Equal));
    assert_eq!(x.compare_same_field(negative), Ok(Ordering::Equal));
    for other in [rational(0, 1), rational(1, 2), rational(1, 1)] {
        assert_eq!(
            x.compare_same_field(other),
            negative.compare_same_field(other)
        );
        assert_eq!(
            x.compare_same_field(other).unwrap(),
            other.compare_same_field(x).unwrap().reverse()
        );
    }
    let constant_a = coord(a, [-1, -2], [-2, -4]);
    let constant_b = coord(positive_root(3, 0, -1), [1, 2], [2, 4]);
    assert_eq!(constant_a.rational_value().unwrap().as_f64(), 0.5);
    assert_eq!(
        constant_a.compare_same_field(constant_b),
        Ok(Ordering::Equal)
    );
    assert_eq!(
        constant_a.compare_same_field(rational(1, 2)),
        Ok(Ordering::Equal)
    );
    assert!(
        x.compare_same_field(coord(positive_root(3, 0, -1), [0, 1], [1, 0]))
            .is_err()
    );
}

#[test]
fn rounded_equal_coordinates_remain_distinct_under_exact_comparison() {
    let value = coord(positive_root(2, 0, -1), [0, 1], [1, 0]);
    let (mut p, mut q) = (1i64, 1i64);
    let mut rounded_ties = 0;
    for _ in 0..30 {
        (p, q) = (p + 2 * q, p + q);
        let other = rational(q, i128::from(p));
        let residual = 2 * i128::from(q) * i128::from(q) - i128::from(p) * i128::from(p);
        assert_eq!(residual.abs(), 1);
        let expected = 0i128.cmp(&residual);
        assert_eq!(value.compare_same_field(other), Ok(expected));
        if value.approximate() == other.approximate() {
            rounded_ties += 1;
        }
    }
    assert!(rounded_ties > 0);
}

fn tensor(values: [i16; 8], point: [f64; 3]) -> f64 {
    values
        .into_iter()
        .enumerate()
        .map(|(i, v)| {
            f64::from(v)
                * (0..3)
                    .map(|a| {
                        if i & (1 << a) == 0 {
                            1. - point[a]
                        } else {
                            point[a]
                        }
                    })
                    .product::<f64>()
        })
        .sum()
}
fn verify(graph: &CriticalGraph) {
    for (_, node) in graph.nodes() {
        let point = node.approximate().unwrap();
        assert!(
            tensor(graph.field().values(), point).abs() < 1e-8,
            "{:?}, {point:?}",
            graph.field()
        );
        for value in node.coordinates() {
            assert!(value.compare_same_field(rational(0, 1)).unwrap() != Ordering::Less);
            assert!(value.compare_same_field(rational(1, 1)).unwrap() != Ordering::Greater);
        }
    }
    for (index, line) in graph.lines() {
        let edges: Vec<_> = graph.edges().filter(|e| e.line == index).collect();
        assert!(!edges.is_empty());
        let first = graph.node(edges[0].from).unwrap();
        let last = graph.node(edges.last().unwrap().to).unwrap();
        assert_eq!(
            first.coordinates()[line.axis()].compare_same_field(rational(0, 1)),
            Ok(Ordering::Equal)
        );
        assert_eq!(
            last.coordinates()[line.axis()].compare_same_field(rational(1, 1)),
            Ok(Ordering::Equal)
        );
        for pair in edges.windows(2) {
            assert_eq!(pair[0].to, pair[1].from);
        }
        for edge in edges {
            let from = graph.node(edge.from).unwrap();
            let to = graph.node(edge.to).unwrap();
            assert!(line.contains(from).unwrap() && line.contains(to).unwrap());
            assert_eq!(
                from.coordinates()[line.axis()].compare_same_field(to.coordinates()[line.axis()]),
                Ok(Ordering::Less)
            );
            let a = from.approximate().unwrap();
            let b = to.approximate().unwrap();
            for t in [0., 0.25, 0.5, 0.75, 1.] {
                let point = std::array::from_fn(|i| a[i] * (1. - t) + b[i] * t);
                assert!(tensor(graph.field().values(), point).abs() < 1e-8);
            }
        }
    }
    for (_, node) in graph.nodes() {
        for normal in 0..3 {
            if node.critical_axes() & (1 << normal) == 0 {
                continue;
            }
            for axis in transverse_axes(normal) {
                if graph.unresolved_line_axes() & (1 << axis) == 0 {
                    assert!(
                        graph
                            .lines()
                            .any(|(_, line)| line.axis() == axis && line.contains(node).unwrap()),
                        "Missing zero line at {:?}",
                        node.approximate()
                    );
                }
            }
        }
    }
}
fn connected(graph: &CriticalGraph) -> usize {
    let mut visited = BTreeSet::new();
    let mut components = 0;
    for (start, _) in graph.nodes() {
        if !visited.insert(start) {
            continue;
        }
        components += 1;
        let mut pending = vec![start];
        while let Some(node) = pending.pop() {
            for edge in graph.edges() {
                let next = if edge.from == node {
                    Some(edge.to)
                } else if edge.to == node {
                    Some(edge.from)
                } else {
                    None
                };
                if let Some(next) = next
                    && visited.insert(next)
                {
                    pending.push(next);
                }
            }
        }
    }
    components
}

#[test]
fn six_point_graph_and_singular_contact_keep_their_exact_zero_line_structure() {
    for values in [
        [13, -3, -3, -3, -3, -3, -3, 13],
        [10, -2, -2, -2, -2, -2, -2, 10],
    ] {
        let field = Trilinear::new(values).unwrap();
        let graph = field.critical_graph(|| false).unwrap();
        assert!(!graph.requires_family_resolution());
        assert_eq!(graph.nodes().count(), 18);
        assert_eq!(
            graph
                .nodes()
                .filter(|(_, n)| n.critical_axes() != 0)
                .count(),
            6
        );
        assert_eq!(graph.lines().count(), 6);
        assert_eq!(graph.edges().count(), 18);
        assert_eq!(connected(&graph), 1);
        verify(&graph);
        let complement = Trilinear::new(values.map(|v| -v))
            .unwrap()
            .critical_graph(|| false)
            .unwrap();
        assert_eq!(
            graph.edges().copied().collect::<Vec<_>>(),
            complement.edges().copied().collect::<Vec<_>>()
        );
        for ((_, a), (_, b)) in graph.nodes().zip(complement.nodes()) {
            assert_eq!(a.critical_axes(), b.critical_axes());
            assert_eq!(a.boundary(), b.boundary());
            for (a, b) in a.coordinates().iter().zip(b.coordinates()) {
                assert_eq!(a.compare_same_field(*b), Ok(Ordering::Equal));
            }
        }
    }
    let graph = Trilinear::new([3, -1, -1, -1, -1, -1, -1, 3])
        .unwrap()
        .critical_graph(|| false)
        .unwrap();
    assert!(!graph.requires_family_resolution());
    assert_eq!(graph.nodes().count(), 7);
    assert_eq!(graph.lines().count(), 3);
    assert_eq!(graph.edges().count(), 6);
    let critical: Vec<_> = graph
        .nodes()
        .filter(|(_, n)| n.critical_axes() != 0)
        .collect();
    assert_eq!(critical.len(), 1);
    assert_eq!(critical[0].1.critical_axes(), 7);
    verify(&graph);
}

#[test]
fn zero_lines_survive_when_their_critical_intersection_lies_outside_the_cell() {
    let field = Trilinear::new([1, 0, 3, 2, -3, -2, -1, 0]).unwrap(); // 2y-1+(x-2)*(2z-1)
    let graph = field.critical_graph(|| false).unwrap();
    assert!(!graph.requires_family_resolution());
    assert_eq!(
        graph
            .nodes()
            .filter(|(_, n)| n.critical_axes() != 0)
            .count(),
        0
    );
    assert_eq!(graph.lines().count(), 1);
    assert_eq!(graph.nodes().count(), 2);
    assert_eq!(graph.edges().count(), 1);
    let line = graph.lines().next().unwrap().1;
    assert_eq!(line.axis(), 0);
    assert_eq!(
        line.fixed_coordinates()
            .map(|v| v.rational_value().unwrap().as_f64()),
        [0.5; 2]
    );
    verify(&graph);
}

#[test]
fn graph_preserves_boundary_crosses_whole_sections_and_unresolved_families() {
    let graph = Trilinear::new([1, -1, -1, 1, 2, 0, 0, 2])
        .unwrap()
        .critical_graph(|| false)
        .unwrap();
    assert!(!graph.requires_family_resolution());
    assert_eq!(graph.nodes().count(), 5);
    assert_eq!(graph.lines().count(), 2);
    assert_eq!(graph.edges().count(), 4);
    assert!(graph.nodes().all(|(_, n)| n.boundary() & (1 << 4) != 0));
    verify(&graph);
    let plane = Trilinear::new([-1, -2, -2, -3, 1, 2, 2, 3])
        .unwrap()
        .critical_graph(|| false)
        .unwrap();
    assert!(plane.requires_family_resolution());
    assert_eq!(plane.whole_sections().count(), 1);
    let section = plane.whole_sections().next().unwrap();
    assert_eq!(section.axis, 2);
    assert_eq!(section.parameter.approximate(), 0.5);
    for values in [[0; 8], [1, -1, -1, 1, 1, -1, -1, 1]] {
        let graph = Trilinear::new(values)
            .unwrap()
            .critical_graph(|| false)
            .unwrap();
        assert!(graph.requires_family_resolution());
    }
    for values in [[1; 8], [-255; 8]] {
        let graph = Trilinear::new(values)
            .unwrap()
            .critical_graph(|| false)
            .unwrap();
        assert!(!graph.requires_family_resolution());
        assert_eq!(graph.nodes().count(), 0);
        assert_eq!(graph.edges().count(), 0);
    }
}

#[test]
fn graph_is_bounded_cancellable_and_keeps_an_earlier_result_unchanged() {
    let field = Trilinear::new([10, -2, -2, -2, -2, -2, -2, 10]).unwrap();
    let checks = Cell::new(0);
    let retained = field
        .critical_graph(|| {
            checks.set(checks.get() + 1);
            false
        })
        .unwrap();
    let count = checks.get();
    for stop in 0..count {
        let at = Cell::new(0);
        let result = field.critical_graph(|| {
            let n = at.get();
            at.set(n + 1);
            n == stop
        });
        assert!(matches!(result, Err(FieldError::Cancelled)));
        assert_eq!(retained.nodes().count(), 18);
        assert_eq!(retained.edges().count(), 18);
    }
    assert!(std::mem::size_of::<CriticalGraph>() <= 4096);
    verify(&retained);
}

#[test]
fn generated_graphs_keep_source_zero_lines_and_exact_joins_under_axis_transforms() {
    let permutations = [
        [0, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ];
    let base = Trilinear::new([10, -2, -2, -2, -2, -2, -2, 10])
        .unwrap()
        .critical_graph(|| false)
        .unwrap();
    for permutation in permutations {
        for flips in 0..8 {
            let values = std::array::from_fn(|i| {
                let mut original = 0;
                for (a, p) in permutation.into_iter().enumerate() {
                    original |= (((i >> a) & 1) ^ ((flips >> a) & 1)) << p;
                }
                base.field().values()[original]
            });
            let graph = Trilinear::new(values)
                .unwrap()
                .critical_graph(|| false)
                .unwrap();
            assert_eq!(graph.nodes().count(), 18);
            assert_eq!(graph.edges().count(), 18);
            assert_eq!(connected(&graph), 1);
            verify(&graph);
            for (_, node) in base.nodes() {
                let point = node.approximate().unwrap();
                let expected: [f64; 3] = std::array::from_fn(|a| {
                    if flips & (1 << a) == 0 {
                        point[permutation[a]]
                    } else {
                        1. - point[permutation[a]]
                    }
                });
                assert!(graph.nodes().any(|(_, n)| {
                    n.approximate()
                        .unwrap()
                        .into_iter()
                        .zip(expected)
                        .all(|(a, b)| (a - b).abs() < 1e-12)
                }));
            }
        }
    }
    let binary = (0..256).flat_map(|signs| {
        (0..3).map(move |mode| {
            std::array::from_fn(move |i| {
                let magnitude = match mode {
                    0 => 1,
                    1 => (i * 37 + signs * 7) % 255 + 1,
                    _ => 255,
                };
                if signs & (1 << i) == 0 {
                    -(magnitude as i16)
                } else {
                    magnitude as i16
                }
            })
        })
    });
    let ternary = (0..3usize.pow(8)).map(|mut code| {
        std::array::from_fn(|_| {
            let value = [-255, 0, 255][code % 3];
            code /= 3;
            value
        })
    });
    let random = (1..769u64).map(|mut state| {
        std::array::from_fn(|_| {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (state % 511) as i16 - 255
        })
    });
    for values in binary.chain(ternary).chain(random) {
        verify(
            &Trilinear::new(values)
                .unwrap()
                .critical_graph(|| false)
                .unwrap(),
        );
    }
}
