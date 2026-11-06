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
    let mut exact_faces = 0;
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
                        if let Some(ratio) = coordinate.rational_value()
                            && (ratio.numerator() == 0
                                || i128::from(ratio.numerator()) == i128::from(ratio.denominator()))
                        {
                            let endpoint = u32::from(ratio.numerator() != 0);
                            assert_eq!(
                                bounds,
                                CellAxisBounds {
                                    lower: endpoint,
                                    upper: endpoint,
                                    denominator: 1
                                }
                            );
                            exact_faces += 1;
                        }
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
    assert!(exact_faces > 0);
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

fn adjacent_mesh(
    field: &MaskField,
    lower: [usize; 3],
    budget: Arc<MemoryBudget>,
) -> Arc<TriangulatedPlanarGraph> {
    adjacent_mesh_axis(field, lower, 2, budget)
}
fn adjacent_mesh_axis(
    field: &MaskField,
    lower: [usize; 3],
    projection: usize,
    budget: Arc<MemoryBudget>,
) -> Arc<TriangulatedPlanarGraph> {
    let graph = Arc::new(build(field, lower, budget.clone()));
    let chart = Arc::new(
        graph
            .projection_chart_with_budget(projection, budget.clone(), || false)
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
    let refined = Arc::new(
        planar
            .refine_with_budget(limits(6), budget.clone(), || false)
            .unwrap(),
    );
    Arc::new(
        refined
            .triangulate_with_budget(PlanarTriangulationLimits::default(), budget, || false)
            .unwrap(),
    )
}

#[test]
fn neighboring_cells_keep_matching_source_face_vertices_on_the_exact_face() {
    let budget = MemoryBudget::new(128 * 1024 * 1024);
    let source = source([3, 2, 2], true, false);
    let field = fractional(source, |[x, y, z]| {
        if (x == 0 && y == 0 && z == 0)
            || (x == 1 && y == 1 && z == 1)
            || (x == 2 && y == 0 && z == 0)
        {
            26
        } else {
            -6
        }
    });
    let left = adjacent_mesh(&field, [0, 0, 0], budget.clone());
    let right = adjacent_mesh(&field, [1, 0, 0], budget.clone());
    let a = left
        .enclose_surface_with_budget(SurfaceEnclosureLimits::default(), budget.clone(), || false)
        .unwrap();
    let b = right
        .enclose_surface_with_budget(SurfaceEnclosureLimits::default(), budget.clone(), || false)
        .unwrap();
    let mut matched = 0usize;
    for i in 0..left.input().vertex_count() {
        let p = *left.input().vertex(i as u32).unwrap();
        if !p[0].compare(coordinate(1, 1)).unwrap().is_eq() {
            continue;
        }
        for j in 0..right.input().vertex_count() {
            let q = *right.input().vertex(j as u32).unwrap();
            if !q[0].compare(coordinate(0, 1)).unwrap().is_eq()
                || !p[1].compare(q[1]).unwrap().is_eq()
            {
                continue;
            }
            if let (Some(x), Some(y)) = (
                a.vertex_lattice_point(i as u32).unwrap(),
                b.vertex_lattice_point(j as u32).unwrap(),
            ) {
                assert_eq!(x[0], EnclosedSurfaceGraph::POSITION_DENOMINATOR);
                assert_eq!(y[0], 0);
                assert_eq!([x[1], x[2]], [y[1], y[2]]);
                matched += 1;
            }
        }
    }
    assert!(matched > 0);
}

fn adjacent_conforming(
    field: &MaskField,
    lower: [usize; 3],
    depth: u8,
    budget: Arc<MemoryBudget>,
) -> Arc<ConformingSurfaceMesh> {
    adjacent_conforming_axis(field, lower, 2, depth, budget)
}
fn adjacent_conforming_axis(
    field: &MaskField,
    lower: [usize; 3],
    projection: usize,
    depth: u8,
    budget: Arc<MemoryBudget>,
) -> Arc<ConformingSurfaceMesh> {
    let mesh = adjacent_mesh_axis(field, lower, projection, budget.clone());
    let enclosed = Arc::new(
        mesh.enclose_surface_with_budget(SurfaceEnclosureLimits::default(), budget.clone(), || {
            false
        })
        .unwrap(),
    );
    let count = enclosed
        .facets()
        .iter()
        .filter(|f| matches!(f, BoundedSurfaceFacet::Candidate { .. }))
        .count();
    Arc::new(
        enclosed
            .refine_conforming_with_budget(
                ConformingSurfaceLimits {
                    tolerance_bits: 12,
                    max_depth: depth,
                    max_vertices: enclosed.vertices().len() + 1024,
                    max_triangles: count + 1024,
                    max_operations: 2_000_000,
                    ..ConformingSurfaceLimits::default()
                },
                budget,
                || false,
            )
            .unwrap(),
    )
}

#[test]
fn adjacent_face_partition_preserves_exact_identity_and_requests_missing_splits() {
    let budget = MemoryBudget::new(128 * 1024 * 1024);
    let source = source([3, 2, 2], true, false);
    let field = fractional(source, |[x, y, z]| {
        if (x == 0 && y == 0 && z == 0)
            || (x == 1 && y == 1 && z == 1)
            || (x == 2 && y == 0 && z == 0)
        {
            26
        } else {
            -6
        }
    });
    let left = adjacent_conforming(&field, [0, 0, 0], 0, budget.clone());
    let right = adjacent_conforming(&field, [1, 0, 0], 7, budget.clone());
    let limits = FacePartitionLimits {
        max_edges_per_cell: 1024,
        max_points: 2048,
        ..FacePartitionLimits::default()
    };
    let baseline = budget.used();
    let partition = left
        .shared_face_partition_with_budget(&right, 0, limits, budget.clone(), || false)
        .unwrap();
    assert!(Arc::ptr_eq(&partition.inputs()[0], &left));
    assert!(Arc::ptr_eq(&partition.inputs()[1], &right));
    assert_eq!(
        partition.face(),
        FaceKey {
            origin: [1, 0, 0],
            axis: 0
        }
    );
    assert_eq!(partition.projection_axis(), 2);
    assert_eq!(budget.used(), baseline + partition.referenced_bytes());
    assert_eq!(
        partition.spans().len(),
        partition.points().len().saturating_sub(1)
    );
    assert!(partition.spans().iter().any(|span| {
        span.coverage
            .iter()
            .all(|c| matches!(c, FaceSpanCoverage::Present { .. }))
    }));
    let mut matched = 0;
    let mut requests = 0;
    for pair in partition.points().windows(2) {
        assert_eq!(
            pair[0].coordinate.compare(pair[1].coordinate).unwrap(),
            std::cmp::Ordering::Less
        );
    }
    for point in partition.points() {
        let mut positions = [None; 2];
        for (side, (mesh, placement)) in
            [(&left, point.placements[0]), (&right, point.placements[1])]
                .into_iter()
                .enumerate()
        {
            positions[side] = match placement {
                FacePlacement::Absent => None,
                FacePlacement::Existing { vertex } => {
                    Some(mesh.vertices()[vertex as usize].position().unwrap())
                }
                FacePlacement::Split {
                    triangle,
                    edge,
                    position,
                } => {
                    requests += 1;
                    let tri = &mesh.triangles()[triangle as usize];
                    assert!(tri.vertices.contains(&edge[0]) && tri.vertices.contains(&edge[1]));
                    let a = mesh.vertices()[edge[0] as usize].position().unwrap();
                    let b = mesh.vertices()[edge[1] as usize].position().unwrap();
                    assert!(position[1] > a[1].min(b[1]) && position[1] < a[1].max(b[1]));
                    let cell = field.cell([side, 0, 0]).unwrap().interpolant().unwrap();
                    let mut sample = [
                        if side == 0 { 1. } else { 0. },
                        point.coordinate.approximate().unwrap(),
                        0.,
                    ];
                    let lo = cell.evaluate(sample).unwrap();
                    sample[2] = 1.;
                    let hi = cell.evaluate(sample).unwrap();
                    let root = -lo / (hi - lo);
                    let actual = f64::from(position[2])
                        / f64::from(ConformingSurfaceMesh::POSITION_DENOMINATOR);
                    assert!((root - actual).abs() <= 1. / 131_072. + 1e-10);
                    Some(position)
                }
            };
        }
        if let (Some(a), Some(b)) = (positions[0], positions[1]) {
            assert_eq!(a[0], ConformingSurfaceMesh::POSITION_DENOMINATOR);
            assert_eq!(b[0], 0);
            assert_eq!([a[1], a[2]], [b[1], b[2]]);
            matched += 1;
        }
    }
    assert!(matched > 0);
    assert!(requests > 0);
    let reverse = right
        .shared_face_partition_with_budget(&left, 0, limits, budget.clone(), || false)
        .unwrap();
    assert_eq!(reverse.face(), partition.face());
    assert_eq!(reverse.points().len(), partition.points().len());
    assert_eq!(reverse.spans().len(), partition.spans().len());
    for (a, b) in partition.spans().iter().zip(reverse.spans()) {
        assert_eq!(a.points, b.points);
        assert_eq!(a.coverage, [b.coverage[1], b.coverage[0]]);
    }
    for (a, b) in partition.points().iter().zip(reverse.points()) {
        assert!(a.coordinate.compare(b.coordinate).unwrap().is_eq());
        assert_eq!(a.placements, [b.placements[1], b.placements[0]]);
    }
}

#[test]
fn face_partition_refuses_unrelated_sources_limits_and_cancellation() {
    let budget = MemoryBudget::new(128 * 1024 * 1024);
    let source = source([3, 2, 2], true, false);
    let sample = |[x, y, z]: [usize; 3]| {
        if (x == 0 && y == 0 && z == 0)
            || (x == 1 && y == 1 && z == 1)
            || (x == 2 && y == 0 && z == 0)
        {
            26
        } else {
            -6
        }
    };
    let field = fractional(source.clone(), sample);
    let other = fractional(source, sample);
    let left = adjacent_conforming(&field, [0, 0, 0], 0, budget.clone());
    let right = adjacent_conforming(&field, [1, 0, 0], 3, budget.clone());
    let unrelated = adjacent_conforming(&other, [1, 0, 0], 0, budget.clone());
    let limits = FacePartitionLimits {
        max_edges_per_cell: 1024,
        max_points: 2048,
        ..FacePartitionLimits::default()
    };
    let baseline = budget.used();
    assert!(matches!(
        left.shared_face_partition_with_budget(&unrelated, 0, limits, budget.clone(), || false),
        Err(SurfaceError::Geometry)
    ));
    assert!(matches!(
        left.shared_face_partition_with_budget(&left, 0, limits, budget.clone(), || false),
        Err(SurfaceError::Geometry)
    ));
    assert!(matches!(
        left.shared_face_partition_with_budget(&right, 2, limits, budget.clone(), || false),
        Err(SurfaceError::Geometry)
    ));
    assert!(matches!(
        left.shared_face_partition_with_budget(
            &right,
            0,
            FacePartitionLimits {
                max_operations: 1,
                ..limits
            },
            budget.clone(),
            || false
        ),
        Err(SurfaceError::Limit)
    ));
    let calls = Cell::new(0usize);
    let partition = left
        .shared_face_partition_with_budget(&right, 0, limits, budget.clone(), || {
            calls.set(calls.get() + 1);
            false
        })
        .unwrap();
    let total_calls = calls.get();
    let bytes = partition.referenced_bytes();
    drop(partition);
    assert_eq!(budget.used(), baseline);
    assert!(matches!(
        left.shared_face_partition_with_budget(
            &right,
            0,
            FacePartitionLimits {
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
        left.shared_face_partition_with_budget(&right, 0, limits, small.clone(), || false),
        Err(SurfaceError::Memory)
    ));
    assert_eq!(small.used(), 0);
    for stop in [1, total_calls / 2, total_calls] {
        let calls = Cell::new(0usize);
        assert!(matches!(
            left.shared_face_partition_with_budget(&right, 0, limits, budget.clone(), || {
                calls.set(calls.get() + 1);
                calls.get() >= stop
            }),
            Err(SurfaceError::Cancelled)
        ));
        assert_eq!(budget.used(), baseline);
    }
}

fn partition_fixture(budget: Arc<MemoryBudget>) -> Arc<SharedFacePartition> {
    let source = source([3, 2, 2], true, false);
    let field = fractional(source, |[x, y, z]| {
        if (x == 0 && y == 0 && z == 0)
            || (x == 1 && y == 1 && z == 1)
            || (x == 2 && y == 0 && z == 0)
        {
            26
        } else {
            -6
        }
    });
    let left = adjacent_conforming(&field, [0, 0, 0], 0, budget.clone());
    let right = adjacent_conforming(&field, [1, 0, 0], 7, budget.clone());
    Arc::new(
        left.shared_face_partition_with_budget(
            &right,
            0,
            FacePartitionLimits {
                max_edges_per_cell: 1024,
                max_points: 2048,
                ..FacePartitionLimits::default()
            },
            budget,
            || false,
        )
        .unwrap(),
    )
}

#[test]
fn applying_face_requests_preserves_area_and_matches_covered_segments() {
    use std::collections::{BTreeMap, BTreeSet};
    let budget = MemoryBudget::new(128 * 1024 * 1024);
    let partition = partition_fixture(budget.clone());
    let baseline = budget.used();
    let limits = AppliedFaceLimits {
        tolerance_bits: [6, 12],
        ..AppliedFaceLimits::default()
    };
    let applied = partition
        .apply_with_budget(limits, budget.clone(), || false)
        .unwrap();
    assert!(Arc::ptr_eq(applied.input(), &partition));
    assert_eq!(budget.used(), baseline + applied.referenced_bytes());
    assert_eq!(applied.point_vertices().len(), partition.points().len());
    assert_eq!(applied.spans().len(), partition.spans().len());
    let mut split_count = 0usize;
    let mut accepted = 0usize;
    for side in 0..2 {
        let source = &partition.inputs()[side];
        let output = &applied.cells()[side];
        let mut source_area = BTreeMap::<u32, i128>::new();
        let mut edges = BTreeMap::<(u32, u32), Vec<(u32, u32)>>::new();
        let mut used = BTreeSet::new();
        let field = source
            .input()
            .input()
            .input()
            .input()
            .input()
            .edges()
            .chart()
            .graph()
            .cell()
            .interpolant()
            .unwrap();
        for triangle in output.triangles() {
            let p = triangle
                .vertices
                .map(|id| output.vertices()[id as usize].position().unwrap());
            *source_area.entry(triangle.source_triangle).or_default() += area(p, 0, 1);
            assert_eq!(
                triangle.source_facet,
                source.triangles()[triangle.source_triangle as usize].source_facet
            );
            used.extend(triangle.vertices);
            for (a, b) in [
                (triangle.vertices[0], triangle.vertices[1]),
                (triangle.vertices[1], triangle.vertices[2]),
                (triangle.vertices[2], triangle.vertices[0]),
            ] {
                edges.entry((a.min(b), a.max(b))).or_default().push((a, b));
            }
            if let AppliedTriangleState::WithinTolerance {
                numerator,
                denominator,
            } = triangle.state
            {
                assert!(numerator * (1u128 << limits.tolerance_bits[side]) <= denominator);
                let bound = numerator as f64 / denominator as f64;
                let q = p.map(|point| {
                    point.map(|v| f64::from(v) / f64::from(AppliedFacePair::POSITION_DENOMINATOR))
                });
                for i in 0..=4 {
                    for j in 0..=4 - i {
                        let weights = [
                            f64::from(i) / 4.,
                            f64::from(j) / 4.,
                            f64::from(4 - i - j) / 4.,
                        ];
                        let mut sample: [f64; 3] =
                            std::array::from_fn(|d| (0..3).map(|k| weights[k] * q[k][d]).sum());
                        let z = sample[2];
                        sample[2] = 0.;
                        let lo = field.evaluate(sample).unwrap();
                        sample[2] = 1.;
                        let hi = field.evaluate(sample).unwrap();
                        assert!((z + lo / (hi - lo)).abs() <= bound + 1e-9);
                    }
                }
                accepted += 1;
            }
        }
        assert_eq!(source_area.len(), source.triangles().len());
        for (id, total) in source_area {
            let original = source.triangles()[id as usize]
                .vertices
                .map(|v| source.vertices()[v as usize].position().unwrap());
            assert_eq!(total, area(original, 0, 1));
        }
        for owners in edges.values() {
            assert!(owners.len() <= 2);
            if owners.len() == 2 {
                assert_eq!(owners[0], (owners[1].1, owners[1].0));
            }
        }
        for &(a, b) in edges.keys() {
            let pa = output.vertices()[a as usize].position().unwrap();
            let pb = output.vertices()[b as usize].position().unwrap();
            let dx = i128::from(pb[0]) - i128::from(pa[0]);
            let dy = i128::from(pb[1]) - i128::from(pa[1]);
            let length = dx * dx + dy * dy;
            for &id in &used {
                if id == a || id == b {
                    continue;
                }
                let p = output.vertices()[id as usize].position().unwrap();
                let x = i128::from(p[0]) - i128::from(pa[0]);
                let y = i128::from(p[1]) - i128::from(pa[1]);
                assert!(
                    !(dx * y == dy * x && x * dx + y * dy > 0 && x * dx + y * dy < length),
                    "hanging vertex {id} on edge {a}-{b}"
                );
            }
        }
        split_count += output
            .vertices()
            .iter()
            .filter(|v| matches!(v, AppliedFaceVertex::Split { .. }))
            .count();
        for span in applied.spans() {
            if let Some([a, b]) = span.segments[side] {
                assert!(edges.contains_key(&(a.min(b), a.max(b))));
            }
        }
    }
    assert!(split_count > 0);
    assert!(accepted > 0);
    let mut matched = 0;
    for span in applied.spans() {
        let original = partition.spans()[span.partition_span as usize];
        for side in 0..2 {
            assert_eq!(
                span.segments[side].is_some(),
                matches!(original.coverage[side], FaceSpanCoverage::Present { .. })
            );
        }
        if let [Some(a), Some(b)] = span.segments {
            for i in 0..2 {
                let left = applied.cells()[0].vertices()[a[i] as usize]
                    .position()
                    .unwrap();
                let right = applied.cells()[1].vertices()[b[i] as usize]
                    .position()
                    .unwrap();
                assert_eq!(left[0], AppliedFacePair::POSITION_DENOMINATOR);
                assert_eq!(right[0], 0);
                assert_eq!([left[1], left[2]], [right[1], right[2]]);
            }
            matched += 1;
        }
    }
    assert!(matched > 0);
    let reverse_partition = Arc::new(
        partition.inputs()[1]
            .shared_face_partition_with_budget(
                &partition.inputs()[0],
                0,
                FacePartitionLimits {
                    max_edges_per_cell: 1024,
                    max_points: 2048,
                    ..FacePartitionLimits::default()
                },
                budget.clone(),
                || false,
            )
            .unwrap(),
    );
    let reverse = reverse_partition
        .apply_with_budget(
            AppliedFaceLimits {
                tolerance_bits: [12, 6],
                ..limits
            },
            budget.clone(),
            || false,
        )
        .unwrap();
    for side in 0..2 {
        assert_eq!(
            applied.cells()[side].vertices(),
            reverse.cells()[1 - side].vertices()
        );
        assert_eq!(
            applied.cells()[side].triangles(),
            reverse.cells()[1 - side].triangles()
        );
    }
    for (forward, backward) in applied.spans().iter().zip(reverse.spans()) {
        assert_eq!(
            forward.segments,
            [backward.segments[1], backward.segments[0]]
        );
    }
    drop(reverse);
    drop(reverse_partition);
    drop(applied);
    assert_eq!(budget.used(), baseline);
}

#[test]
fn face_application_is_atomic_under_limits_and_cancellation() {
    let budget = MemoryBudget::new(128 * 1024 * 1024);
    let partition = partition_fixture(budget.clone());
    let before_vertices = partition
        .inputs()
        .each_ref()
        .map(|mesh| mesh.vertices().to_vec());
    let before_triangles = partition
        .inputs()
        .each_ref()
        .map(|mesh| mesh.triangles().to_vec());
    let baseline = budget.used();
    let limits = AppliedFaceLimits::default();
    let calls = Cell::new(0usize);
    let result = partition
        .apply_with_budget(limits, budget.clone(), || {
            calls.set(calls.get() + 1);
            false
        })
        .unwrap();
    let total_calls = calls.get();
    let bytes = result.referenced_bytes();
    let needed_vertices = result
        .cells()
        .iter()
        .map(|cell| cell.vertices().len())
        .max()
        .unwrap();
    let needed_triangles = result
        .cells()
        .iter()
        .map(|cell| cell.triangles().len())
        .max()
        .unwrap();
    drop(result);
    let strict = partition
        .apply_with_budget(
            AppliedFaceLimits {
                tolerance_bits: [16, 16],
                ..limits
            },
            budget.clone(),
            || false,
        )
        .unwrap();
    assert!(
        strict
            .cells()
            .iter()
            .flat_map(|cell| cell.triangles())
            .any(|triangle| matches!(triangle.state, AppliedTriangleState::Unresolved { .. }))
    );
    for triangle in strict.cells().iter().flat_map(|cell| cell.triangles()) {
        if let AppliedTriangleState::WithinTolerance {
            numerator,
            denominator,
        } = triangle.state
        {
            assert!(numerator * (1u128 << 16) <= denominator);
        }
    }
    drop(strict);
    assert!(matches!(
        partition.apply_with_budget(
            AppliedFaceLimits {
                tolerance_bits: [17, 8],
                ..limits
            },
            budget.clone(),
            || false
        ),
        Err(SurfaceError::Limit)
    ));
    assert!(matches!(
        partition.apply_with_budget(
            AppliedFaceLimits {
                max_vertices_per_cell: needed_vertices - 1,
                ..limits
            },
            budget.clone(),
            || false
        ),
        Err(SurfaceError::Limit)
    ));
    assert!(matches!(
        partition.apply_with_budget(
            AppliedFaceLimits {
                max_triangles_per_cell: needed_triangles - 1,
                ..limits
            },
            budget.clone(),
            || false
        ),
        Err(SurfaceError::Limit)
    ));
    assert!(matches!(
        partition.apply_with_budget(
            AppliedFaceLimits {
                max_operations: 1,
                ..limits
            },
            budget.clone(),
            || false
        ),
        Err(SurfaceError::Limit)
    ));
    assert!(matches!(
        partition.apply_with_budget(
            AppliedFaceLimits {
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
        partition.apply_with_budget(limits, small.clone(), || false),
        Err(SurfaceError::Memory)
    ));
    assert_eq!(small.used(), 0);
    for stop in [1, total_calls / 2, total_calls] {
        let calls = Cell::new(0usize);
        assert!(matches!(
            partition.apply_with_budget(limits, budget.clone(), || {
                calls.set(calls.get() + 1);
                calls.get() >= stop
            }),
            Err(SurfaceError::Cancelled)
        ));
        assert_eq!(budget.used(), baseline);
    }
    for side in 0..2 {
        assert_eq!(partition.inputs()[side].vertices(), before_vertices[side]);
        assert_eq!(partition.inputs()[side].triangles(), before_triangles[side]);
    }
}

#[test]
fn face_application_matches_segments_for_all_transverse_axis_pairs() {
    let budget = MemoryBudget::new(128 * 1024 * 1024);
    for face_axis in 0..3 {
        for projection in 0..3 {
            if face_axis == projection {
                continue;
            }
            let [a, b] = transverse(face_axis);
            let mut dimensions = [2; 3];
            dimensions[face_axis] = 3;
            let field = fractional(source(dimensions, true, false), |p| {
                if (p[face_axis] == 0 && p[a] == 0 && p[b] == 0)
                    || (p[face_axis] == 1 && p[a] == 1 && p[b] == 1)
                    || (p[face_axis] == 2 && p[a] == 0 && p[b] == 0)
                {
                    26
                } else {
                    -6
                }
            });
            let mut upper = [0; 3];
            upper[face_axis] = 1;
            let left = adjacent_conforming_axis(&field, [0; 3], projection, 0, budget.clone());
            let right = adjacent_conforming_axis(&field, upper, projection, 7, budget.clone());
            let partition = Arc::new(
                left.shared_face_partition_with_budget(
                    &right,
                    face_axis,
                    FacePartitionLimits {
                        max_edges_per_cell: 1024,
                        max_points: 2048,
                        ..FacePartitionLimits::default()
                    },
                    budget.clone(),
                    || false,
                )
                .unwrap(),
            );
            let result = partition
                .apply_with_budget(AppliedFaceLimits::default(), budget.clone(), || false)
                .unwrap();
            let mut matched = 0;
            for span in result.spans() {
                if let [Some(left), Some(right)] = span.segments {
                    for i in 0..2 {
                        let a = result.cells()[0].vertices()[left[i] as usize]
                            .position()
                            .unwrap();
                        let b = result.cells()[1].vertices()[right[i] as usize]
                            .position()
                            .unwrap();
                        assert_eq!(a[face_axis], AppliedFacePair::POSITION_DENOMINATOR);
                        assert_eq!(b[face_axis], 0);
                        for axis in 0..3 {
                            if axis != face_axis {
                                assert_eq!(a[axis], b[axis]);
                            }
                        }
                    }
                    matched += 1;
                }
            }
            assert!(matched > 0, "face {face_axis}, projection {projection}");
        }
    }
    assert_eq!(budget.used(), 0);
}
