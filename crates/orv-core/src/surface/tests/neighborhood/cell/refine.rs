// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

fn refine(graph: &Arc<SourceCellGraph>, bits: u8, budget: Arc<MemoryBudget>) -> RefinedCellGraph {
    graph
        .refine_edges_with_budget(
            CellPathLimits {
                precision_bits: bits,
                ..CellPathLimits::default()
            },
            budget,
            || false,
        )
        .unwrap()
}
fn verify_paths(paths: &RefinedCellGraph) {
    let graph = paths.graph();
    let values = graph.cell().interpolant().unwrap().values();
    for (id, node) in graph.nodes().iter().enumerate() {
        let vertex = &paths.vertices()[id];
        assert_eq!(vertex.origin(), CellPathOrigin::Node(id as u16));
        assert!(same(node.coordinates(), vertex.coordinates()));
        assert_eq!(
            node.coordinates()
                .map(|c| c.approximate().unwrap().to_bits()),
            vertex
                .coordinates()
                .map(|c| c.approximate().unwrap().to_bits())
        );
    }
    let mut visits = vec![0usize; paths.vertices().len()];
    for (id, edge) in graph.edges().iter().enumerate() {
        let path = paths.edge_vertices(id as u16).unwrap();
        assert!(path.len() >= 2);
        assert_eq!(path[0], u32::from(edge.nodes[0]));
        assert_eq!(*path.last().unwrap(), u32::from(edge.nodes[1]));
        for vertex in path {
            let p = paths.vertex(*vertex).unwrap();
            visits[*vertex as usize] += 1;
            assert!(
                tensor(
                    values,
                    p.coordinates().map(|c| c.approximate().unwrap()),
                    None
                )
                .abs()
                    < 1e-8
            );
        }
        for vertex in &path[1..path.len() - 1] {
            assert_eq!(
                paths.vertex(*vertex).unwrap().origin(),
                CellPathOrigin::Edge(id as u16)
            );
        }
        match edge.geometry {
            CellGraphGeometry::Axis { .. } => assert_eq!(path.len(), 2),
            CellGraphGeometry::Face { face } => {
                let axis = usize::from(face / 2);
                let [u, v] = transverse(axis);
                let field = graph
                    .cell()
                    .interpolant()
                    .unwrap()
                    .face(axis, face & 1 != 0)
                    .unwrap();
                for vertex in path {
                    let p = paths.vertex(*vertex).unwrap().coordinates();
                    assert_eq!(field.sign_at([p[u], p[v]]).unwrap(), 0);
                    let fixed = p[axis].rational_value().unwrap();
                    assert_eq!(
                        i128::from(fixed.numerator()),
                        i128::from(face & 1) * i128::from(fixed.denominator())
                    );
                }
            }
        }
    }
    assert!(visits[graph.nodes().len()..].iter().all(|n| *n == 1));
    assert!(paths.vertex(paths.vertices().len() as u32).is_none());
    assert!(paths.edge_vertices(graph.edges().len() as u16).is_none());
    for (id, region) in graph.regions().iter().enumerate() {
        let mut expected = Vec::new();
        let mut current = region.first;
        for _ in 0..region.edge_count {
            let half = &graph.half_edges()[usize::from(current)];
            let mut path = paths.edge_vertices(half.edge).unwrap().to_vec();
            if path[0] != u32::from(half.from) {
                path.reverse();
            }
            assert_eq!(*path.last().unwrap(), u32::from(half.to));
            expected.extend_from_slice(&path[..path.len() - 1]);
            current = half.next;
        }
        let mut actual = Vec::new();
        paths
            .region_boundary(
                id as u16,
                || false,
                |vertex| {
                    actual.push(vertex);
                    Ok(())
                },
            )
            .unwrap();
        assert_eq!(actual, expected);
        assert_eq!(current, region.first);
    }
    assert!(matches!(
        paths.region_boundary(graph.regions().len() as u16, || false, |_| Ok(())),
        Err(SurfaceError::Geometry)
    ));
    assert!(matches!(
        paths.region_boundary(0, || true, |_| panic!("cancelled walk emitted")),
        Err(SurfaceError::Cancelled)
    ));
    assert!(matches!(
        paths.region_boundary(0, || false, |_| Err(SurfaceError::Limit)),
        Err(SurfaceError::Limit)
    ));
}

