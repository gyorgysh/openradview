// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use std::cell::Cell;

fn input(axis: usize, budget: Arc<MemoryBudget>) -> Arc<TriangulatedPlanarGraph> {
    let graph = refined([26, -6, -6, -6, -6, -6, -6, 26], axis, budget.clone());
    Arc::new(
        graph
            .triangulate_with_budget(PlanarTriangulationLimits::default(), budget, || false)
            .unwrap(),
    )
}

#[test]
fn source_bound_vertex_boxes_enclose_independent_field_values_on_all_axes() {
    let budget = MemoryBudget::new(128 * 1024 * 1024);
    let values = [26, -6, -6, -6, -6, -6, -6, 26];
    let mut candidates = 0;
    for axis in 0..3 {
        let mesh = input(axis, budget.clone());
        let before = budget.used();
        let enclosed = mesh
            .enclose_surface_with_budget(SurfaceEnclosureLimits::default(), budget.clone(), || {
                false
            })
            .unwrap();
        assert!(Arc::ptr_eq(&mesh, enclosed.input()));
        assert_eq!(budget.used(), before + enclosed.referenced_bytes());
        assert_eq!(enclosed.vertices().len(), mesh.input().vertex_count());
        assert_eq!(enclosed.facets().len(), mesh.triangles().len());
        let chart = mesh.input().input().input().edges().chart();
        let [u, v] = chart.transverse_axes();
        for (id, vertex) in enclosed.vertices().iter().enumerate() {
            let point = *mesh.input().vertex(id as u32).unwrap();
            match (
                vertex,
                mesh.vertex_fiber_bounds(id as u32, 16, || false).unwrap(),
            ) {
                (BoundedSurfaceVertex::Empty, CellFiberBounds::Empty)
                | (BoundedSurfaceVertex::Whole, CellFiberBounds::Whole) => {}
                (
                    BoundedSurfaceVertex::Point { axes, normal },
                    CellFiberBounds::Point {
                        lower,
                        upper,
                        denominator,
                        normal: expected,
                    },
                ) => {
                    assert_eq!(*normal, expected);
                    assert_eq!(
                        axes[axis],
                        CellAxisBounds {
                            lower,
                            upper,
                            denominator
                        }
                    );
                    for (bounds, coordinate) in [(axes[u], point[0]), (axes[v], point[1])] {
                        let value = coordinate.approximate().unwrap();
                        assert!(bounds.upper - bounds.lower <= 1);
                        assert!(
                            value >= f64::from(bounds.lower) / f64::from(bounds.denominator) - 1e-9
                        );
                        assert!(
                            value <= f64::from(bounds.upper) / f64::from(bounds.denominator) + 1e-9
                        );
                    }
                    let mut p = [0.; 3];
                    p[u] = point[0].approximate().unwrap();
                    p[v] = point[1].approximate().unwrap();
                    let lo = tensor(values, p, None);
                    p[axis] = 1.;
                    let hi = tensor(values, p, None);
                    if (hi - lo).abs() > 1e-8 {
                        let height = -lo / (hi - lo);
                        assert!(height >= f64::from(lower) / f64::from(denominator) - 1e-9);
                        assert!(height <= f64::from(upper) / f64::from(denominator) + 1e-9);
                    }
                }
                mismatch => panic!("mismatched source fiber: {mismatch:?}"),
            }
        }
        for (triangle, state) in mesh.triangles().iter().zip(enclosed.facets()) {
            if let BoundedSurfaceFacet::Candidate { normal } = state {
                candidates += 1;
                for id in triangle.vertices {
                    assert!(
                        matches!(enclosed.vertices()[id as usize], BoundedSurfaceVertex::Point { normal: n, .. } if n == *normal)
                    );
                }
            }
        }
        drop(enclosed);
        assert_eq!(budget.used(), before);
    }
    assert!(candidates > 0);
    assert_eq!(budget.used(), 0);
}

