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
