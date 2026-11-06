// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

fn limits(bits: u8) -> PlanarRefinementLimits {
    PlanarRefinementLimits {
        curves: RefinementLimits {
            precision_bits: bits,
            max_vertices: 4096,
            ..RefinementLimits::default()
        },
        max_vertices: 4096,
        ..PlanarRefinementLimits::default()
    }
}
fn validate(planar: &Arc<ProjectedPlanarGraph>, refined: &RefinedPlanarGraph) -> usize {
    assert!(Arc::ptr_eq(planar, refined.input()));
    let chart = planar.input().edges().chart().subdivision();
    let curves = refined.curves();
    for (id, n) in chart.nodes() {
        assert!(same2(
            *n.coordinates(),
            *refined.vertex(u32::from(id)).unwrap()
        ));
    }
    for (id, e) in planar.edges().iter().enumerate() {
        let path = refined.edge_vertices(id as u32).unwrap();
        assert!(path.len() >= 2);
        assert_eq!(path.first(), Some(&u32::from(e.nodes[0])));
        assert_eq!(path.last(), Some(&u32::from(e.nodes[1])));
        if let Some(chart_id) = e.chart_edge {
            assert_eq!(path, curves.edge_vertices(chart_id).unwrap());
        } else {
            let a = refined.vertex(path[0]).unwrap();
            let b = refined.vertex(*path.last().unwrap()).unwrap();
            let axis = if a[0].compare(b[0]).unwrap().is_eq() {
                1
            } else {
                0
            };
            for pair in path.windows(2) {
                let p = refined.vertex(pair[0]).unwrap();
                let q = refined.vertex(pair[1]).unwrap();
                assert_eq!(
                    p[axis].compare(q[axis]).unwrap(),
                    a[axis].compare(b[axis]).unwrap()
                );
                assert!(p[1 - axis].compare(q[1 - axis]).unwrap().is_eq());
            }
            for id in &path[1..path.len() - 1] {
                let p = refined.vertex(*id).unwrap();
                assert!(
                    curves
                        .slice_coordinates()
                        .iter()
                        .any(|u| u.compare(p[0]).unwrap().is_eq())
                );
                for field in chart.fields() {
                    assert_ne!(field.sign_at(*p).unwrap(), 0);
                }
            }
        }
    }
    for (id, r) in planar.regions().iter().enumerate() {
        let mut expected = Vec::new();
        let mut at = r.first;
        for _ in 0..r.edge_count {
            let h = planar.half_edges()[at as usize];
            let e = planar.edges()[h.edge as usize];
            let path = refined.edge_vertices(h.edge).unwrap();
            if h.from == e.nodes[0] {
                expected.extend_from_slice(&path[..path.len() - 1]);
            } else {
                expected.extend(path.iter().rev().take(path.len() - 1).copied());
            }
            at = h.next;
        }
        assert_eq!(at, r.first);
        let mut got = Vec::new();
        refined
            .region_boundary(
                id as u32,
                || false,
                |n| {
                    got.push(n);
                    Ok(())
                },
            )
            .unwrap();
        assert_eq!(got, expected);
    }
    assert!(matches!(
        refined.region_boundary(planar.regions().len() as u32, || false, |_| Ok(())),
        Err(SurfaceError::Geometry)
    ));
    assert!(matches!(
        refined.region_boundary(0, || true, |_| panic!("cancelled walk")),
        Err(SurfaceError::Cancelled)
    ));
    assert!(matches!(
        refined.region_boundary(0, || false, |_| Err(SurfaceError::Limit)),
        Err(SurfaceError::Limit)
    ));
    assert!(refined.vertex(refined.vertex_count() as u32).is_none());
    assert!(refined.edge_vertices(planar.edges().len() as u32).is_none());
    // Every horizontal source chord and curve chord spanning the same slice
    // stays on one side, unless they meet at a shared slice endpoint.
    for (id, e) in planar.edges().iter().enumerate() {
        if e.chart_edge.is_some() {
            continue;
        }
        let path = refined.edge_vertices(id as u32).unwrap();
        for pair in path.windows(2) {
            let [a, b] = [pair[0], pair[1]]
                .map(|v| refined.vertex(v).unwrap().map(|c| c.approximate().unwrap()));
            if (a[1] - b[1]).abs() > 1e-12 {
                continue;
            }
            for (j, other) in planar.edges().iter().enumerate() {
                if other.chart_edge.is_none() {
                    continue;
                }
                for edge in refined.edge_vertices(j as u32).unwrap().windows(2) {
                    let [c, d] = [edge[0], edge[1]]
                        .map(|v| refined.vertex(v).unwrap().map(|x| x.approximate().unwrap()));
                    if (a[0].min(b[0]) - c[0].min(d[0])).abs() < 1e-10
                        && (a[0].max(b[0]) - c[0].max(d[0])).abs() < 1e-10
                        && (a[0] - b[0]).abs() > 1e-12
                    {
                        assert!((c[1] - a[1]) * (d[1] - a[1]) >= -1e-10);
                    }
                }
            }
        }
    }
    refined.vertex_count() - curves.vertices().len()
}
#[test]
fn coordinated_planar_paths_preserve_regions_and_source_cuts() {
    let budget = MemoryBudget::new(128 * 1024 * 1024);
    let mut seed = 242u32;
    let mut added = 0usize;
    for case in 0..34 {
        let values = if case < 4 {
            [
                [26, -6, -6, -6, -6, -6, -6, 26],
                [2, -2, -2, -2, -2, -2, -2, 2],
                [0, 0, 2, 2, 2, 2, -4, -2],
                [0, 0, 2, 2, -2, -2, 0, 2],
            ][case]
        } else {
            std::array::from_fn(|_| {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                2 * ((seed >> 16) % 81) as i16 - 80
            })
        };
        let field = field(values);
        let graph = Arc::new(build(&field, [0; 3], budget.clone()));
        if graph.state() != &CellGraphState::Finite {
            continue;
        }
        for axis in 0..3 {
            let chart = Arc::new(
                graph
                    .projection_chart_with_budget(axis, budget.clone(), || false)
                    .unwrap(),
            );
            let edges = Arc::new(
                chart
                    .project_edges_with_budget(budget.clone(), || false)
                    .unwrap(),
            );
            let contacts = Arc::new(
                edges
                    .contacts_with_budget(budget.clone(), || false)
                    .unwrap(),
            );
            let split = Arc::new(
                contacts
                    .split_with_budget(budget.clone(), || false)
                    .unwrap(),
            );
            let planar = Arc::new(
                split
                    .planar_graph_with_budget(budget.clone(), || false)
                    .unwrap(),
            );
            let before = budget.used();
            let refined = planar
                .refine_with_budget(limits(7), budget.clone(), || false)
                .unwrap();
            assert_eq!(
                budget.used(),
                before + refined.referenced_bytes() + refined.curves().referenced_bytes()
            );
            added += validate(&planar, &refined);
        }
    }
    assert!(
        added > 0,
        "fixture must split a source cut at a refinement slice"
    );
    assert_eq!(budget.used(), 0);
}
#[test]
fn planar_refinement_limits_cancellation_and_pressure_preserve_input() {
    let budget = MemoryBudget::new(128 * 1024 * 1024);
    let split = input([26, -6, -6, -6, -6, -6, -6, 26], 0, budget.clone());
    let planar = Arc::new(
        split
            .planar_graph_with_budget(budget.clone(), || false)
            .unwrap(),
    );
    let before = budget.used();
    assert!(matches!(
        planar.refine_with_budget(
            PlanarRefinementLimits {
                max_vertices: 0,
                ..limits(6)
            },
            budget.clone(),
            || false
        ),
        Err(SurfaceError::Limit)
    ));
    assert!(matches!(
        planar.refine_with_budget(
            PlanarRefinementLimits {
                max_bytes: 0,
                ..limits(6)
            },
            budget.clone(),
            || false
        ),
        Err(SurfaceError::Limit)
    ));
    let tiny = MemoryBudget::new(1);
    assert!(matches!(
        planar.refine_with_budget(limits(6), tiny.clone(), || false),
        Err(SurfaceError::Memory)
    ));
    assert_eq!(tiny.used(), 0);
    let probe = MemoryBudget::new(128 * 1024 * 1024);
    let peak = Cell::new(0);
    let sampled = planar
        .refine_with_budget(limits(6), probe.clone(), || {
            peak.set(peak.get().max(probe.used()));
            false
        })
        .unwrap();
    drop(sampled);
    assert_eq!(probe.used(), 0);
    assert!(peak.get() > 1);
    probe.set_limit(peak.get() - 1);
    assert!(matches!(
        planar.refine_with_budget(limits(6), probe.clone(), || false),
        Err(SurfaceError::Memory)
    ));
    assert_eq!(probe.used(), 0);
    let calls = Cell::new(0);
    let output = planar
        .refine_with_budget(limits(6), budget.clone(), || {
            calls.set(calls.get() + 1);
            false
        })
        .unwrap();
    let total = calls.get();
    validate(&planar, &output);
    drop(output);
    for at in [1, 2, total / 3, 2 * total / 3, total] {
        let calls = Cell::new(0);
        assert!(matches!(
            planar.refine_with_budget(limits(6), budget.clone(), || {
                calls.set(calls.get() + 1);
                calls.get() == at
            }),
            Err(SurfaceError::Cancelled)
        ));
        assert_eq!(budget.used(), before);
    }
}
