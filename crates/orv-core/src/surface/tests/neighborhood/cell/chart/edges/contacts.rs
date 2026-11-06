// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use std::collections::HashSet;

#[test]
fn source_contacts_preserve_owners_geometry_and_memory() {
    let budget = MemoryBudget::new(64 * 1024 * 1024);
    let mut seen = [0usize; 3];
    for values in [
        [2, -2, -2, -2, -2, -2, -2, 2],
        [6, -2, -2, -2, -2, -2, -2, 6],
        [0, 0, 2, 2, 2, 2, -4, -2],
        [0, 0, 2, 2, -2, -2, 0, 2],
    ] {
        let graph = Arc::new(build(&field(values), [0; 3], budget.clone()));
        for axis in 0..3 {
            let chart = Arc::new(
                graph
                    .projection_chart_with_budget(axis, budget.clone(), || false)
                    .unwrap(),
            );
            let projected = Arc::new(
                chart
                    .project_edges_with_budget(budget.clone(), || false)
                    .unwrap(),
            );
            let contacts = projected
                .contacts_with_budget(budget.clone(), || false)
                .unwrap();
            assert!(Arc::ptr_eq(contacts.edges(), &projected));
            assert_eq!(
                budget.used(),
                graph.referenced_bytes()
                    + chart.referenced_bytes()
                    + projected.referenced_bytes()
                    + contacts.referenced_bytes()
            );
            let mut owners = HashSet::new();
            let [u, v] = chart.transverse_axes();
            let on_edge = |id: u16, p: [ExactCoordinate; 2]| {
                let edge = &projected.edges()[usize::from(id)];
                let endpoints = edge.source_nodes.map(|n| {
                    let p = graph.nodes()[usize::from(n)].coordinates();
                    [p[u].approximate().unwrap(), p[v].approximate().unwrap()]
                });
                let p = p.map(|v| v.approximate().unwrap());
                (0..2).all(|i| {
                    p[i] >= endpoints[0][i].min(endpoints[1][i]) - 1e-12
                        && p[i] <= endpoints[0][i].max(endpoints[1][i]) + 1e-12
                })
            };
            for contact in contacts.contacts() {
                let pts = match contact.geometry {
                    ProjectedContactGeometry::Point(p) => {
                        seen[0] += 1;
                        vec![p]
                    }
                    ProjectedContactGeometry::Overlap(p) => {
                        seen[1] += 1;
                        vec![p[0], p[1]]
                    }
                };
                match contact.owner {
                    ProjectedContactOwner::Edges { first, second } => {
                        seen[2] += 1;
                        assert!(first < second);
                        assert!(owners.insert((0, first, second)));
                        for p in pts {
                            assert!(on_edge(first, p) && on_edge(second, p));
                        }
                    }
                    ProjectedContactOwner::EndFace { edge, upper } => {
                        assert!(owners.insert((1 + u8::from(upper), edge, 0)));
                        for p in pts {
                            assert!(on_edge(edge, p));
                            assert_eq!(
                                chart.subdivision().fields()[usize::from(upper)]
                                    .sign_at(p)
                                    .unwrap(),
                                0
                            );
                        }
                    }
                }
            }
            // Independently compare projected bounding boxes of every pair.
            // All non-end-face inputs are points or axis-aligned segments.
            for (i, first) in projected.edges().iter().enumerate() {
                if matches!(first.kind, ProjectedEdgeKind::EndFace { .. }) {
                    continue;
                }
                let bounds = |edge: &ProjectedEdge| {
                    edge.source_nodes.map(|n| {
                        let p = graph.nodes()[usize::from(n)].coordinates();
                        [p[u].approximate().unwrap(), p[v].approximate().unwrap()]
                    })
                };
                let a = bounds(first);
                for (j, second) in projected.edges().iter().enumerate().skip(i + 1) {
                    if matches!(second.kind, ProjectedEdgeKind::EndFace { .. }) {
                        continue;
                    }
                    let b = bounds(second);
                    let intersects = (0..2).all(|k| {
                        a[0][k].min(a[1][k]).max(b[0][k].min(b[1][k]))
                            <= a[0][k].max(a[1][k]).min(b[0][k].max(b[1][k])) + 1e-12
                    });
                    assert_eq!(owners.contains(&(0, i as u16, j as u16)), intersects);
                }
            }
            let baseline = budget.used();
            let calls = Cell::new(0);
            let another = projected
                .contacts_with_budget(budget.clone(), || {
                    calls.set(calls.get() + 1);
                    false
                })
                .unwrap();
            let total = calls.get();
            drop(another);
            for at in [1, total / 2, total] {
                let count = Cell::new(0);
                assert!(matches!(
                    projected.contacts_with_budget(budget.clone(), || {
                        count.set(count.get() + 1);
                        count.get() == at
                    }),
                    Err(SurfaceError::Cancelled)
                ));
                assert_eq!(budget.used(), baseline);
            }
            let insufficient = MemoryBudget::new(contacts.referenced_bytes() - 1);
            assert!(matches!(
                projected.contacts_with_budget(insufficient.clone(), || false),
                Err(SurfaceError::Memory)
            ));
            assert_eq!(insufficient.used(), 0);
            assert!(matches!(
                projected.contacts_with_budget(budget.clone(), || true),
                Err(SurfaceError::Cancelled)
            ));
        }
    }
    assert!(seen[0] > 0 && seen[2] > 0, "{seen:?}");
    assert_eq!(budget.used(), 0);
}