#[test]
fn enclosure_admission_and_cancellation_never_publish_partial_results() {
    let budget = MemoryBudget::new(128 * 1024 * 1024);
    let mesh = input(0, budget.clone());
    let baseline = budget.used();
    let limits = SurfaceEnclosureLimits::default();
    let completed = mesh
        .enclose_surface_with_budget(limits, budget.clone(), || false)
        .unwrap();
    let needed = completed.referenced_bytes();
    drop(completed);
    assert!(matches!(
        mesh.enclose_surface_with_budget(
            SurfaceEnclosureLimits {
                precision_bits: 17,
                ..limits
            },
            budget.clone(),
            || false
        ),
        Err(SurfaceError::Limit)
    ));
    assert!(matches!(
        mesh.enclose_surface_with_budget(
            SurfaceEnclosureLimits {
                max_bytes: needed - 1,
                ..limits
            },
            budget.clone(),
            || false
        ),
        Err(SurfaceError::Limit)
    ));
    let small = MemoryBudget::new(needed - 1);
    assert!(matches!(
        mesh.enclose_surface_with_budget(limits, small.clone(), || false),
        Err(SurfaceError::Memory)
    ));
    assert_eq!(small.used(), 0);
    for stop in [1, 2, 10, 100] {
        let calls = Cell::new(0usize);
        assert!(matches!(
            mesh.enclose_surface_with_budget(limits, budget.clone(), || {
                calls.set(calls.get() + 1);
                calls.get() >= stop
            }),
            Err(SurfaceError::Cancelled)
        ));
        assert_eq!(budget.used(), baseline);
    }
    let coarse = mesh
        .enclose_surface_with_budget(
            SurfaceEnclosureLimits {
                precision_bits: 0,
                ..limits
            },
            budget.clone(),
            || false,
        )
        .unwrap();
    assert_eq!(coarse.facets().len(), mesh.triangles().len());
    drop(coarse);
    assert_eq!(budget.used(), baseline);
}

#[test]
fn certified_source_facets_bound_sampled_surface_interior() {
    let budget = MemoryBudget::new(128 * 1024 * 1024);
    let values = [26, -6, -6, -6, -6, -6, -6, 26];
    let mut certified = 0usize;
    for axis in 0..3 {
        let mesh = input(axis, budget.clone());
        let enclosed = mesh
            .enclose_surface_with_budget(SurfaceEnclosureLimits::default(), budget.clone(), || {
                false
            })
            .unwrap();
        let chart = mesh.input().input().input().edges().chart();
        let [u, v] = chart.transverse_axes();
        for (id, triangle) in mesh.triangles().iter().enumerate() {
            let result = enclosed.facet_certificate(id as u32, || false).unwrap();
            let SurfaceFacetCertificate::Certified {
                numerator,
                denominator,
            } = result
            else {
                continue;
            };
            let bound = numerator as f64 / denominator as f64;
            let vertices =
                triangle.vertices.map(|v| {
                    enclosed.vertex_lattice_point(v).unwrap().unwrap().map(|c| {
                        f64::from(c) / f64::from(EnclosedSurfaceGraph::POSITION_DENOMINATOR)
                    })
                });
            for i in 0..=8 {
                for j in 0..=8 - i {
                    let w = [
                        f64::from(i) / 8.,
                        f64::from(j) / 8.,
                        f64::from(8 - i - j) / 8.,
                    ];
                    let mut p =
                        std::array::from_fn(|d| (0..3).map(|k| w[k] * vertices[k][d]).sum::<f64>());
                    let z = p[axis];
                    p[axis] = 0.;
                    let lo = tensor(values, p, None);
                    p[axis] = 1.;
                    let hi = tensor(values, p, None);
                    assert!((0. ..=1.).contains(&p[u]) && (0. ..=1.).contains(&p[v]));
                    let root = -lo / (hi - lo);
                    assert!((-1e-12..=1. + 1e-12).contains(&root));
                    assert!((z - root).abs() <= bound + 1e-10);
                }
            }
            certified += 1;
        }
        assert!(matches!(
            enclosed.facet_certificate(mesh.triangles().len() as u32, || false),
            Err(SurfaceError::Geometry)
        ));
        assert!(matches!(
            enclosed.facet_certificate(0, || true),
            Err(SurfaceError::Cancelled)
        ));
    }
    assert!(certified > 0);
}

fn area(points: [[u32; 3]; 3], u: usize, v: usize) -> i128 {
    let delta = |a: usize, b: usize, d: usize| i128::from(points[a][d]) - i128::from(points[b][d]);
    delta(1, 0, u) * delta(2, 0, v) - delta(1, 0, v) * delta(2, 0, u)
}

