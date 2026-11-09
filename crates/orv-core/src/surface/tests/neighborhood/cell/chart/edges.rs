// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
mod contacts;

fn equal(a: [ExactCoordinate; 2], b: [ExactCoordinate; 2]) -> bool {
    a.into_iter()
        .zip(b)
        .all(|(a, b)| a.compare(b).unwrap().is_eq())
}
fn verify(chart: &Arc<CellProjectionChart>, budget: Arc<MemoryBudget>) -> [usize; 3] {
    let graph = chart.graph();
    let projected = chart
        .project_edges_with_budget(budget.clone(), || false)
        .unwrap();
    assert!(Arc::ptr_eq(projected.chart(), chart));
    assert_eq!(projected.edges().len(), graph.edges().len());
    assert_eq!(
        budget.used(),
        graph.referenced_bytes() + chart.referenced_bytes() + projected.referenced_bytes()
    );
    let [u, v] = chart.transverse_axes();
    let mut kinds = [0; 3];
    for (id, edge) in graph.edges().iter().enumerate() {
        let entry = &projected.edges()[id];
        assert_eq!(entry.source_nodes, edge.nodes);
        let points = edge.nodes.map(|n| {
            let p = graph.nodes()[usize::from(n)].coordinates();
            [p[u], p[v]]
        });
        let steps = projected.chart_steps(id as u16).unwrap();
        match entry.kind {
            ProjectedEdgeKind::Collapsed => {
                kinds[0] += 1;
                assert!(equal(points[0], points[1]));
                assert!(steps.is_empty());
                assert!(
                    matches!(edge.geometry, CellGraphGeometry::Axis { axis } if axis == chart.axis())
                );
                let a = graph.nodes()[usize::from(edge.nodes[0])].coordinates()
                    [usize::from(chart.axis())];
                let b = graph.nodes()[usize::from(edge.nodes[1])].coordinates()
                    [usize::from(chart.axis())];
                assert_ne!(a.compare(b).unwrap(), std::cmp::Ordering::Equal);
            }
            ProjectedEdgeKind::Segment => {
                kinds[1] += 1;
                assert!(!equal(points[0], points[1]));
                assert!(
                    points[0][0].compare(points[1][0]).unwrap().is_eq()
                        || points[0][1].compare(points[1][1]).unwrap().is_eq()
                );
                assert!(steps.is_empty());
            }
            ProjectedEdgeKind::EndFace { upper } => {
                kinds[2] += 1;
                assert!(!steps.is_empty());
                assert_ne!(edge.faces & (1 << (2 * chart.axis() + u8::from(upper))), 0);
                let mut last = points[0];
                for step in steps {
                    let e = chart.subdivision().edge(step.edge).unwrap();
                    assert_ne!(e.curves & (1 << usize::from(upper)), 0);
                    let ids = if step.forward {
                        [e.from, e.to]
                    } else {
                        [e.to, e.from]
                    };
                    let a = *chart.subdivision().node(ids[0]).unwrap().coordinates();
                    let b = *chart.subdivision().node(ids[1]).unwrap().coordinates();
                    assert!(equal(last, a));
                    assert!(!equal(a, b));
                    last = b;
                }
                assert!(equal(last, points[1]));
            }
        }
    }
    assert!(projected.chart_steps(graph.edges().len() as u16).is_none());
    kinds
}

