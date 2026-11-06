// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

fn coordinate(n: i16, denominator: i16) -> ExactCoordinate {
    let f = Trilinear::new(std::array::from_fn(|i| {
        if i & 1 == 0 { n } else { n - denominator }
    }))
    .unwrap();
    let EdgeZero::Point(r) = f.edge(0, 0).unwrap() else {
        panic!("point")
    };
    ExactCoordinate::from_rational(r).unwrap().unwrap()
}

#[test]
fn cell_projection_preserves_graph_constraints_and_exact_fibers() {
    let budget = MemoryBudget::new(64 * 1024 * 1024);
    let mut whole_count = 0;
    for values in [
        [2, -2, -2, -2, -2, -2, -2, 2],
        [6, -2, -2, -2, -2, -2, -2, 6],
        [2, -2, -2, 2, 6, 2, 2, 6],
        [0, 0, 2, 2, 2, 2, -4, -2],
        [0, 0, 2, 2, -2, -2, 0, 2],
    ] {
        let field = field(values);
        let graph = Arc::new(build(&field, [0; 3], budget.clone()));
        for axis in 0..3 {
            let chart = graph
                .projection_chart_with_budget(axis, budget.clone(), || false)
                .unwrap();
            assert!(Arc::ptr_eq(chart.graph(), &graph));
            assert_eq!(chart.graph().field().revision(), field.revision());
            assert_eq!(chart.axis(), axis as u8);
            let [u, v] = chart.transverse_axes();
            for node in graph.nodes() {
                let p = node.coordinates();
                assert!(chart.subdivision().nodes().any(|(_, n)| {
                    let c = n.coordinates();
                    c[0].compare(p[u]).unwrap().is_eq() && c[1].compare(p[v]).unwrap().is_eq()
                }));
            }
            assert_eq!(
                chart.subdivision().fields(),
                [
                    graph
                        .cell()
                        .interpolant()
                        .unwrap()
                        .face(axis, false)
                        .unwrap(),
                    graph
                        .cell()
                        .interpolant()
                        .unwrap()
                        .face(axis, true)
                        .unwrap(),
                ]
            );
            for i in 0..=8 {
                for j in 0..=8 {
                    let point = [coordinate(i, 8), coordinate(j, 8)];
                    let mut p = [0.; 3];
                    p[u] = f64::from(i) / 8.;
                    p[v] = f64::from(j) / 8.;
                    let lo = tensor(values, p, None);
                    p[axis] = 1.;
                    let hi = tensor(values, p, None);
                    match chart.fiber(point).unwrap() {
                        CellFiber::Empty => assert!(lo * hi > 0.),
                        CellFiber::Whole => {
                            assert_eq!((lo, hi), (0., 0.));
                            whole_count += 1;
                        }
                        CellFiber::Point { height, normal } => {
                            assert!(lo * hi <= 0. && (lo != 0. || hi != 0.));
                            assert_eq!(normal, (hi - lo).signum() as i8);
                            p[axis] = height.as_f64();
                            assert!((0. ..=1.).contains(&p[axis]));
                            assert!(tensor(values, p, None).abs() < 1e-12);
                        }
                    }
                }
            }
            assert_eq!(
                budget.used(),
                graph.referenced_bytes() + chart.referenced_bytes()
            );
        }
    }
    assert!(whole_count > 0);
    assert_eq!(budget.used(), 0);
}

#[test]
fn projection_rejects_invalid_scope_cancellation_and_admission_without_leaking() {
    let budget = MemoryBudget::new(64 * 1024 * 1024);
    let graph = Arc::new(build(
        &field([2, -2, -2, -2, -2, -2, -2, 2]),
        [0; 3],
        budget.clone(),
    ));
    let baseline = budget.used();
    assert!(matches!(
        graph.projection_chart_with_budget(3, budget.clone(), || false),
        Err(SurfaceError::Geometry)
    ));
    assert!(matches!(
        graph.projection_chart_with_budget(0, budget.clone(), || true),
        Err(SurfaceError::Cancelled)
    ));
    let tiny = MemoryBudget::new(1);
    assert!(matches!(
        graph.projection_chart_with_budget(0, tiny.clone(), || false),
        Err(SurfaceError::Memory)
    ));
    assert_eq!(tiny.used(), 0);
    assert_eq!(budget.used(), baseline);
    let calls = Cell::new(0);
    let chart = graph
        .projection_chart_with_budget(0, budget.clone(), || {
            calls.set(calls.get() + 1);
            false
        })
        .unwrap();
    let total = calls.get();
    drop(chart);
    for at in [1, 2, total / 2, total] {
        let count = Cell::new(0);
        assert!(matches!(
            graph.projection_chart_with_budget(0, budget.clone(), || {
                count.set(count.get() + 1);
                count.get() == at
            }),
            Err(SurfaceError::Cancelled)
        ));
        assert_eq!(budget.used(), baseline);
    }
}