#[test]
fn adaptive_children_preserve_parent_domain_and_bound_source_zeroes() {
    let budget = MemoryBudget::new(128 * 1024 * 1024);
    let values = [26, -6, -6, -6, -6, -6, -6, 26];
    let mut divided = 0usize;
    let mut accepted = 0usize;
    for axis in 0..3 {
        let mesh = input(axis, budget.clone());
        let enclosed = Arc::new(
            mesh.enclose_surface_with_budget(
                SurfaceEnclosureLimits::default(),
                budget.clone(),
                || false,
            )
            .unwrap(),
        );
        let chart = mesh.input().input().input().edges().chart();
        let [u, v] = chart.transverse_axes();
        for (id, triangle) in mesh.triangles().iter().enumerate() {
            if !matches!(enclosed.facets()[id], BoundedSurfaceFacet::Candidate { .. }) {
                continue;
            }
            let limits = AdaptiveFacetLimits {
                tolerance_bits: 8,
                max_depth: 6,
                max_triangles: 64,
                ..AdaptiveFacetLimits::default()
            };
            let result = enclosed
                .refine_facet_with_budget(id as u32, limits, budget.clone(), || false)
                .unwrap();
            assert!(Arc::ptr_eq(result.input(), &enclosed));
            assert_eq!(result.source_facet(), id as u32);
            let parent = triangle.vertices.map(|v| {
                enclosed
                    .vertex_lattice_point(v)
                    .unwrap()
                    .unwrap()
                    .map(|x| x * 256)
            });
            let mut combined = 0i128;
            for leaf in result.triangles() {
                assert!(leaf.depth <= limits.max_depth);
                let part = area(leaf.vertices, u, v);
                if area(parent, u, v) > 0 {
                    assert!(part > 0);
                }
                combined += part;
                if let AdaptiveFacetState::WithinTolerance {
                    numerator,
                    denominator,
                } = leaf.state
                {
                    assert!(numerator * (1u128 << limits.tolerance_bits) <= denominator);
                    let bound = numerator as f64 / denominator as f64;
                    let grid = f64::from(AdaptiveFacetMesh::POSITION_DENOMINATOR);
                    let vertex = leaf.vertices.map(|p| p.map(|c| f64::from(c) / grid));
                    for i in 0..=4 {
                        for j in 0..=4 - i {
                            let w = [
                                f64::from(i) / 4.,
                                f64::from(j) / 4.,
                                f64::from(4 - i - j) / 4.,
                            ];
                            let mut p: [f64; 3] =
                                std::array::from_fn(|d| (0..3).map(|k| w[k] * vertex[k][d]).sum());
                            let z = p[axis];
                            p[axis] = 0.;
                            let lo = tensor(values, p, None);
                            p[axis] = 1.;
                            let hi = tensor(values, p, None);
                            let root = -lo / (hi - lo);
                            assert!((-1e-10..=1. + 1e-10).contains(&root));
                            assert!((z - root).abs() <= bound + 1e-9);
                        }
                    }
                    accepted += 1;
                }
            }
            assert_eq!(combined, area(parent, u, v));
            assert!(result.triangles().len() <= limits.max_triangles);
            if result.triangles().len() > 1 {
                divided += 1;
            }
        }
    }
    assert!(divided > 0);
    assert!(accepted > 0);
    assert_eq!(budget.used(), 0);
}

