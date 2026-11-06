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