#[test]
fn projected_edges_preserve_full_source_and_end_face_chains() {
    let budget = MemoryBudget::new(64 * 1024 * 1024);
    let mut total = [0; 3];
    let mut seed = 8765u32;
    for case in 0..110 {
        let values = if case < 6 {
            [
                [2, -2, -2, -2, -2, -2, -2, 2],
                [6, -2, -2, -2, -2, -2, -2, 6],
                [26, -6, -6, -6, -6, -6, -6, 26],
                [2, -2, -2, 2, 6, 2, 2, 6],
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
            for (a, b) in total.iter_mut().zip(verify(&chart, budget.clone())) {
                *a += b;
            }
        }
    }
    assert!(total.into_iter().all(|n| n > 0), "{total:?}");
    assert_eq!(budget.used(), 0);
}

#[test]
fn projected_edges_cancel_and_refuse_memory_without_leaks() {
    let budget = MemoryBudget::new(64 * 1024 * 1024);
    let graph = Arc::new(build(
        &field([6, -2, -2, -2, -2, -2, -2, 6]),
        [0; 3],
        budget.clone(),
    ));
    let chart = Arc::new(
        graph
            .projection_chart_with_budget(0, budget.clone(), || false)
            .unwrap(),
    );
    let baseline = budget.used();
    let tiny = MemoryBudget::new(1);
    assert!(matches!(
        chart.project_edges_with_budget(tiny.clone(), || false),
        Err(SurfaceError::Memory)
    ));
    assert_eq!(tiny.used(), 0);
    let calls = Cell::new(0);
    let output = chart
        .project_edges_with_budget(budget.clone(), || {
            calls.set(calls.get() + 1);
            false
        })
        .unwrap();
    let total = calls.get();
    let insufficient = MemoryBudget::new(output.referenced_bytes() - 1);
    assert!(matches!(
        chart.project_edges_with_budget(insufficient.clone(), || false),
        Err(SurfaceError::Memory)
    ));
    assert_eq!(insufficient.used(), 0);
    drop(output);
    for at in [1, 2, total / 2, total] {
        let calls = Cell::new(0);
        assert!(matches!(
            chart.project_edges_with_budget(budget.clone(), || {
                calls.set(calls.get() + 1);
                calls.get() == at
            }),
            Err(SurfaceError::Cancelled)
        ));
        assert_eq!(budget.used(), baseline);
    }
}

#[test]
fn additional_refined_face_constraint_splits_the_directed_chart_link() {
    let budget = MemoryBudget::new(64 * 1024 * 1024);
    let graph = Arc::new(build(
        &field([26, -6, -6, -6, -6, -6, -6, 26]),
        [0; 3],
        budget.clone(),
    ));
    let paths = graph
        .refine_edges_with_budget(
            CellPathLimits {
                precision_bits: 8,
                ..CellPathLimits::default()
            },
            budget.clone(),
            || false,
        )
        .unwrap();
    let (id, face, path) = graph
        .edges()
        .iter()
        .enumerate()
        .find_map(|(id, e)| {
            let CellGraphGeometry::Face { face } = e.geometry else {
                return None;
            };
            let path = paths.edge_vertices(id as u16)?;
            (path.len() > 2).then_some((id, face, path))
        })
        .expect("fixture needs a refined face arc");
    let axis = usize::from(face / 2);
    let [u, v] = transverse(axis);
    let vertex = paths.vertex(path[path.len() / 2]).unwrap().coordinates();
    let point = [vertex[u], vertex[v]];
    let chart = Arc::new(
        graph
            .projection_chart_with_insertions_and_budget(axis, &[point], budget.clone(), || false)
            .unwrap(),
    );
    let projected = chart
        .project_edges_with_budget(budget.clone(), || false)
        .unwrap();
    let steps = projected.chart_steps(id as u16).unwrap();
    assert!(steps.len() >= 2);
    assert!(steps.windows(2).any(|pair| {
        let first = chart.subdivision().edge(pair[0].edge).unwrap();
        let last = if pair[0].forward {
            first.to
        } else {
            first.from
        };
        equal(
            *chart.subdivision().node(last).unwrap().coordinates(),
            point,
        )
    }));
    assert!(matches!(
        graph.projection_chart_with_insertions_and_budget(
            axis,
            &[point; JointSubdivision::MAX_INSERTIONS + 1],
            budget.clone(),
            || false,
        ),
        Err(SurfaceError::Limit)
    ));
}