#[test]
fn adaptive_limits_cancellation_and_memory_refuse_partial_output() {
    let budget = MemoryBudget::new(128 * 1024 * 1024);
    let mesh = input(0, budget.clone());
    let enclosed = Arc::new(
        mesh.enclose_surface_with_budget(SurfaceEnclosureLimits::default(), budget.clone(), || {
            false
        })
        .unwrap(),
    );
    let id = enclosed
        .facets()
        .iter()
        .position(|f| matches!(f, BoundedSurfaceFacet::Candidate { .. }))
        .unwrap() as u32;
    let baseline = budget.used();
    let limits = AdaptiveFacetLimits {
        tolerance_bits: 16,
        max_depth: 5,
        max_triangles: 32,
        ..AdaptiveFacetLimits::default()
    };
    let total_calls = Cell::new(0usize);
    let output = enclosed
        .refine_facet_with_budget(id, limits, budget.clone(), || {
            total_calls.set(total_calls.get() + 1);
            false
        })
        .unwrap();
    let bytes = output.referenced_bytes();
    drop(output);
    assert_eq!(budget.used(), baseline);
    assert!(matches!(
        enclosed.refine_facet_with_budget(
            id,
            AdaptiveFacetLimits {
                max_depth: 9,
                ..limits
            },
            budget.clone(),
            || false
        ),
        Err(SurfaceError::Limit)
    ));
    assert!(matches!(
        enclosed.refine_facet_with_budget(
            id,
            AdaptiveFacetLimits {
                max_triangles: 0,
                ..limits
            },
            budget.clone(),
            || false
        ),
        Err(SurfaceError::Limit)
    ));
    assert!(matches!(
        enclosed.refine_facet_with_budget(
            id,
            AdaptiveFacetLimits {
                max_bytes: bytes - 1,
                ..limits
            },
            budget.clone(),
            || false
        ),
        Err(SurfaceError::Limit)
    ));
    let small = MemoryBudget::new(bytes);
    assert!(matches!(
        enclosed.refine_facet_with_budget(id, limits, small.clone(), || false),
        Err(SurfaceError::Memory)
    ));
    assert_eq!(small.used(), 0);
    for stop in [1, total_calls.get() / 2, total_calls.get()] {
        let calls = Cell::new(0usize);
        assert!(matches!(
            enclosed.refine_facet_with_budget(id, limits, budget.clone(), || {
                calls.set(calls.get() + 1);
                calls.get() >= stop
            }),
            Err(SurfaceError::Cancelled)
        ));
        assert_eq!(budget.used(), baseline);
    }
}

#[test]
fn adaptive_requested_tolerance_is_not_lowered_by_depth_or_triangle_caps() {
    let budget = MemoryBudget::new(128 * 1024 * 1024);
    let mesh = input(0, budget.clone());
    let enclosed = Arc::new(
        mesh.enclose_surface_with_budget(SurfaceEnclosureLimits::default(), budget.clone(), || {
            false
        })
        .unwrap(),
    );
    let id = (0..mesh.triangles().len()).find(|id| {
        matches!(enclosed.facet_certificate(*id as u32, || false).unwrap(), SurfaceFacetCertificate::Certified { numerator, denominator } if numerator * (1 << 16) > denominator)
    }).expect("facet requiring refinement") as u32;
    let limits = AdaptiveFacetLimits {
        tolerance_bits: 16,
        ..AdaptiveFacetLimits::default()
    };
    let depth = enclosed
        .refine_facet_with_budget(
            id,
            AdaptiveFacetLimits {
                max_depth: 0,
                ..limits
            },
            budget.clone(),
            || false,
        )
        .unwrap();
    assert_eq!(depth.triangles().len(), 1);
    assert!(matches!(
        depth.triangles()[0].state,
        AdaptiveFacetState::Unresolved {
            reason: AdaptiveStopReason::DepthLimit,
            ..
        }
    ));
    let count = enclosed
        .refine_facet_with_budget(
            id,
            AdaptiveFacetLimits {
                max_triangles: 1,
                ..limits
            },
            budget.clone(),
            || false,
        )
        .unwrap();
    assert_eq!(count.triangles().len(), 1);
    assert!(matches!(
        count.triangles()[0].state,
        AdaptiveFacetState::Unresolved {
            reason: AdaptiveStopReason::TriangleLimit,
            ..
        }
    ));
}

