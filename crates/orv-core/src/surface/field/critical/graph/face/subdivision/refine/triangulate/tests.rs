// SPDX-License-Identifier: GPL-3.0-or-later
use super::super::super::super::super::tests::{coord, positive_root, rational};
use super::*;
use std::{
    cell::Cell,
    collections::{BTreeMap, BTreeSet},
};

fn build(values: [i16; 4], bits: u8) -> Arc<RefinedSubdivision> {
    Arc::new(
        Bilinear::new(values)
            .unwrap()
            .subdivision(&[], &[], || false)
            .unwrap()
            .refine(
                RefinementLimits {
                    precision_bits: bits,
                    ..Default::default()
                },
                || false,
            )
            .unwrap(),
    )
}
fn verify(mesh: &TriangulatedSubdivision) {
    let input = mesh.subdivision();
    let mut edges = BTreeMap::<(u32, u32), Vec<(i8, u8)>>::new();
    let mut used = BTreeSet::new();
    let mut area = 0.;
    for triangle in mesh.triangles() {
        assert!(input.base.regions().any(|(id, _)| id == triangle.region));
        let [a, b, c] = triangle.vertices.map(|id| {
            used.insert(id);
            input.vertex(id).unwrap().map(|v| v.approximate().unwrap())
        });
        let cross = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
        assert!(cross >= -1e-15);
        area += cross / 2.;
        for i in 0..3 {
            let (a, b) = (triangle.vertices[i], triangle.vertices[(i + 1) % 3]);
            assert_ne!(a, b);
            let key = (a.min(b), a.max(b));
            edges
                .entry(key)
                .or_default()
                .push((if a < b { 1 } else { -1 }, triangle.region));
        }
    }
    assert!((area - 1.).abs() < 2e-12, "area={area}");
    assert_eq!(used.len(), input.vertices.len());
    let mut boundary_count = 0;
    let mut constraints = BTreeSet::new();
    for (id, edge) in input.base.edges() {
        for pair in input.edge_vertices(id).unwrap().windows(2) {
            let key = (pair[0].min(pair[1]), pair[0].max(pair[1]));
            let occurrences = edges.get(&key).unwrap();
            constraints.insert(key);
            if edge.boundary.is_some() {
                assert_eq!(occurrences.len(), 1);
                boundary_count += 1;
            } else {
                assert_eq!(occurrences.len(), 2);
                assert_ne!(occurrences[0].1, occurrences[1].1);
            }
        }
    }
    for (key, occurrences) in edges {
        assert!((1..=2).contains(&occurrences.len()));
        if occurrences.len() == 2 {
            assert_eq!(
                occurrences.iter().map(|(s, _)| i32::from(*s)).sum::<i32>(),
                0
            );
            if !constraints.contains(&key) {
                assert_eq!(occurrences[0].1, occurrences[1].1);
            }
        } else {
            assert!(constraints.contains(&key));
        }
    }
    assert_eq!(
        mesh.triangles.len(),
        2 * input.vertices.len() - boundary_count - 2
    );
}
#[test]
fn generated_monotone_regions_cover_the_square_and_preserve_every_constraint() {
    let budget = MemoryBudget::new(32 * 1024 * 1024);
    let mut state = 137u64;
    for case in 0..593 {
        let values = if case < 81 {
            let mut n = case;
            std::array::from_fn(|_| {
                let v = (n % 3) as i16 - 1;
                n /= 3;
                v
            })
        } else {
            std::array::from_fn(|_| {
                state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
                (state % 511) as i16 - 255
            })
        };
        let input = build(values, 6);
        let mesh = input
            .triangulate_with_budget(Default::default(), budget.clone(), || false)
            .unwrap_or_else(|error| panic!("{values:?}: {error:?}"));
        verify(&mesh);
        assert!(Arc::ptr_eq(mesh.subdivision(), &input));
        assert_eq!(budget.used(), mesh.referenced_bytes());
        drop(mesh);
        assert_eq!(budget.used(), 0);
    }
}
#[test]
fn triangulation_is_deterministic_under_complement_and_input_order() {
    let root = positive_root(2, 0, -1);
    let other = positive_root(3, 0, -1);
    let points = [root, other].map(|r| [coord(r, [0, 1], [1, 0]), coord(r, [1, 0], [2, 1])]);
    let boundary = [
        [coord(root, [0, 1], [1, 0]), rational(0, 1)],
        [rational(1, 1), coord(other, [0, 1], [1, 0])],
    ];
    let make = |values, b: &[[ExactCoordinate; 2]], p: &[[ExactCoordinate; 2]]| {
        let base = Bilinear::new(values)
            .unwrap()
            .subdivision(b, p, || false)
            .unwrap();
        Arc::new(base.refine(RefinementLimits::default(), || false).unwrap())
            .triangulate(Default::default(), || false)
            .unwrap()
    };
    let a = make([-1, -1, 2, 1], &boundary, &points);
    let b = make(
        [1, 1, -2, -1],
        &[boundary[1], boundary[0]],
        &[points[1], points[0]],
    );
    verify(&a);
    verify(&b);
    assert_eq!(a.triangles, b.triangles);
}
#[test]
fn whole_zero_isolated_points_include_unrelated_fields_and_exact_diagonal_splits() {
    let root = |n| coord(positive_root(n, 0, -1), [0, 1], [1, 0]);
    let points = [
        [rational(1, 2); 2],
        [root(2), root(3)],
        [root(5), root(7)],
        [root(11), root(13)],
    ];
    let base = Bilinear::new([0; 4])
        .unwrap()
        .subdivision(&[], &points, || false)
        .unwrap();
    let input = Arc::new(base.refine(Default::default(), || false).unwrap());
    let mesh = input.triangulate(Default::default(), || false).unwrap();
    verify(&mesh);
    assert_eq!(mesh.triangles.len(), 10);
}
#[test]
fn rounded_equal_interior_points_remain_distinct_nonzero_triangles() {
    let n = (1 << 20) - 3;
    let x = coord(positive_root(n, n, -1), [1, 1], [2, 0]);
    let y = coord(positive_root(n + 2, n, -1), [1, 1], [2, 0]);
    assert_eq!(x.approximate(), y.approximate());
    let points = [[x, rational(1, 2)], [y, rational(1, 2)]];
    let base = Bilinear::new([0; 4])
        .unwrap()
        .subdivision(&[], &points, || false)
        .unwrap();
    let input = Arc::new(base.refine(Default::default(), || false).unwrap());
    let mesh = input.triangulate(Default::default(), || false).unwrap();
    verify(&mesh);
    let budget = MemoryBudget::new(2 * 1024 * 1024);
    for triangle in &mesh.triangles {
        assert_eq!(
            orientation(
                triangle.vertices.map(|id| input.vertices[id as usize]),
                &budget,
                &|| false
            ),
            Ok(1)
        );
    }
}
#[test]
fn maximum_refinement_detail_has_linear_triangle_count() {
    let input = build([-1, -1, 254, -1], MAX_BITS);
    let mesh = input.triangulate(Default::default(), || false).unwrap();
    verify(&mesh);
    assert!(mesh.triangles.len() < 2 * input.vertices.len());
}