#[test]
fn source_cell_paths_preserve_constraints_contacts_orientation_and_source_identity() {
    let budget = MemoryBudget::new(64 * 1024 * 1024);
    for values in [
        [2, -2, -2, -2, -2, -2, -2, 2],
        [6, -2, -2, -2, -2, -2, -2, 6],
        [26, -6, -6, -6, -6, -6, -6, 26],
        [2, -2, -2, 2, 6, 2, 2, 6],
        [0, 0, 2, 2, 2, 2, -4, -2],
        [0, 0, 2, 2, -2, -2, 0, 2],
    ] {
        let field = field(values);
        let graph = Arc::new(build(&field, [0; 3], budget.clone()));
        for bits in [0, 4, 8, 12] {
            let paths = refine(&graph, bits, budget.clone());
            assert!(Arc::ptr_eq(paths.graph(), &graph));
            assert_eq!(paths.graph().field().key(), field.key());
            assert_eq!(paths.graph().field().revision(), field.revision());
            assert!(Arc::ptr_eq(paths.graph().field().source(), field.source()));
            assert_eq!(paths.maximum_coordinate_error().denominator(), 1 << bits);
            assert_eq!(
                budget.used(),
                graph.referenced_bytes() + paths.referenced_bytes()
            );
            verify_paths(&paths);
        }
    }
    assert_eq!(budget.used(), 0);
}

fn distance_to_segment(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let d = [b[0] - a[0], b[1] - a[1]];
    let t =
        (((p[0] - a[0]) * d[0] + (p[1] - a[1]) * d[1]) / (d[0] * d[0] + d[1] * d[1])).clamp(0., 1.);
    ((p[0] - a[0] - t * d[0]).powi(2) + (p[1] - a[1] - t * d[1]).powi(2)).sqrt()
}
fn verify_distance(paths: &RefinedCellGraph) {
    let graph = paths.graph();
    let tolerance = 2f64.sqrt() / paths.maximum_coordinate_error().denominator() as f64 + 1e-10;
    for (id, edge) in graph.edges().iter().enumerate() {
        let CellGraphGeometry::Face { face } = edge.geometry else {
            continue;
        };
        let axis = usize::from(face / 2);
        let [u, v] = transverse(axis);
        let [a, b, c, d] = graph
            .cell()
            .interpolant()
            .unwrap()
            .face(axis, face & 1 != 0)
            .unwrap()
            .values()
            .map(f64::from);
        let coefficients = [a, b - a, d - a, a - b + c - d];
        let points: Vec<_> = paths
            .edge_vertices(id as u16)
            .unwrap()
            .iter()
            .map(|id| {
                let p = paths.vertex(*id).unwrap().coordinates();
                [p[u].approximate().unwrap(), p[v].approximate().unwrap()]
            })
            .collect();
        for i in 0..=256 {
            let t = i as f64 / 256.;
            let x = points[0][0] * (1. - t) + points.last().unwrap()[0] * t;
            let denominator = coefficients[2] + coefficients[3] * x;
            if denominator == 0. {
                continue;
            }
            let p = [x, -(coefficients[0] + coefficients[1] * x) / denominator];
            let distance = points
                .windows(2)
                .map(|s| distance_to_segment(p, s[0], s[1]))
                .fold(f64::INFINITY, f64::min);
            assert!(
                distance <= tolerance,
                "{distance} > {tolerance}, coefficients={coefficients:?}"
            );
        }
        // Check the other direction using both exact rational curve charts.
        for segment in points.windows(2) {
            for t in [0.25, 0.5, 0.75] {
                let p: [f64; 2] =
                    std::array::from_fn(|i| segment[0][i] * (1. - t) + segment[1][i] * t);
                let mut distance = f64::INFINITY;
                for chart in 0..2 {
                    let (b, c) = if chart == 0 {
                        (coefficients[1], coefficients[2])
                    } else {
                        (coefficients[2], coefficients[1])
                    };
                    let denominator = c + coefficients[3] * p[chart];
                    if denominator == 0. {
                        continue;
                    }
                    let q = -(coefficients[0] + b * p[chart]) / denominator;
                    let other = 1 - chart;
                    let lo = segment[0][other].min(segment[1][other]);
                    let hi = segment[0][other].max(segment[1][other]);
                    if q >= lo - 1e-12 && q <= hi + 1e-12 {
                        distance = distance.min((q - p[other]).abs());
                    }
                }
                assert!(
                    distance <= tolerance,
                    "chord-to-curve {distance} > {tolerance}"
                );
            }
        }
    }
}

