// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
fn tensor(values: [i16; 8], p: [f64; 3], derivative: Option<usize>) -> f64 {
    values
        .into_iter()
        .enumerate()
        .map(|(i, v)| {
            f64::from(v)
                * (0..3)
                    .map(|a| {
                        if derivative == Some(a) {
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
fn field(values: [i16; 8]) -> MaskField {
    fractional(source([2; 3], true, false), |[x, y, z]| {
        values[x + 2 * y + 4 * z]
    })
}
fn build(field: &MaskField, lower: [usize; 3], budget: Arc<MemoryBudget>) -> SourceCellGraph {
    field
        .cell_graph_with_budget(lower, CellGraphLimits::default(), budget, || false)
        .unwrap()
}
fn same(a: &[ExactCoordinate; 3], b: &[ExactCoordinate; 3]) -> bool {
    a.iter()
        .zip(b)
        .all(|(a, b)| a.compare(*b).unwrap() == std::cmp::Ordering::Equal)
}
fn verify(graph: &SourceCellGraph) {
    let field = graph.cell().interpolant().unwrap();
    let values = field.values();
    assert_eq!(graph.phase(), Some(field.phase()));
    assert_eq!(graph.zero_boundary(), Some(field.zero_boundary()));
    assert_eq!(graph.factorization().unwrap().original(), field);
    if field.phase() != CellPhase::Crossing {
        assert_eq!(graph.state(), &CellGraphState::NoInteriorInterface);
    } else if graph.factorization().unwrap().residual().kind() != ResidualKind::Trilinear {
        assert!(matches!(graph.state(), CellGraphState::Family { .. }));
    } else {
        assert_eq!(graph.state(), &CellGraphState::Finite);
    }
    if graph.state() != &CellGraphState::Finite {
        assert!(
            graph.nodes().is_empty()
                && graph.edges().is_empty()
                && graph.half_edges().is_empty()
                && graph.regions().is_empty()
        );
        return;
    }
    assert!(!graph.regions().is_empty());
    for node in graph.nodes() {
        let p = node.coordinates().map(|c| c.approximate().unwrap());
        assert!(tensor(values, p, None).abs() < 1e-8);
        let mut boundary = 0;
        for (axis, c) in node.coordinates().iter().enumerate() {
            if let Some(r) = c.rational_value() {
                if r.numerator() == 0 {
                    boundary |= 1 << (2 * axis);
                }
                if i128::from(r.numerator()) == i128::from(r.denominator()) {
                    boundary |= 1 << (2 * axis + 1);
                }
            }
        }
        assert_eq!(boundary, node.boundary());
        assert_eq!(boundary, node.source_faces());
        for (axis, s) in node.gradient_signs().into_iter().enumerate() {
            let value = tensor(values, p, Some(axis));
            if s == 0 {
                assert!(value.abs() < 1e-8)
            } else if value.abs() > 1e-8 {
                assert_eq!(value.signum() as i8, s)
            }
        }
    }
    let critical = field.critical_graph(|| false).unwrap();
    for (id, node) in critical.nodes() {
        let matches: Vec<_> = graph
            .nodes()
            .iter()
            .filter(|n| n.original_node() == Some(id))
            .collect();
        assert_eq!(matches.len(), 1);
        assert!(same(matches[0].coordinates(), node.coordinates()));
        assert_eq!(matches[0].critical_axes(), node.critical_axes());
    }
    for axis in 0..3 {
        for upper in [false, true] {
            let (key, _) = graph.cell().face(axis, upper).unwrap();
            let face = graph.field().source_face(key, || false).unwrap();
            let contour = face.contour().unwrap();
            let [u, v] = transverse(axis);
            for (_, original) in contour.nodes() {
                let node = graph
                    .nodes()
                    .iter()
                    .find(|n| {
                        n.boundary() & (1 << (2 * axis + usize::from(upper))) != 0
                            && n.coordinates()[u]
                                .compare(original.coordinates()[0])
                                .unwrap()
                                == std::cmp::Ordering::Equal
                            && n.coordinates()[v]
                                .compare(original.coordinates()[1])
                                .unwrap()
                                == std::cmp::Ordering::Equal
                    })
                    .unwrap();
                assert_eq!(
                    [node.coordinates()[u], node.coordinates()[v]]
                        .map(|c| c.approximate().unwrap().to_bits()),
                    original
                        .coordinates()
                        .map(|c| c.approximate().unwrap().to_bits())
                );
            }
        }
    }
    for (id, line) in critical.lines() {
        let axis = line.axis();
        let [u, v] = transverse(axis);
        let fixed = line.fixed_coordinates();
        let mut nodes: Vec<_> = graph
            .nodes()
            .iter()
            .enumerate()
            .filter(|(_, n)| {
                n.coordinates()[u].compare(fixed[0]).unwrap() == std::cmp::Ordering::Equal
                    && n.coordinates()[v].compare(fixed[1]).unwrap() == std::cmp::Ordering::Equal
            })
            .map(|(i, _)| i as u16)
            .collect();
        nodes.sort_by(|a, b| {
            graph.nodes()[usize::from(*a)].coordinates()[axis]
                .compare(graph.nodes()[usize::from(*b)].coordinates()[axis])
                .unwrap()
        });
        for pair in nodes.windows(2) {
            assert_eq!(
                graph
                    .edges()
                    .iter()
                    .filter(|e| e.anchors & (1 << id) != 0
                        && e.nodes.contains(&pair[0])
                        && e.nodes.contains(&pair[1]))
                    .count(),
                1
            );
        }
        assert_eq!(
            graph
                .edges()
                .iter()
                .filter(|e| e.anchors & (1 << id) != 0)
                .count(),
            nodes.len() - 1
        );
    }
    let mut active = 0;
    for (id, edge) in graph.edges().iter().enumerate() {
        let [a, b] = edge.nodes.map(|i| &graph.nodes()[usize::from(i)]);
        assert_eq!(edge.faces, a.boundary() & b.boundary());
        let pa = a.coordinates().map(|c| c.approximate().unwrap());
        let pb = b.coordinates().map(|c| c.approximate().unwrap());
        for t in [0.25, 0.5, 0.75] {
            let mut p = std::array::from_fn(|i| pa[i] * (1. - t) + pb[i] * t);
            let tangent = match edge.geometry {
                CellGraphGeometry::Axis { axis } => {
                    assert!(edge.anchors != 0);
                    assert!(tensor(values, p, None).abs() < 1e-8);
                    let g: [f64; 3] = std::array::from_fn(|i| tensor(values, p, Some(i)));
                    assert!(g[usize::from(axis)].abs() < 1e-8);
                    std::array::from_fn(|i| pb[i] - pa[i])
                }
                CellGraphGeometry::Face { face } => {
                    assert_eq!(edge.anchors, 0);
                    let axis = usize::from(face / 2);
                    let [u, v] = transverse(axis);
                    let [a, b, c, d] = field
                        .face(axis, face & 1 != 0)
                        .unwrap()
                        .values()
                        .map(f64::from);
                    let [a, b, c, d] = [a, b - a, d - a, a - b + c - d];
                    p[v] = -(a + b * p[u]) / (c + d * p[u]);
                    assert!(tensor(values, p, None).abs() < 1e-8);
                    let mut tangent = [0.; 3];
                    tangent[u] = -tensor(values, p, Some(v));
                    tangent[v] = tensor(values, p, Some(u));
                    if dot(tangent, sub(pb, pa)) < 0. {
                        tangent = tangent.map(|x| -x);
                    }
                    tangent
                }
            };
            let normal = std::array::from_fn(|i| -tensor(values, p, Some(i)));
            let left = cross(normal, tangent);
            let mut allowed = [true; 2];
            for face in 0..6 {
                if edge.faces & (1 << face) != 0 {
                    let component = left[face / 2] * if face & 1 == 0 { -1. } else { 1. };
                    assert!(component.abs() > 1e-12);
                    allowed[0] &= component < 0.;
                    allowed[1] &= component > 0.;
                }
            }
            assert_eq!(edge.eligible, allowed);
        }
        if let Some(first) = edge.first_half {
            assert!(edge.eligible.iter().any(|v| *v));
            active += 2;
            for (direction, h) in graph.half_edges()[usize::from(first)..usize::from(first) + 2]
                .iter()
                .enumerate()
            {
                assert_eq!(usize::from(h.edge), id);
                assert_eq!(h.from, edge.nodes[direction]);
                assert_eq!(h.to, edge.nodes[1 - direction]);
                assert_eq!(h.eligible, edge.eligible[direction]);
            }
        } else {
            assert_eq!(edge.eligible, [false; 2]);
        }
    }
    assert_eq!(active, graph.half_edges().len());
    let mut incoming = vec![0usize; active];
    let mut seen = vec![false; active];
    for (id, h) in graph.half_edges().iter().enumerate() {
        let twin = &graph.half_edges()[usize::from(h.twin)];
        assert_eq!(usize::from(twin.twin), id);
        assert_eq!([twin.to, twin.from], [h.from, h.to]);
        let next = &graph.half_edges()[usize::from(h.next)];
        assert_eq!(next.from, h.to);
        assert_eq!(next.eligible, h.eligible);
        incoming[usize::from(h.next)] += 1;
        assert_eq!(h.eligible, h.region.is_some());
    }
    assert!(incoming.iter().all(|n| *n == 1));
    for (id, region) in graph.regions().iter().enumerate() {
        let mut current = region.first;
        for _ in 0..region.edge_count {
            assert!(!seen[usize::from(current)]);
            seen[usize::from(current)] = true;
            let h = &graph.half_edges()[usize::from(current)];
            assert_eq!(h.region, Some(id as u16));
            current = h.next;
        }
        assert_eq!(current, region.first);
    }
    assert_eq!(
        seen.iter().filter(|v| **v).count(),
        graph.half_edges().iter().filter(|h| h.eligible).count()
    );
}

#[test]
fn source_cell_graph_preserves_all_ternary_source_phases_constraints_and_cycles() {
    let source = source([2; 3], true, false);
    let budget = MemoryBudget::new(64 * 1024 * 1024);
    let mut finite = 0;
    for mut code in 0..6561 {
        let values: [i16; 8] = std::array::from_fn(|_| {
            let v = code % 3;
            code /= 3;
            2 * (v as i16 - 1)
        });
        let field = fractional(source.clone(), |[x, y, z]| values[x + 2 * y + 4 * z]);
        let graph = field
            .cell_graph_with_budget([0; 3], CellGraphLimits::default(), budget.clone(), || false)
            .unwrap_or_else(|e| panic!("{values:?}: {e:?}"));
        verify(&graph);
        finite += usize::from(graph.state() == &CellGraphState::Finite);
        assert_eq!(budget.used(), graph.referenced_bytes());
        drop(graph);
        assert_eq!(budget.used(), 0);
    }
    assert!(finite > 1000);
}

#[test]
fn source_cell_graph_handles_maximum_fractional_encoding_and_full_coefficient_range() {
    let source = source([2; 3], false, false);
    let budget = MemoryBudget::new(64 * 1024 * 1024);
    let mut seed = 151u32;
    let mut finite = 0;
    for case in 0..514 {
        let values: [i16; 8] = std::array::from_fn(|i| {
            if case < 256 {
                if case & (1 << i) == 0 { -255 } else { 255 }
            } else if case == 512 {
                if i == 0 || i == 7 { 255 } else { -85 }
            } else if case == 513 {
                if i == 0 || i == 7 { 255 } else { -255 }
            } else {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                2 * ((seed >> 16) % 256) as i16 - 255
            }
        });
        let payload = values.map(|v| ((v + 255) / 2) as u8);
        let masks = Arc::new(
            Segments::from_fractional(
                source.clone(),
                &[65535],
                &[(65535, 0), (65535, 1)],
                &payload,
                FractionalEncoding {
                    kind: FractionalKind::Probability,
                    maximum: 255,
                },
                Some(true),
                || false,
            )
            .unwrap(),
        );
        let field = MaskField::new(MaskOwner::Supplied(masks), 65535).unwrap();
        let graph = build(&field, [0; 3], budget.clone());
        verify(&graph);
        assert_eq!(graph.cell().interpolant().unwrap().values(), values);
        finite += usize::from(graph.state() == &CellGraphState::Finite);
        assert_eq!(budget.used(), graph.referenced_bytes());
        drop(graph);
        assert_eq!(budget.used(), 0);
    }
    assert!(finite > 200);
    let larger = super::source([3; 3], false, false);
    let mut payload: Vec<u8> = (0..27)
        .map(|_| {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            (seed >> 16) as u8
        })
        .collect();
    payload.push(0);
    let masks = Arc::new(
        Segments::from_fractional(
            larger,
            &[65535],
            &[(65535, 0), (65535, 1), (65535, 2)],
            &payload,
            FractionalEncoding {
                kind: FractionalKind::Probability,
                maximum: 255,
            },
            Some(true),
            || false,
        )
        .unwrap(),
    );
    let field = MaskField::new(MaskOwner::Supplied(masks), 65535).unwrap();
    for z in 0..2 {
        for y in 0..2 {
            for x in 0..2 {
                let graph = build(&field, [x, y, z], budget.clone());
                verify(&graph);
                assert_eq!(budget.used(), graph.referenced_bytes());
                drop(graph);
                assert_eq!(budget.used(), 0);
            }
        }
    }
}

#[test]
fn source_cell_graph_keeps_analytic_two_sheet_cone_tunnel_and_face_cross_topology() {
    let budget = MemoryBudget::new(64 * 1024 * 1024);
    for (values, regions) in [
        ([2, -2, -2, -2, -2, -2, -2, 2], 2),
        ([6, -2, -2, -2, -2, -2, -2, 6], 6),
        ([26, -6, -6, -6, -6, -6, -6, 26], 12),
        ([2, -2, -2, 2, 6, 2, 2, 6], 2),
    ] {
        let field = field(values);
        let graph = build(&field, [0; 3], budget.clone());
        verify(&graph);
        assert_eq!(graph.regions().len(), regions, "{values:?}");
        assert_eq!(graph.field().key(), field.key());
        assert_eq!(graph.field().revision(), field.revision());
        assert!(Arc::ptr_eq(graph.field().source(), field.source()));
    }
    assert_eq!(budget.used(), 0);
}

#[test]
fn source_cell_graph_extra_neighbor_constraints_share_original_face_bits_and_split_anchors() {
    let budget = MemoryBudget::new(64 * 1024 * 1024);
    let mut seed = 19u32;
    let mut finite = 0;
    let mut additional = 0;
    for iteration in 0..32 {
        let source = source([3, 3, 3], false, false);
        let mut values = [0i16; 27];
        for v in &mut values {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            *v = 2 * ((seed % 41) as i16 - 20);
        }
        let field = fractional(source, |p| {
            if iteration < 3 {
                eight_roots(iteration, p)
            } else {
                values[p[0] + 3 * p[1] + 9 * p[2]]
            }
        });
        for z in 0..2 {
            for y in 0..2 {
                for x in 0..2 {
                    let graph = build(&field, [x, y, z], budget.clone());
                    verify(&graph);
                    if graph.state() == &CellGraphState::Finite {
                        finite += 1;
                        additional += graph
                            .nodes()
                            .iter()
                            .filter(|n| n.original_node().is_none())
                            .count();
                    }
                }
            }
        }
        assert_eq!(budget.used(), 0);
    }
    assert!(finite > 100);
    assert!(additional > 100);
}

#[test]
fn source_cell_graph_unavailable_families_and_selected_strata_never_publish_a_false_empty_surface()
{
    let budget = MemoryBudget::new(64 * 1024 * 1024);
    for invalid in [false, true] {
        let source = source([2, 2, 3], false, invalid);
        let field = if invalid {
            fractional(source, |_| 0)
        } else {
            fractional_slices(source, &[0, 2], |_| 0)
        };
        let graph = build(&field, [0; 3], budget.clone());
        assert!(
            matches!(graph.state(),CellGraphState::Unavailable {invalid_corners,unknown_corners} if (*invalid_corners!=0)==invalid && (*unknown_corners!=0)!=invalid)
        );
        assert!(graph.nodes().is_empty());
        assert!(graph.phase().is_none());
    }
    for values in [
        [0; 8],
        [-2; 8],
        [-2, -2, -2, 0, -2, -2, -2, -2],
        [-2, 2, -2, 2, -2, 2, -2, 2],
    ] {
        let f = field(values);
        let graph = build(&f, [0; 3], budget.clone());
        verify(&graph);
        assert_ne!(graph.state(), &CellGraphState::Finite);
        assert_eq!(
            graph.zero_boundary(),
            Some(Trilinear::new(values).unwrap().zero_boundary())
        );
    }
    assert_eq!(budget.used(), 0);
}

#[test]
fn source_cell_graph_cancellation_limits_and_source_lifetime_preserve_the_prior_owner() {
    let field = field([26, -6, -6, -6, -6, -6, -6, 26]);
    let budget = MemoryBudget::new(64 * 1024 * 1024);
    let calls = Cell::new(0usize);
    let peak = Cell::new(0u64);
    let prior = field
        .cell_graph_with_budget([0; 3], CellGraphLimits::default(), budget.clone(), || {
            calls.set(calls.get() + 1);
            peak.set(peak.get().max(budget.used()));
            false
        })
        .unwrap();
    let count = calls.get();
    let baseline = budget.used();
    let edges = prior.edges().to_vec();
    let MaskOwner::Supplied(map) = field.owner() else {
        panic!("Supplied owner required")
    };
    let weak = Arc::downgrade(map);
    let weak_source = Arc::downgrade(field.source());
    let owners = Arc::strong_count(map);
    for stop in 1..=count {
        calls.set(0);
        assert!(
            matches!(
                field.cell_graph_with_budget(
                    [0; 3],
                    CellGraphLimits::default(),
                    budget.clone(),
                    || {
                        calls.set(calls.get() + 1);
                        calls.get() == stop
                    }
                ),
                Err(SurfaceError::Cancelled)
            ),
            "{stop}/{count}"
        );
        assert_eq!(budget.used(), baseline);
        assert_eq!(Arc::strong_count(map), owners);
        assert_eq!(prior.edges(), edges);
    }
    let exact = CellGraphLimits {
        max_nodes: prior.nodes().len(),
        max_edges: prior.edges().len(),
        max_bytes: prior.referenced_bytes(),
    };
    let other = field
        .cell_graph_with_budget([0; 3], exact, budget.clone(), || false)
        .unwrap();
    assert_eq!(other.edges(), edges);
    drop(other);
    for limits in [
        CellGraphLimits {
            max_nodes: exact.max_nodes - 1,
            ..exact
        },
        CellGraphLimits {
            max_edges: exact.max_edges - 1,
            ..exact
        },
        CellGraphLimits {
            max_bytes: exact.max_bytes - 1,
            ..exact
        },
    ] {
        assert!(matches!(
            field.cell_graph_with_budget([0; 3], limits, budget.clone(), || false),
            Err(SurfaceError::Limit)
        ));
        assert_eq!(budget.used(), baseline);
    }
    // The final observed peak includes both scratch and completed output. One
    // byte less must refuse that late admission and retain the prior graph.
    budget.set_limit(baseline + peak.get() - 1);
    assert!(matches!(
        field.cell_graph_with_budget([0; 3], CellGraphLimits::default(), budget.clone(), || false),
        Err(SurfaceError::Memory)
    ));
    assert_eq!(budget.used(), baseline);
    budget.set_limit(baseline);
    assert!(matches!(
        field.cell_graph_with_budget([0; 3], exact, budget.clone(), || false),
        Err(SurfaceError::Memory)
    ));
    assert_eq!(budget.used(), baseline);
    let revision = field.revision().clone();
    drop(field);
    assert!(weak.upgrade().is_some());
    assert_eq!(prior.field().revision(), &revision);
    verify(&prior);
    drop(prior);
    assert!(weak.upgrade().is_none());
    assert!(weak_source.upgrade().is_none());
    assert_eq!(budget.used(), 0);
}

#[test]
fn source_cell_graph_boundary_zero_edges_distinguish_strata_from_entering_surfaces() {
    let budget = MemoryBudget::new(64 * 1024 * 1024);
    for (values, entering) in [
        ([0, 0, 2, 2, 2, 2, -4, -2], false),
        ([0, 0, 2, 2, -2, -2, 0, 2], true),
    ] {
        let field = field(values);
        let graph = build(&field, [0; 3], budget.clone());
        verify(&graph);
        assert_eq!(graph.regions().len(), 1);
        let edge = graph
            .edges()
            .iter()
            .find(|e| {
                e.geometry == CellGraphGeometry::Axis { axis: 0 } && e.faces == (1 << 2) | (1 << 4)
            })
            .unwrap();
        assert_eq!(
            edge.eligible.iter().filter(|v| **v).count(),
            usize::from(entering)
        );
        assert_eq!(edge.first_half.is_some(), entering);
        if !entering {
            assert!(
                edge.nodes
                    .into_iter()
                    .all(|n| graph.nodes()[usize::from(n)].surface_degree() == 0)
            );
        }
        let MaskOwner::Supplied(map) = field.owner() else {
            panic!("Supplied owner required")
        };
        assert_eq!(map.mask(65535).unwrap().raw_value([0, 0, 0]), Some(100));
        assert_eq!(map.mask(65535).unwrap().value([0, 0, 0]), Some(true));
    }
    assert_eq!(budget.used(), 0);
}

fn flip(c: ExactCoordinate) -> ExactCoordinate {
    let BilinearSlice::Point(p) = Bilinear::new([1, 0, -1, 0])
        .unwrap()
        .zero_slice(0, c)
        .unwrap()
    else {
        panic!("Finite reflected point required")
    };
    p
}
fn geometry_code(g: CellGraphGeometry) -> u8 {
    match g {
        CellGraphGeometry::Axis { axis } => axis,
        CellGraphGeometry::Face { face } => 3 + face,
    }
}
fn region_signatures(
    graph: &SourceCellGraph,
    node_map: &[u16],
    geometry_map: impl Fn(CellGraphGeometry) -> CellGraphGeometry,
    reverse: bool,
) -> Vec<Vec<(u16, u16, u8)>> {
    let mut regions = Vec::new();
    for region in graph.regions() {
        let mut current = region.first;
        let mut cycle = Vec::new();
        for _ in 0..region.edge_count {
            let h = &graph.half_edges()[usize::from(current)];
            let e = &graph.edges()[usize::from(h.edge)];
            let [a, b] = if reverse {
                [h.to, h.from]
            } else {
                [h.from, h.to]
            };
            cycle.push((
                node_map[usize::from(a)],
                node_map[usize::from(b)],
                geometry_code(geometry_map(e.geometry)),
            ));
            current = h.next;
        }
        if reverse {
            cycle.reverse();
        }
        let first = cycle.iter().enumerate().min_by_key(|(_, v)| **v).unwrap().0;
        cycle.rotate_left(first);
        regions.push(cycle);
    }
    regions.sort();
    regions
}
#[test]
fn source_cell_graph_cycles_and_side_eligibility_transform_under_axes_and_complement() {
    let source = source([2; 3], false, false);
    let budget = MemoryBudget::new(64 * 1024 * 1024);
    for values in [
        [2, -2, -2, -2, -2, -2, -2, 2],
        [6, -2, -2, -2, -2, -2, -2, 6],
        [26, -6, -6, -6, -6, -6, -6, 26],
        [2, -2, -2, 2, 6, 2, 2, 6],
        [0, 0, 2, 2, 2, 2, -4, -2],
        [0, 0, 2, 2, -2, -2, 0, 2],
    ] {
        let base = fractional(source.clone(), |[x, y, z]| values[x + 2 * y + 4 * z]);
        let original = build(&base, [0; 3], budget.clone());
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
                    let mut transformed = [0; 8];
                    for (i, v) in values.into_iter().enumerate() {
                        let index = (0..3)
                            .map(|a| {
                                (((i >> perm[a]) & 1) ^ usize::from(flips & (1 << a) != 0)) << a
                            })
                            .sum::<usize>();
                        transformed[index] = if complement { -v } else { v };
                    }
                    let field =
                        fractional(source.clone(), |[x, y, z]| transformed[x + 2 * y + 4 * z]);
                    let graph = build(&field, [0; 3], budget.clone());
                    verify(&graph);
                    assert_ne!(graph.field().key(), original.field().key());
                    assert_eq!(graph.nodes().len(), original.nodes().len());
                    assert_eq!(graph.edges().len(), original.edges().len());
                    let node_map: Vec<_> = original
                        .nodes()
                        .iter()
                        .map(|node| {
                            let p = std::array::from_fn(|a| {
                                let c = node.coordinates()[perm[a]];
                                if flips & (1 << a) == 0 { c } else { flip(c) }
                            });
                            graph
                                .nodes()
                                .iter()
                                .position(|n| same(n.coordinates(), &p))
                                .unwrap() as u16
                        })
                        .collect();
                    let face_map = |face: u8| {
                        let axis = perm
                            .iter()
                            .position(|a| *a == usize::from(face / 2))
                            .unwrap();
                        (2 * axis + usize::from((face & 1 != 0) != (flips & (1 << axis) != 0)))
                            as u8
                    };
                    let geometry_map = |g| match g {
                        CellGraphGeometry::Axis { axis } => CellGraphGeometry::Axis {
                            axis: perm.iter().position(|a| *a == usize::from(axis)).unwrap() as u8,
                        },
                        CellGraphGeometry::Face { face } => CellGraphGeometry::Face {
                            face: face_map(face),
                        },
                    };
                    let reverse =
                        !(inversions + flips.count_ones() as usize + usize::from(complement))
                            .is_multiple_of(2);
                    for e in original.edges() {
                        let nodes = e.nodes.map(|n| node_map[usize::from(n)]);
                        let mapped = graph
                            .edges()
                            .iter()
                            .find(|other| {
                                other.nodes == [nodes[0].min(nodes[1]), nodes[0].max(nodes[1])]
                                    && other.geometry == geometry_map(e.geometry)
                            })
                            .unwrap();
                        let mut faces = 0;
                        for f in 0..6 {
                            if e.faces & (1 << f) != 0 {
                                faces |= 1 << face_map(f);
                            }
                        }
                        assert_eq!(mapped.faces, faces);
                        let forward = usize::from(nodes[0] != mapped.nodes[0]);
                        assert_eq!(mapped.eligible[forward], e.eligible[usize::from(reverse)]);
                        assert_eq!(
                            mapped.eligible[1 - forward],
                            e.eligible[usize::from(!reverse)]
                        );
                    }
                    let identity: Vec<_> = (0..graph.nodes().len()).map(|i| i as u16).collect();
                    assert_eq!(
                        region_signatures(&original, &node_map, geometry_map, reverse),
                        region_signatures(&graph, &identity, |g| g, false)
                    );
                }
            }
        }
    }
    assert_eq!(budget.used(), 0);
}