#[test]
fn full_boundary_and_interior_constraint_limits_keep_all_original_vertices() {
    let mut boundary = Vec::new();
    for side in 0..4 {
        for k in 1..=20 {
            let t = rational(k, 21);
            boundary.push(match side {
                0 => [t, rational(0, 1)],
                1 => [rational(1, 1), t],
                2 => [t, rational(1, 1)],
                _ => [rational(0, 1), t],
            });
        }
    }
    let base = Bilinear::new([1, -1, 1, -1])
        .unwrap()
        .subdivision(&boundary, &[], || false)
        .unwrap();
    assert_eq!(base.nodes().count(), NODES);
    let input = Arc::new(base.refine(Default::default(), || false).unwrap());
    verify(&input.triangulate(Default::default(), || false).unwrap());
    let mut isolated = Vec::new();
    for y in 1..=8 {
        for x in 1..=10 {
            isolated.push([rational(x, 11), rational(y, 9)]);
        }
    }
    let base = Bilinear::new([0; 4])
        .unwrap()
        .subdivision(&[], &isolated, || false)
        .unwrap();
    let input = Arc::new(base.refine(Default::default(), || false).unwrap());
    let mesh = input.triangulate(Default::default(), || false).unwrap();
    verify(&mesh);
    assert_eq!(mesh.triangles.len(), 162);
}