#[test]
fn source_cell_paths_bound_both_curve_distances_with_independent_rational_samples() {
    let budget = MemoryBudget::new(64 * 1024 * 1024);
    let mut seed = 41u32;
    for case in 0..40 {
        let values = if case == 0 {
            [26, -6, -6, -6, -6, -6, -6, 26]
        } else {
            std::array::from_fn(|_| {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                2 * ((seed % 201) as i16 - 100)
            })
        };
        let f = field(values);
        let graph = Arc::new(build(&f, [0; 3], budget.clone()));
        if graph.state() != &CellGraphState::Finite {
            continue;
        }
        for bits in [0, 8, 12] {
            let paths = refine(&graph, bits, budget.clone());
            verify_paths(&paths);
            verify_distance(&paths);
        }
    }
    assert_eq!(budget.used(), 0);
}

#[test]
fn source_cell_paths_share_neighbor_face_vertices_and_floating_bits_in_both_directions() {
    let budget = MemoryBudget::new(64 * 1024 * 1024);
    let mut seed = 19u32;
    let mut matches = 0;
    for iteration in 0..8 {
        let values: [i16; 27] = std::array::from_fn(|_| {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            2 * ((seed % 41) as i16 - 20)
        });
        let f = fractional(source([3; 3], false, false), |p| {
            if iteration < 3 {
                eight_roots(iteration, p)
            } else {
                values[p[0] + 3 * p[1] + 9 * p[2]]
            }
        });
        for axis in 0..3 {
            for corner in 0..4 {
                let [u, v] = transverse(axis);
                let mut lower = [0; 3];
                lower[u] = corner & 1;
                lower[v] = corner >> 1;
                let left = Arc::new(build(&f, lower, budget.clone()));
                lower[axis] = 1;
                let right = Arc::new(build(&f, lower, budget.clone()));
                if left.state() != &CellGraphState::Finite
                    || right.state() != &CellGraphState::Finite
                {
                    continue;
                }
                for bits in [4, 10] {
                    let a = refine(&left, bits, budget.clone());
                    let b = refine(&right, bits, budget.clone());
                    let face_edges = |g: &SourceCellGraph, face: u8| {
                        g.edges()
                            .iter()
                            .enumerate()
                            .filter(move |(_, e)| e.faces & (1 << face) != 0)
                            .map(|(id, _)| id as u16)
                            .collect::<Vec<_>>()
                    };
                    let a_ids = face_edges(&left, (2 * axis + 1) as u8);
                    let b_ids = face_edges(&right, (2 * axis) as u8);
                    assert_eq!(a_ids.len(), b_ids.len());
                    let uv = |p: &RefinedCellGraph, id: u32| {
                        let c = p.vertex(id).unwrap().coordinates();
                        [c[u], c[v]]
                    };
                    for id in a_ids {
                        let ap = a.edge_vertices(id).unwrap();
                        let first = uv(&a, ap[0]);
                        let last = uv(&a, *ap.last().unwrap());
                        let eq = |a: [ExactCoordinate; 2], b: [ExactCoordinate; 2]| {
                            a.into_iter()
                                .zip(b)
                                .all(|(a, b)| a.compare(b).unwrap() == std::cmp::Ordering::Equal)
                        };
                        let mut found = None;
                        for id in &b_ids {
                            let bp = b.edge_vertices(*id).unwrap();
                            if eq(first, uv(&b, bp[0])) && eq(last, uv(&b, *bp.last().unwrap())) {
                                found = Some(bp.to_vec());
                                break;
                            }
                            if eq(first, uv(&b, *bp.last().unwrap())) && eq(last, uv(&b, bp[0])) {
                                found = Some(bp.iter().rev().copied().collect());
                                break;
                            }
                        }
                        let bp = found.expect("missing shared source-face path");
                        assert_eq!(ap.len(), bp.len());
                        for (ai, bi) in ap.iter().zip(bp) {
                            let ac = uv(&a, *ai);
                            let bc = uv(&b, bi);
                            assert!(eq(ac, bc));
                            assert_eq!(
                                ac.map(|c| c.approximate().unwrap().to_bits()),
                                bc.map(|c| c.approximate().unwrap().to_bits())
                            );
                        }
                        matches += 1;
                    }
                }
            }
        }
    }
    assert!(matches > 100);
    assert_eq!(budget.used(), 0);
}

