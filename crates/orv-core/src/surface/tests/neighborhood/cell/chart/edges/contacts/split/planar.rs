// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use std::collections::BTreeSet;

fn verify(split: &Arc<SplitProjectedConstraints>, output: &ProjectedPlanarGraph) -> bool {
    assert!(Arc::ptr_eq(split, output.input()));
    let chart = split.edges().chart().subdivision();
    let edges = output.edges();
    let halves = output.half_edges();
    assert_eq!(halves.len(), 2 * edges.len());
    let mut nodes = BTreeSet::new();
    let mut chart_seen = BTreeSet::new();
    let mut segment_seen = BTreeSet::new();
    for edge in edges {
        nodes.extend(edge.nodes);
        if let Some(id) = edge.chart_edge {
            assert!(chart_seen.insert(id));
            let original = chart.edge(id).unwrap();
            assert_eq!(edge.nodes, [original.from, original.to]);
        }
        if let Some(id) = edge.segment {
            assert!(segment_seen.insert(id));
            assert_eq!(edge.nodes, split.segments()[id as usize].nodes);
        }
        assert!(edge.chart_edge.is_some() || edge.segment.is_some());
    }
    assert_eq!(chart_seen.len(), chart.edges().count());
    assert_eq!(segment_seen.len(), split.segments().len());
    assert_eq!(nodes.len() + output.regions().len() + 1, edges.len() + 2);
    let mut visited = vec![false; halves.len()];
    for (id, region) in output.regions().iter().enumerate() {
        let parent = chart
            .regions()
            .find(|(n, _)| *n == region.chart_region)
            .unwrap()
            .1;
        assert_eq!(region.signs, parent.signs);
        let mut at = region.first as usize;
        for _ in 0..region.edge_count {
            let h = halves[at];
            assert!(!visited[at]);
            visited[at] = true;
            assert_eq!(h.region, Some(id as u32));
            assert_eq!(h.chart_region, Some(region.chart_region));
            let twin = halves[h.twin as usize];
            assert_eq!(twin.twin, at as u32);
            assert_eq!([twin.from, twin.to], [h.to, h.from]);
            assert_eq!(halves[h.next as usize].from, h.to);
            at = h.next as usize;
        }
        assert_eq!(at, region.first as usize);
    }
    let outside = halves
        .iter()
        .enumerate()
        .find(|(i, _)| !visited[*i])
        .unwrap()
        .0;
    let mut at = outside;
    loop {
        assert!(!visited[at]);
        visited[at] = true;
        let h = halves[at];
        assert_eq!(h.region, None);
        assert_eq!(h.chart_region, None);
        at = h.next as usize;
        if at == outside {
            break;
        }
    }
    assert!(visited.into_iter().all(|v| v));
    // For non-chart straight edges, an interior midpoint has the parent
    // field's sign independently of the rotation-system implementation.
    for (id, e) in edges.iter().enumerate() {
        if e.chart_edge.is_some() {
            continue;
        }
        let p = e.nodes.map(|n| {
            chart
                .node(n)
                .unwrap()
                .coordinates()
                .map(|c| c.approximate().unwrap())
        });
        let [u, v] = [(p[0][0] + p[1][0]) / 2., (p[0][1] + p[1][1]) / 2.];
        for half in [halves[2 * id], halves[2 * id + 1]] {
            let parent = chart
                .regions()
                .find(|(id, _)| Some(*id) == half.chart_region)
                .unwrap()
                .1;
            for (f, sign) in chart.fields().into_iter().zip(parent.signs) {
                let [a, b, c, d] = f.values().map(f64::from);
                let value =
                    a * (1. - u) * (1. - v) + b * u * (1. - v) + c * u * v + d * (1. - u) * v;
                if value.abs() > 1e-9 {
                    assert_eq!(value.signum() as i8, sign);
                }
            }
        }
    }
    output.regions().len() > chart.regions().count()
}
fn input(
    values: [i16; 8],
    axis: usize,
    budget: Arc<MemoryBudget>,
) -> Arc<SplitProjectedConstraints> {
    let graph = Arc::new(build(&field(values), [0; 3], budget.clone()));
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
    Arc::new(contacts.split_with_budget(budget, || false).unwrap())
}
#[test]
fn connected_projected_graph_has_closed_consistent_faces() {
    let budget = MemoryBudget::new(128 * 1024 * 1024);
    let mut seed = 831u32;
    let mut cut = false;
    for case in 0..60 {
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
            let baseline = budget.used();
            let output = split
                .planar_graph_with_budget(budget.clone(), || false)
                .unwrap();
            assert_eq!(budget.used(), baseline + output.referenced_bytes());
            cut |= verify(&split, &output);
        }
    }
    assert!(cut, "fixtures must exercise a source cut");
    assert_eq!(budget.used(), 0);
}
#[test]
fn planar_graph_rejects_pressure_and_cancellation_without_leaks() {
    let budget = MemoryBudget::new(128 * 1024 * 1024);
    let split = input([6, -2, -2, -2, -2, -2, -2, 6], 0, budget.clone());
    let baseline = budget.used();
    let tiny = MemoryBudget::new(1);
    assert!(matches!(
        split.planar_graph_with_budget(tiny.clone(), || false),
        Err(SurfaceError::Memory)
    ));
    assert_eq!(tiny.used(), 0);
    let probe = MemoryBudget::new(128 * 1024 * 1024);
    let peak = Cell::new(0);
    let sampled = split
        .planar_graph_with_budget(probe.clone(), || {
            peak.set(peak.get().max(probe.used()));
            false
        })
        .unwrap();
    drop(sampled);
    assert_eq!(probe.used(), 0);
    assert!(peak.get() > 1);
    probe.set_limit(peak.get() - 1);
    assert!(matches!(
        split.planar_graph_with_budget(probe.clone(), || false),
        Err(SurfaceError::Memory)
    ));
    assert_eq!(probe.used(), 0);
    let calls = Cell::new(0);
    let output = split
        .planar_graph_with_budget(budget.clone(), || {
            calls.set(calls.get() + 1);
            false
        })
        .unwrap();
    let total = calls.get();
    verify(&split, &output);
    drop(output);
    for at in [1, 2, total / 3, 2 * total / 3, total] {
        let calls = Cell::new(0);
        assert!(matches!(
            split.planar_graph_with_budget(budget.clone(), || {
                calls.set(calls.get() + 1);
                calls.get() == at
            }),
            Err(SurfaceError::Cancelled)
        ));
        assert_eq!(budget.used(), baseline);
    }
}