#[test]
fn late_exact_predicate_pressure_releases_unpublished_mesh_and_keeps_prior_owner() {
    let n = (1 << 20) - 3;
    let points = [n, n + 2].map(|a| {
        [
            coord(positive_root(a, n, -1), [1, 1], [2, 0]),
            rational(1, 2),
        ]
    });
    let base = Bilinear::new([0; 4])
        .unwrap()
        .subdivision(&[], &points, || false)
        .unwrap();
    let input = Arc::new(base.refine(Default::default(), || false).unwrap());
    let budget = MemoryBudget::new(32 * 1024 * 1024);
    let prior = input
        .triangulate_with_budget(Default::default(), budget.clone(), || false)
        .unwrap();
    let baseline = budget.used();
    let scratch = input.vertices.len()
        * (std::mem::size_of::<Bounds>()
            + std::mem::size_of::<u32>()
            + 2 * std::mem::size_of::<SweepVertex>())
        + NODES;
    budget.set_limit(baseline + scratch as u64 + prior.referenced_bytes());
    assert!(matches!(
        input.triangulate_with_budget(Default::default(), budget.clone(), || false),
        Err(SurfaceError::Memory)
    ));
    assert_eq!(budget.used(), baseline);
    assert_eq!(Arc::strong_count(&input), 2);
    verify(&prior);
    drop(prior);
    assert_eq!(budget.used(), 0);
}
#[test]
fn limits_pressure_and_every_cancellation_preserve_the_input_and_prior_mesh() {
    let budget = MemoryBudget::new(32 * 1024 * 1024);
    let input = build([-1, -1, 2, 1], 1);
    let calls = Cell::new(0usize);
    let prior = input
        .triangulate_with_budget(Default::default(), budget.clone(), || {
            calls.set(calls.get() + 1);
            false
        })
        .unwrap();
    let count = calls.get();
    let baseline = budget.used();
    let original = prior.triangles.clone();
    for stop in 1..=count {
        calls.set(0);
        assert!(matches!(
            input.triangulate_with_budget(Default::default(), budget.clone(), || {
                calls.set(calls.get() + 1);
                calls.get() == stop
            }),
            Err(SurfaceError::Cancelled)
        ));
        assert_eq!(budget.used(), baseline);
        assert_eq!(prior.triangles, original);
        assert_eq!(Arc::strong_count(&input), 2);
    }
    for limits in [
        TriangulationLimits {
            max_triangles: 1,
            ..Default::default()
        },
        TriangulationLimits {
            max_bytes: 1,
            ..Default::default()
        },
        TriangulationLimits {
            max_triangles: MAX_TRIANGLES + 1,
            ..Default::default()
        },
        TriangulationLimits {
            max_bytes: MAX_TRIANGLE_BYTES + 1,
            ..Default::default()
        },
    ] {
        assert!(matches!(
            input.triangulate_with_budget(limits, budget.clone(), || false),
            Err(SurfaceError::Limit)
        ));
        assert_eq!(budget.used(), baseline);
    }
    assert!(matches!(
        input.triangulate_with_budget(Default::default(), MemoryBudget::new(0), || false),
        Err(SurfaceError::Memory)
    ));
    budget.set_limit(baseline + prior.referenced_bytes());
    assert!(matches!(
        input.triangulate_with_budget(Default::default(), budget.clone(), || false),
        Err(SurfaceError::Memory)
    ));
    assert_eq!(budget.used(), baseline);
    drop(prior);
    assert_eq!(budget.used(), 0);
}
