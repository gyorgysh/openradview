// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use std::collections::{BTreeMap, BTreeSet};

fn xy(input: &RefinedPlanarGraph, id: u32) -> [f64; 2] {
    input.vertex(id).unwrap().map(|v| v.approximate().unwrap())
}
fn cross(a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> f64 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}
fn inside(poly: &[[f64; 2]], p: [f64; 2]) -> bool {
    let mut within = false;
    for (a, b) in poly
        .iter()
        .zip(poly.iter().cycle().skip(1))
        .take(poly.len())
    {
        if (a[1] > p[1]) != (b[1] > p[1])
            && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0]
        {
            within = !within;
        }
    }
    within
}
fn validate(input: &Arc<RefinedPlanarGraph>, mesh: &TriangulatedPlanarGraph) {
    assert!(Arc::ptr_eq(input, mesh.input()));
    let graph = input.input();
    let mut polys = Vec::new();
    for id in 0..graph.regions().len() {
        let mut poly = Vec::new();
        input
            .region_boundary(
                id as u32,
                || false,
                |n| {
                    poly.push(xy(input, n));
                    Ok(())
                },
            )
            .unwrap();
        polys.push(poly);
    }
    let mut used = BTreeSet::new();
    let mut counts = BTreeMap::<(u32, u32), usize>::new();
    let mut area = 0.;
    for triangle in mesh.triangles() {
        assert!((triangle.region as usize) < polys.len());
        let [a, b, c] = triangle.vertices.map(|id| xy(input, id));
        let signed = cross(a, b, c);
        assert!(signed > 0., "{signed:?}");
        area += signed / 2.;
        let center = [(a[0] + b[0] + c[0]) / 3., (a[1] + b[1] + c[1]) / 3.];
        assert!(
            inside(&polys[triangle.region as usize], center),
            "triangle centroid outside its region: {center:?}"
        );
        used.extend(triangle.vertices);
        for [a, b] in [
            [triangle.vertices[0], triangle.vertices[1]],
            [triangle.vertices[1], triangle.vertices[2]],
            [triangle.vertices[2], triangle.vertices[0]],
        ] {
            *counts.entry((a.min(b), a.max(b))).or_default() += 1;
        }
    }
    assert!((area - 1.).abs() < 1e-9, "area {area}");
    assert_eq!(used.len(), input.vertex_count());
    for (&(a, b), &count) in &counts {
        assert!((1..=2).contains(&count));
        if count == 1 {
            let [a, b] = [xy(input, a), xy(input, b)];
            assert!(
                (0..2).any(|k| (a[k].abs() < 1e-10 && b[k].abs() < 1e-10)
                    || ((a[k] - 1.).abs() < 1e-10 && (b[k] - 1.).abs() < 1e-10)),
                "unmatched interior edge {a:?} {b:?}"
            );
        }
    }
    // Every refined input boundary segment must occur in the mesh; vertical
    // segments may have extra vertices where another slab boundary meets them.
    for id in 0..graph.edges().len() {
        let path = input.edge_vertices(id as u32).unwrap();
        for pair in path.windows(2) {
            let [a, b] = [xy(input, pair[0]), xy(input, pair[1])];
            if (a[0] - b[0]).abs() > 1e-12 {
                assert!(counts.contains_key(&(pair[0].min(pair[1]), pair[0].max(pair[1]))));
            } else {
                let mut line: Vec<_> = (0..input.vertex_count())
                    .map(|i| i as u32)
                    .filter(|id| {
                        let p = xy(input, *id);
                        (p[0] - a[0]).abs() < 1e-12
                            && p[1] >= a[1].min(b[1]) - 1e-12
                            && p[1] <= a[1].max(b[1]) + 1e-12
                    })
                    .collect();
                line.sort_by(|a, b| xy(input, *a)[1].total_cmp(&xy(input, *b)[1]));
                for pair in line.windows(2) {
                    assert!(counts.contains_key(&(pair[0].min(pair[1]), pair[0].max(pair[1]))));
                }
            }
        }
    }
}
fn refined(values: [i16; 8], axis: usize, budget: Arc<MemoryBudget>) -> Arc<RefinedPlanarGraph> {
    let split = input(values, axis, budget.clone());
    let planar = Arc::new(
        split
            .planar_graph_with_budget(budget.clone(), || false)
            .unwrap(),
    );
    Arc::new(
        planar
            .refine_with_budget(limits(6), budget, || false)
            .unwrap(),
    )
}
#[test]
fn planar_triangles_cover_regions_preserve_boundaries_and_source_vertices() {
    let budget = MemoryBudget::new(128 * 1024 * 1024);
    let mut seed = 652u32;
    let mut cases = 0usize;
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
            let input = Arc::new(
                planar
                    .refine_with_budget(limits(6), budget.clone(), || false)
                    .unwrap(),
            );
            let baseline = budget.used();
            let mesh = input
                .triangulate_with_budget(
                    PlanarTriangulationLimits::default(),
                    budget.clone(),
                    || false,
                )
                .unwrap();
            assert_eq!(budget.used(), baseline + mesh.referenced_bytes());
            validate(&input, &mesh);
            cases += 1;
        }
    }
    assert!(cases >= 50);
    assert_eq!(budget.used(), 0);
}
#[test]
fn isolated_chart_point_is_retained_in_one_new_region() {
    let budget = MemoryBudget::new(128 * 1024 * 1024);
    let graph = Arc::new(build(
        &field([26, -6, -6, -6, -6, -6, -6, 26]),
        [0; 3],
        budget.clone(),
    ));
    let mut found = false;
    for x in 1..8 {
        for y in 1..8 {
            let p = [coordinate(x, 8), coordinate(y, 8)];
            let chart = Arc::new(
                graph
                    .projection_chart_with_insertions_and_budget(0, &[p], budget.clone(), || false)
                    .unwrap(),
            );
            let Some((id, _)) = chart
                .subdivision()
                .nodes()
                .find(|(_, n)| same2(*n.coordinates(), p))
            else {
                continue;
            };
            if chart.subdivision().interior_region(id).is_none() {
                continue;
            }
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
            let chart = planar.input().edges().chart().subdivision();
            let (id, _) = chart
                .nodes()
                .find(|(_, n)| same2(*n.coordinates(), p))
                .unwrap();
            if planar.edges().iter().any(|e| e.nodes.contains(&id)) {
                continue;
            }
            let input = Arc::new(
                planar
                    .refine_with_budget(limits(6), budget.clone(), || false)
                    .unwrap(),
            );
            let mesh = input
                .triangulate_with_budget(
                    PlanarTriangulationLimits::default(),
                    budget.clone(),
                    || false,
                )
                .unwrap();
            let regions: BTreeSet<_> = mesh
                .triangles()
                .iter()
                .filter(|t| t.vertices.contains(&u32::from(id)))
                .map(|t| t.region)
                .collect();
            assert_eq!(regions.len(), 1);
            validate(&input, &mesh);
            found = true;
            break;
        }
        if found {
            break;
        }
    }
    assert!(
        found,
        "fixture must retain an isolated interior chart point"
    );
}
#[test]
fn planar_triangulation_limits_cancellation_and_pressure_keep_input() {
    let budget = MemoryBudget::new(128 * 1024 * 1024);
    let input = refined([26, -6, -6, -6, -6, -6, -6, 26], 0, budget.clone());
    let baseline = budget.used();
    assert!(matches!(
        input.triangulate_with_budget(
            PlanarTriangulationLimits {
                max_triangles: 0,
                ..Default::default()
            },
            budget.clone(),
            || false
        ),
        Err(SurfaceError::Limit)
    ));
    assert!(matches!(
        input.triangulate_with_budget(
            PlanarTriangulationLimits {
                max_bytes: 0,
                ..Default::default()
            },
            budget.clone(),
            || false
        ),
        Err(SurfaceError::Limit)
    ));
    let tiny = MemoryBudget::new(1);
    assert!(matches!(
        input.triangulate_with_budget(Default::default(), tiny.clone(), || false),
        Err(SurfaceError::Memory)
    ));
    assert_eq!(tiny.used(), 0);
    let probe = MemoryBudget::new(128 * 1024 * 1024);
    let peak = Cell::new(0);
    let sampled = input
        .triangulate_with_budget(Default::default(), probe.clone(), || {
            peak.set(peak.get().max(probe.used()));
            false
        })
        .unwrap();
    drop(sampled);
    assert_eq!(probe.used(), 0);
    assert!(peak.get() > 1);
    probe.set_limit(peak.get() - 1);
    assert!(matches!(
        input.triangulate_with_budget(Default::default(), probe.clone(), || false),
        Err(SurfaceError::Memory)
    ));
    assert_eq!(probe.used(), 0);
    let calls = Cell::new(0);
    let mesh = input
        .triangulate_with_budget(Default::default(), budget.clone(), || {
            calls.set(calls.get() + 1);
            false
        })
        .unwrap();
    let total = calls.get();
    validate(&input, &mesh);
    drop(mesh);
    for at in [1, 2, total / 3, 2 * total / 3, total] {
        let calls = Cell::new(0);
        assert!(matches!(
            input.triangulate_with_budget(Default::default(), budget.clone(), || {
                calls.set(calls.get() + 1);
                calls.get() == at
            }),
            Err(SurfaceError::Cancelled)
        ));
        assert_eq!(budget.used(), baseline);
    }
}