#[test]
fn shared_candidate_edges_refine_without_hanging_vertices() {
    use std::collections::{BTreeMap, BTreeSet};
    let budget = MemoryBudget::new(128 * 1024 * 1024);
    let mut split_count = 0usize;
    let mut propagated = 0usize;
    let mut shared_splits = 0usize;
    for axis in 0..3 {
        let mesh = input(axis, budget.clone());
        let enclosed = Arc::new(
            mesh.enclose_surface_with_budget(
                SurfaceEnclosureLimits::default(),
                budget.clone(),
                || false,
            )
            .unwrap(),
        );
        let initial = enclosed
            .facets()
            .iter()
            .filter(|f| matches!(f, BoundedSurfaceFacet::Candidate { .. }))
            .count();
        let limits = ConformingSurfaceLimits {
            tolerance_bits: 8,
            max_depth: 5,
            max_vertices: enclosed.vertices().len() + 1024,
            max_triangles: initial + 1024,
            max_operations: 2_000_000,
            ..ConformingSurfaceLimits::default()
        };
        let baseline = budget.used();
        let refined = enclosed
            .refine_conforming_with_budget(limits, budget.clone(), || false)
            .unwrap();
        assert!(Arc::ptr_eq(refined.input(), &enclosed));
        assert_eq!(budget.used(), baseline + refined.referenced_bytes());
        let chart = mesh.input().input().input().edges().chart();
        let [u, v] = chart.transverse_axes();
        let mut original_edges = BTreeMap::<(u32, u32), Vec<u32>>::new();
        for (id, triangle) in mesh.triangles().iter().enumerate() {
            if !matches!(enclosed.facets()[id], BoundedSurfaceFacet::Candidate { .. }) {
                continue;
            }
            for (a, b) in [
                (triangle.vertices[0], triangle.vertices[1]),
                (triangle.vertices[1], triangle.vertices[2]),
                (triangle.vertices[2], triangle.vertices[0]),
            ] {
                original_edges
                    .entry((a.min(b), a.max(b)))
                    .or_default()
                    .push(id as u32);
            }
        }
        let mut area_by_source = BTreeMap::<u32, i128>::new();
        let mut edges = BTreeMap::<(u32, u32), Vec<(u32, u32)>>::new();
        let mut used = BTreeSet::new();
        for triangle in refined.triangles() {
            let p = triangle
                .vertices
                .map(|id| refined.vertices()[id as usize].position().unwrap());
            *area_by_source.entry(triangle.source_facet).or_default() += area(p, u, v);
            used.extend(triangle.vertices);
            for (a, b) in [
                (triangle.vertices[0], triangle.vertices[1]),
                (triangle.vertices[1], triangle.vertices[2]),
                (triangle.vertices[2], triangle.vertices[0]),
            ] {
                edges.entry((a.min(b), a.max(b))).or_default().push((a, b));
            }
            if let ConformingFacetState::WithinTolerance {
                numerator,
                denominator,
            } = triangle.state
            {
                assert!(numerator * (1u128 << limits.tolerance_bits) <= denominator);
                let bound = numerator as f64 / denominator as f64;
                let grid = f64::from(ConformingSurfaceMesh::POSITION_DENOMINATOR);
                let vertices = p.map(|point| point.map(|coordinate| f64::from(coordinate) / grid));
                for i in 0..=4 {
                    for j in 0..=4 - i {
                        let weights = [
                            f64::from(i) / 4.,
                            f64::from(j) / 4.,
                            f64::from(4 - i - j) / 4.,
                        ];
                        let mut position: [f64; 3] = std::array::from_fn(|d| {
                            (0..3).map(|k| weights[k] * vertices[k][d]).sum()
                        });
                        let height = position[axis];
                        position[axis] = 0.;
                        let lo = tensor([26, -6, -6, -6, -6, -6, -6, 26], position, None);
                        position[axis] = 1.;
                        let hi = tensor([26, -6, -6, -6, -6, -6, -6, 26], position, None);
                        assert!((height + lo / (hi - lo)).abs() <= bound + 1e-9);
                    }
                }
            }
        }
        for (source, total) in &area_by_source {
            let original = mesh.triangles()[*source as usize].vertices.map(|id| {
                enclosed
                    .vertex_lattice_point(id)
                    .unwrap()
                    .unwrap()
                    .map(|v| v * 256)
            });
            assert_eq!(*total, area(original, u, v));
        }
        assert_eq!(area_by_source.len(), initial);
        for owners in edges.values() {
            assert!(owners.len() <= 2);
            if owners.len() == 2 {
                assert_eq!(owners[0], (owners[1].1, owners[1].0));
                if owners[0].0 as usize >= enclosed.vertices().len()
                    || owners[0].1 as usize >= enclosed.vertices().len()
                {
                    propagated += 1;
                }
            }
        }
        for &(a, b) in edges.keys() {
            let pa = refined.vertices()[a as usize].position().unwrap();
            let pb = refined.vertices()[b as usize].position().unwrap();
            let dx = i128::from(pb[u]) - i128::from(pa[u]);
            let dy = i128::from(pb[v]) - i128::from(pa[v]);
            let length = dx * dx + dy * dy;
            for &id in &used {
                if id == a || id == b {
                    continue;
                }
                let p = refined.vertices()[id as usize].position().unwrap();
                let x = i128::from(p[u]) - i128::from(pa[u]);
                let y = i128::from(p[v]) - i128::from(pa[v]);
                assert!(
                    !(dx * y == dy * x && x * dx + y * dy > 0 && x * dx + y * dy < length),
                    "hanging vertex {id} on edge {a}-{b}"
                );
            }
        }
        for (id, vertex) in refined.vertices().iter().enumerate() {
            if let ConformingVertex::Midpoint {
                parents: [a, b], ..
            } = *vertex
                && let Some(owners) = original_edges.get(&(a.min(b), a.max(b)))
                && owners.len() == 2
            {
                for owner in owners {
                    assert!(
                        refined
                            .triangles()
                            .iter()
                            .any(|triangle| triangle.source_facet == *owner
                                && triangle.vertices.contains(&(id as u32)))
                    );
                }
                shared_splits += 1;
            }
        }
        split_count += refined.vertices().len() - enclosed.vertices().len();
        drop(refined);
        assert_eq!(budget.used(), baseline);
    }
    assert!(split_count > 0);
    assert!(propagated > 0);
    assert!(shared_splits > 0);
    assert_eq!(budget.used(), 0);
}