#[test]
fn source_cell_paths_cancellation_exact_caps_and_late_admission_preserve_prior_owners() {
    let f = field([2, -2, -2, -2, -2, -2, -2, 2]);
    let budget = MemoryBudget::new(64 * 1024 * 1024);
    let graph = Arc::new(build(&f, [0; 3], budget.clone()));
    let graph_bytes = budget.used();
    let calls = Cell::new(0usize);
    let peak = Cell::new(0u64);
    let limits = CellPathLimits {
        precision_bits: 2,
        ..CellPathLimits::default()
    };
    let prior = graph
        .refine_edges_with_budget(limits, budget.clone(), || {
            calls.set(calls.get() + 1);
            peak.set(peak.get().max(budget.used()));
            false
        })
        .unwrap();
    let count = calls.get();
    let baseline = budget.used();
    let owners = Arc::strong_count(&graph);
    let original = prior
        .vertices()
        .iter()
        .map(|v| {
            (
                v.origin(),
                v.coordinates().map(|c| c.approximate().unwrap().to_bits()),
            )
        })
        .collect::<Vec<_>>();
    for stop in 1..=count {
        calls.set(0);
        assert!(
            matches!(
                graph.refine_edges_with_budget(limits, budget.clone(), || {
                    calls.set(calls.get() + 1);
                    calls.get() == stop
                }),
                Err(SurfaceError::Cancelled)
            ),
            "{stop}/{count}"
        );
        assert_eq!(budget.used(), baseline);
        assert_eq!(Arc::strong_count(&graph), owners);
    }
    let exact = CellPathLimits {
        max_vertices: prior.vertices().len(),
        max_bytes: prior.referenced_bytes(),
        ..limits
    };
    drop(
        graph
            .refine_edges_with_budget(exact, budget.clone(), || false)
            .unwrap(),
    );
    for limits in [
        CellPathLimits {
            max_vertices: exact.max_vertices - 1,
            ..exact
        },
        CellPathLimits {
            max_bytes: exact.max_bytes - 1,
            ..exact
        },
        CellPathLimits {
            precision_bits: 25,
            ..exact
        },
        CellPathLimits {
            max_vertices: RefinementLimits::MAX_VERTICES + 1,
            ..exact
        },
        CellPathLimits {
            max_bytes: RefinementLimits::MAX_BYTES + 1,
            ..exact
        },
    ] {
        assert!(matches!(
            graph.refine_edges_with_budget(limits, budget.clone(), || false),
            Err(SurfaceError::Limit)
        ));
        assert_eq!(budget.used(), baseline);
    }
    budget.set_limit(baseline + peak.get() - graph_bytes - 1);
    assert!(matches!(
        graph.refine_edges_with_budget(limits, budget.clone(), || false),
        Err(SurfaceError::Memory)
    ));
    assert_eq!(budget.used(), baseline);
    assert_eq!(Arc::strong_count(&graph), owners);
    budget.set_limit(baseline);
    assert!(matches!(
        graph.refine_edges_with_budget(limits, budget.clone(), || false),
        Err(SurfaceError::Memory)
    ));
    assert_eq!(budget.used(), baseline);
    assert_eq!(
        prior
            .vertices()
            .iter()
            .map(|v| (
                v.origin(),
                v.coordinates().map(|c| c.approximate().unwrap().to_bits())
            ))
            .collect::<Vec<_>>(),
        original
    );
    let MaskOwner::Supplied(map) = f.owner() else {
        panic!("supplied owner required")
    };
    let weak = Arc::downgrade(map);
    let source = Arc::downgrade(f.source());
    let revision = f.revision().clone();
    drop(f);
    drop(graph);
    assert!(weak.upgrade().is_some());
    assert_eq!(prior.graph().field().revision(), &revision);
    verify_paths(&prior);
    drop(prior);
    assert!(weak.upgrade().is_none());
    assert!(source.upgrade().is_none());
    assert_eq!(budget.used(), 0);
}

