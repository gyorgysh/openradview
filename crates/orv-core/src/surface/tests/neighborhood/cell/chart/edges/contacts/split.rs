// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use std::collections::BTreeSet;

fn same2(a: [ExactCoordinate; 2], b: [ExactCoordinate; 2]) -> bool {
    a.into_iter()
        .zip(b)
        .all(|(a, b)| a.compare(b).unwrap().is_eq())
}
fn point(chart: &CellProjectionChart, node: u8) -> [ExactCoordinate; 2] {
    *chart.subdivision().node(node).unwrap().coordinates()
}
fn validate(contacts: &Arc<ProjectedContacts>, split: &SplitProjectedConstraints) -> [usize; 4] {
    assert!(Arc::ptr_eq(split.contacts(), contacts));
    let edges = split.edges();
    let chart = edges.chart();
    let graph = chart.graph();
    assert!(Arc::ptr_eq(graph, contacts.edges().chart().graph()));
    let mut seen = [0; 4];
    for (_, original) in contacts.edges().chart().subdivision().nodes() {
        assert!(
            chart
                .subdivision()
                .nodes()
                .any(|(_, new)| same2(*original.coordinates(), *new.coordinates()))
        );
    }
    for contact in contacts.contacts() {
        let points: &[[ExactCoordinate; 2]] = match &contact.geometry {
            ProjectedContactGeometry::Point(p) => std::slice::from_ref(p),
            ProjectedContactGeometry::Overlap(p) => p,
        };
        for p in points {
            assert!(
                chart
                    .subdivision()
                    .nodes()
                    .any(|(_, n)| same2(*p, *n.coordinates()))
            );
        }
    }
    let mut unique = BTreeSet::new();
    for (id, segment) in split.segments().iter().enumerate() {
        assert!(segment.nodes[0] < segment.nodes[1]);
        assert!(unique.insert(segment.nodes));
        let [a, b] = segment.nodes.map(|n| point(chart, n));
        assert!(!same2(a, b));
        assert!(a[0].compare(b[0]).unwrap().is_eq() || a[1].compare(b[1]).unwrap().is_eq());
        if let Some(chart_id) = segment.chart_edge {
            seen[3] += 1;
            let e = chart.subdivision().edge(chart_id).unwrap();
            assert!(matches!(e.geometry, CommonZero::Axis { .. }));
            assert_eq!([e.from.min(e.to), e.from.max(e.to)], segment.nodes);
        }
        for other in &split.segments()[id + 1..] {
            let [c, d] = other
                .nodes
                .map(|n| point(chart, n))
                .map(|p| p.map(|n| n.approximate().unwrap()));
            let [a, b] = [a, b].map(|p| p.map(|n| n.approximate().unwrap()));
            let overlaps = (0..2).all(|k| {
                a[k].min(b[k]).max(c[k].min(d[k])) <= a[k].max(b[k]).min(c[k].max(d[k])) + 1e-12
            });
            if overlaps {
                assert!(segment.nodes.iter().any(|n| other.nodes.contains(n)));
            }
        }
    }
    let [u, v] = chart.transverse_axes();
    for (id, original) in graph.edges().iter().enumerate() {
        let source = original.nodes.map(|n| {
            let p = graph.nodes()[usize::from(n)].coordinates();
            [p[u], p[v]]
        });
        match split.path(id as u16).unwrap() {
            SplitPath::Collapsed { node } => {
                seen[0] += 1;
                assert!(same2(source[0], source[1]));
                assert!(same2(source[0], point(chart, node)));
                assert!(split.steps(id as u16).is_none());
            }
            SplitPath::EndFace => {
                seen[1] += 1;
                assert!(!edges.chart_steps(id as u16).unwrap().is_empty());
                assert!(split.steps(id as u16).is_none());
            }
            SplitPath::Segment { len, .. } => {
                seen[2] += 1;
                let steps = split.steps(id as u16).unwrap();
                assert_eq!(steps.len(), len as usize);
                assert!(!steps.is_empty());
                let mut cursor = source[0];
                for step in steps {
                    let segment = split.segments()[step.segment as usize];
                    let nodes = if step.forward {
                        segment.nodes
                    } else {
                        [segment.nodes[1], segment.nodes[0]]
                    };
                    assert!(same2(cursor, point(chart, nodes[0])));
                    cursor = point(chart, nodes[1]);
                }
                assert!(same2(cursor, source[1]));
            }
        }
    }
    assert!(split.path(graph.edges().len() as u16).is_none());
    seen
}
#[test]
fn split_contacts_produce_source_bound_non_crossing_segment_paths() {
    let budget = MemoryBudget::new(128 * 1024 * 1024);
    let mut seen = [0; 4];
    let mut seed = 771u32;
    for case in 0..54 {
        let values = if case < 4 {
            [
                [2, -2, -2, -2, -2, -2, -2, 2],
                [6, -2, -2, -2, -2, -2, -2, 6],
                [0, 0, 2, 2, 2, 2, -4, -2],
                [0, 0, 2, 2, -2, -2, 0, 2],
            ][case]
        } else {
            std::array::from_fn(|_| {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                2 * ((seed >> 16) % 81) as i16 - 80
            })
        };
        let graph = Arc::new(build(&field(values), [0; 3], budget.clone()));
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
            let split = contacts
                .split_with_budget(budget.clone(), || false)
                .unwrap();
            for (a, b) in seen.iter_mut().zip(validate(&contacts, &split)) {
                *a += b;
            }
        }
    }
    assert!(seen.into_iter().all(|v| v > 0), "{seen:?}");
    assert_eq!(budget.used(), 0);
}
#[test]
fn split_refuses_memory_and_cancellation_without_losing_inputs() {
    let budget = MemoryBudget::new(128 * 1024 * 1024);
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
    let baseline = budget.used();
    let tiny = MemoryBudget::new(1);
    assert!(matches!(
        contacts.split_with_budget(tiny.clone(), || false),
        Err(SurfaceError::Memory)
    ));
    assert_eq!(tiny.used(), 0);
    // Observe a successful run's simultaneous reservations, then refuse one
    // byte below that peak after earlier allocations have already succeeded.
    let probe = MemoryBudget::new(128 * 1024 * 1024);
    let peak = Cell::new(0);
    let sampled = contacts
        .split_with_budget(probe.clone(), || {
            peak.set(peak.get().max(probe.used()));
            false
        })
        .unwrap();
    drop(sampled);
    assert_eq!(probe.used(), 0);
    assert!(peak.get() > 1);
    probe.set_limit(peak.get() - 1);
    assert!(matches!(
        contacts.split_with_budget(probe.clone(), || false),
        Err(SurfaceError::Memory)
    ));
    assert_eq!(probe.used(), 0);
    let calls = Cell::new(0);
    let result = contacts
        .split_with_budget(budget.clone(), || {
            calls.set(calls.get() + 1);
            false
        })
        .unwrap();
    let total = calls.get();
    validate(&contacts, &result);
    drop(result);
    assert_eq!(budget.used(), baseline);
    for at in [1, 2, total / 3, 2 * total / 3, total] {
        let calls = Cell::new(0);
        assert!(matches!(
            contacts.split_with_budget(budget.clone(), || {
                calls.set(calls.get() + 1);
                calls.get() == at
            }),
            Err(SurfaceError::Cancelled)
        ));
        assert_eq!(budget.used(), baseline);
    }
}

#[test]
fn split_keeps_an_existing_refined_chart_constraint() {
    let budget = MemoryBudget::new(128 * 1024 * 1024);
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
    let (face, path) = graph
        .edges()
        .iter()
        .enumerate()
        .find_map(|(id, edge)| {
            let CellGraphGeometry::Face { face } = edge.geometry else {
                return None;
            };
            let path = paths.edge_vertices(id as u16)?;
            (path.len() > 2).then_some((face, path))
        })
        .unwrap();
    let axis = usize::from(face / 2);
    let [u, v] = transverse(axis);
    let p = paths.vertex(path[path.len() / 2]).unwrap().coordinates();
    let wanted = [p[u], p[v]];
    let chart = Arc::new(
        graph
            .projection_chart_with_insertions_and_budget(axis, &[wanted], budget.clone(), || false)
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
    let split = contacts
        .split_with_budget(budget.clone(), || false)
        .unwrap();
    assert!(
        split
            .edges()
            .chart()
            .subdivision()
            .nodes()
            .any(|(_, n)| same2(*n.coordinates(), wanted))
    );
    validate(&contacts, &split);
}