#[test]
fn shared_refinement_limits_and_cancellation_keep_the_input_intact() {
    let budget = MemoryBudget::new(128 * 1024 * 1024);
    let mesh = input(0, budget.clone());
    let enclosed = Arc::new(
        mesh.enclose_surface_with_budget(SurfaceEnclosureLimits::default(), budget.clone(), || {
            false
        })
        .unwrap(),
    );
    let candidate_count = enclosed
        .facets()
        .iter()
        .filter(|f| matches!(f, BoundedSurfaceFacet::Candidate { .. }))
        .count();
    let limits = ConformingSurfaceLimits {
        max_vertices: enclosed.vertices().len() + 128,
        max_triangles: candidate_count + 128,
        max_operations: 100_000,
        ..ConformingSurfaceLimits::default()
    };
    let baseline = budget.used();
    let calls = Cell::new(0usize);
    let refined = enclosed
        .refine_conforming_with_budget(limits, budget.clone(), || {
            calls.set(calls.get() + 1);
            false
        })
        .unwrap();
    let total_calls = calls.get();
    let bytes = refined.referenced_bytes();
    drop(refined);
    assert!(matches!(
        enclosed.refine_conforming_with_budget(
            ConformingSurfaceLimits {
                max_vertices: enclosed.vertices().len() - 1,
                ..limits
            },
            budget.clone(),
            || false
        ),
        Err(SurfaceError::Limit)
    ));
    assert!(matches!(
        enclosed.refine_conforming_with_budget(
            ConformingSurfaceLimits {
                max_triangles: candidate_count - 1,
                ..limits
            },
            budget.clone(),
            || false
        ),
        Err(SurfaceError::Limit)
    ));
    assert!(matches!(
        enclosed.refine_conforming_with_budget(
            ConformingSurfaceLimits {
                max_bytes: bytes - 1,
                ..limits
            },
            budget.clone(),
            || false
        ),
        Err(SurfaceError::Limit)
    ));
    let small = MemoryBudget::new(bytes);
    assert!(matches!(
        enclosed.refine_conforming_with_budget(limits, small.clone(), || false),
        Err(SurfaceError::Memory)
    ));
    assert_eq!(small.used(), 0);
    for stop in [1, total_calls / 2, total_calls] {
        let calls = Cell::new(0usize);
        assert!(matches!(
            enclosed.refine_conforming_with_budget(limits, budget.clone(), || {
                calls.set(calls.get() + 1);
                calls.get() >= stop
            }),
            Err(SurfaceError::Cancelled)
        ));
        assert_eq!(budget.used(), baseline);
    }
    let limited = enclosed
        .refine_conforming_with_budget(
            ConformingSurfaceLimits {
                max_operations: 1,
                ..limits
            },
            budget.clone(),
            || false,
        )
        .unwrap();
    assert_eq!(limited.triangles().len(), candidate_count);
    assert!(limited.triangles().iter().any(|t| matches!(
        t.state,
        ConformingFacetState::Unresolved {
            reason: ConformingStopReason::WorkLimit,
            ..
        }
    )));
}