#[test]
fn source_cell_paths_refuse_unavailable_family_and_noninterface_inputs() {
    let budget = MemoryBudget::new(64 * 1024 * 1024);
    for values in [[0; 8], [-2; 8], [-2, 2, -2, 2, -2, 2, -2, 2]] {
        let graph = Arc::new(build(&field(values), [0; 3], budget.clone()));
        let used = budget.used();
        assert!(matches!(
            graph.refine_edges_with_budget(CellPathLimits::default(), budget.clone(), || false),
            Err(SurfaceError::Geometry)
        ));
        assert!(matches!(
            graph.refine_edges_with_budget(CellPathLimits::default(), budget.clone(), || true),
            Err(SurfaceError::Cancelled)
        ));
        assert_eq!(budget.used(), used);
    }
    for invalid in [false, true] {
        let s = source([2, 2, 3], false, invalid);
        let f = if invalid {
            fractional(s, |_| 0)
        } else {
            fractional_slices(s, &[0, 2], |_| 0)
        };
        let graph = Arc::new(build(&f, [0; 3], budget.clone()));
        let used = budget.used();
        assert!(matches!(
            graph.refine_edges_with_budget(CellPathLimits::default(), budget.clone(), || false),
            Err(SurfaceError::Geometry)
        ));
        assert_eq!(budget.used(), used);
    }
    assert_eq!(budget.used(), 0);
}

#[test]
fn source_cell_paths_support_maximum_precision_and_fractional_encoding() {
    let budget = MemoryBudget::new(64 * 1024 * 1024);
    let f = field([2, -2, -2, -2, -2, -2, -2, 2]);
    let graph = Arc::new(build(&f, [0; 3], budget.clone()));
    let coarse = refine(&graph, 8, budget.clone());
    let fine = refine(&graph, 24, budget.clone());
    assert!(fine.vertices().len() > coarse.vertices().len());
    assert_eq!(fine.maximum_coordinate_error().denominator(), 1 << 24);
    verify_paths(&fine);
    drop(coarse);
    drop(fine);
    drop(graph);
    assert_eq!(budget.used(), 0);
    let source = source([2; 3], false, false);
    let mut finite = 0;
    for code in 0..256 {
        let values: [i16; 8] =
            std::array::from_fn(|i| if code & (1 << i) == 0 { -255 } else { 255 });
        let masks = Arc::new(
            Segments::from_fractional(
                source.clone(),
                &[65535],
                &[(65535, 0), (65535, 1)],
                &values.map(|v| ((v + 255) / 2) as u8),
                FractionalEncoding {
                    kind: FractionalKind::Probability,
                    maximum: 255,
                },
                Some(true),
                || false,
            )
            .unwrap(),
        );
        let f = MaskField::new(MaskOwner::Supplied(masks), 65535).unwrap();
        let graph = Arc::new(build(&f, [0; 3], budget.clone()));
        if graph.state() == &CellGraphState::Finite {
            let paths = refine(&graph, 4, budget.clone());
            assert_eq!(paths.graph().cell().interpolant().unwrap().values(), values);
            verify_paths(&paths);
            verify_distance(&paths);
            finite += 1;
        }
    }
    assert!(finite > 100);
    assert_eq!(budget.used(), 0);
}
