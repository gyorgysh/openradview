// SPDX-License-Identifier: GPL-3.0-or-later
use super::super::super::super::super::tests::{coord, positive_root, rational};
use super::super::super::super::subdivision::refine::triangulate::scratch_bytes;
use super::*;
use crate::surface::field::critical::graph::compare::orientation;
use std::{
    cell::Cell,
    collections::{BTreeMap, BTreeSet},
};

fn build(
    values: [[i16; 4]; 2],
    points: &[[ExactCoordinate; 2]],
    bits: u8,
) -> Arc<RefinedJointSubdivision> {
    let [a, b] = values.map(|v| Bilinear::new(v).unwrap());
    Arc::new(
        a.joint_subdivision(b, points, || false)
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
fn verify(mesh: &TriangulatedJointSubdivision) {
    let input = mesh.subdivision();
    let mut used = BTreeSet::new();
    let mut edges = BTreeMap::<(u32, u32), Vec<(i8, u8)>>::new();
    let mut area = 0.;
    let mut region_area = [0.; 9];
    for triangle in mesh.triangles() {
        assert!(input.base.regions().any(|(id, _)| id == triangle.region));
        let [a, b, c] = triangle.vertices.map(|id| {
            used.insert(id);
            input.vertex(id).unwrap().map(|v| v.approximate().unwrap())
        });
        let cross = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
        assert!(cross >= -1e-15);
        area += cross / 2.;
        region_area[usize::from(triangle.region)] += cross / 2.;
        for i in 0..3 {
            let (a, b) = (triangle.vertices[i], triangle.vertices[(i + 1) % 3]);
            assert_ne!(a, b);
            edges
                .entry((a.min(b), a.max(b)))
                .or_default()
                .push((if a < b { 1 } else { -1 }, triangle.region));
        }
    }
    assert!((area - 1.).abs() < 1e-10, "Area: {area}");
    for (id, _) in input.base.regions() {
        assert!(region_area[usize::from(id)] > 0.);
    }
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
    for (id, _) in input.base.nodes() {
        if let Some(region) = input.base.interior_region(id) {
            let incident: Vec<_> = mesh
                .triangles
                .iter()
                .filter(|t| t.vertices.contains(&u32::from(id)))
                .collect();
            assert!(incident.len() >= 3);
            assert!(incident.iter().all(|t| t.region == region));
        }
    }
    assert_eq!(
        mesh.triangles.len(),
        2 * input.vertices.len() - boundary_count - 2
    );
    assert_eq!(
        mesh.referenced_bytes(),
        (std::mem::size_of::<TriangulatedJointSubdivision>()
            + mesh.triangles.len() * std::mem::size_of::<RegionTriangle>()) as u64
    );
}
fn coverage(mesh: &TriangulatedJointSubdivision) -> usize {
    let input = mesh.subdivision();
    let triangles: Vec<_> = mesh
        .triangles
        .iter()
        .map(|t| {
            (
                t.region,
                t.vertices
                    .map(|id| input.vertex(id).unwrap().map(|v| v.approximate().unwrap())),
            )
        })
        .collect();
    let tolerance = input.maximum_coordinate_error().as_f64();
    let mut checked = 0;
    for i in 0..12 {
        for j in 0..12 {
            let p = [(f64::from(i) + 0.317) / 12., (f64::from(j) + 0.713) / 12.];
            let [u, v] = p;
            let mut expected = [0i8; 2];
            let mut separated = true;
            for (index, field) in input.base.fields.iter().enumerate() {
                let [a, b, c, d] = field.values.map(f64::from);
                let f = a * (1. - u) * (1. - v) + b * u * (1. - v) + c * u * v + d * (1. - u) * v;
                expected[index] = if f > 0. {
                    1
                } else if f < 0. {
                    -1
                } else {
                    0
                };
                let gradient = (b - a).abs().max((c - d).abs()) + (d - a).abs().max((c - b).abs());
                if field.values != [0; 4] && f.abs() <= gradient * tolerance + 1e-9 {
                    separated = false;
                }
            }
            if !separated {
                continue;
            }
            let mut found = BTreeSet::new();
            for (region, [a, b, c]) in &triangles {
                let cross = |a: [f64; 2], b: [f64; 2]| {
                    (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0])
                };
                if [cross(*a, *b), cross(*b, *c), cross(*c, *a)]
                    .into_iter()
                    .all(|v| v >= -1e-14)
                {
                    found.insert(*region);
                }
            }
            assert_eq!(found.len(), 1);
            let region = *found.first().unwrap();
            assert_eq!(
                input.base.regions[usize::from(region)].unwrap().signs,
                expected
            );
            checked += 1;
        }
    }
    checked
}

#[test]
fn every_ternary_pair_has_conforming_triangles_with_original_region_ids() {
    let fields: Vec<_> = (0..81)
        .map(|mut n| {
            std::array::from_fn(|_| {
                let v = (n % 3) as i16 - 1;
                n /= 3;
                v
            })
        })
        .collect();
    for a in &fields {
        for b in &fields {
            let input = build([*a, *b], &[], 0);
            let mesh = input
                .triangulate(Default::default(), || false)
                .unwrap_or_else(|e| panic!("{a:?}/{b:?}: {e:?}"));
            verify(&mesh);
            coverage(&mesh);
            assert!(Arc::ptr_eq(mesh.subdivision(), &input));
        }
    }
}
#[test]
fn generated_joint_triangles_preserve_constraints_and_independent_field_coverage() {
    let points: Vec<_> = (1..4)
        .flat_map(|u| (1..4).map(move |v| [rational(u, 4), rational(v, 4)]))
        .collect();
    let mut state = 317u64;
    let mut checked = 0;
    for case in 0..512 {
        let values = std::array::from_fn(|slot| {
            std::array::from_fn(|corner| {
                if case < 256 {
                    return if case & (1 << (4 * slot + corner)) == 0 {
                        -255
                    } else {
                        255
                    };
                }
                state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
                (state % 511) as i16 - 255
            })
        });
        let input = build(values, &points, 5);
        let mesh = input
            .triangulate(Default::default(), || false)
            .unwrap_or_else(|e| panic!("{values:?}: {e:?}"));
        verify(&mesh);
        checked += coverage(&mesh);
    }
    assert!(checked > 10000);
}
#[test]
fn lenses_tangencies_crosses_and_maximum_constraints_keep_nonempty_region_meshes() {
    for values in [
        [[0, -1, 0, 1], [3, -29, 3, 19]],
        [[0, -1, 0, 1], [0, -1, 0, 2]],
        [[0, -1, 0, 1], [1, -7, 1, 5]],
        [[1, -2, 4, -2], [4, -2, 1, -2]],
    ] {
        let input = build(values, &[], 0);
        let mesh = input.triangulate(Default::default(), || false).unwrap();
        verify(&mesh);
        coverage(&mesh);
    }
    let points: Vec<_> = (1..=8)
        .flat_map(|v| (1..=10).map(move |u| [rational(u, 11), rational(v, 9)]))
        .collect();
    let input = build([[1, -2, 4, -2], [4, -2, 1, -2]], &points, 8);
    let mesh = input.triangulate(Default::default(), || false).unwrap();
    verify(&mesh);
    coverage(&mesh);
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
    let input = build([[-1, -1, 2, 1], [3, -29, 3, 19]], &boundary, 6);
    let mesh = input.triangulate(Default::default(), || false).unwrap();
    verify(&mesh);
    coverage(&mesh);
}
#[test]
fn exact_turns_keep_unrelated_irrational_and_rounded_equal_original_constraints() {
    let n = (1 << 20) - 3;
    let a = coord(positive_root(n, n, -1), [1, 1], [2, 0]);
    let b = coord(positive_root(n + 2, n, -1), [1, 1], [2, 0]);
    assert_eq!(a.approximate(), b.approximate());
    let root = |n| coord(positive_root(n, 0, -1), [0, 1], [1, 0]);
    let points = [
        [a, rational(1, 2)],
        [b, rational(1, 2)],
        [root(2), root(3)],
        [root(5), root(7)],
        [root(11), root(13)],
    ];
    let input = build([[0; 4], [1; 4]], &points, 8);
    let mesh = input.triangulate(Default::default(), || false).unwrap();
    verify(&mesh);
    let budget = MemoryBudget::new(2 * 1024 * 1024);
    for triangle in mesh.triangles() {
        assert_eq!(
            orientation(
                triangle.vertices.map(|id| *input.vertex(id).unwrap()),
                &budget,
                &|| false
            ),
            Ok(1)
        );
    }
    assert_eq!(budget.used(), 0);
}
#[test]
fn field_sign_order_and_insertion_order_leave_geometry_deterministic() {
    let values = [[-1, -1, 2, 1], [3, -29, 3, 19]];
    let points = [
        [rational(3, 7), rational(4, 9)],
        [rational(1, 2), rational(3, 4)],
    ];
    let input = build(values, &points, 8);
    let first = input.triangulate(Default::default(), || false).unwrap();
    for (values, swapped) in [
        ([values[1], values[0]], true),
        ([values[0].map(|v| -v), values[1]], false),
    ] {
        let other = build(values, &[points[1], points[0]], 8);
        let second = other.triangulate(Default::default(), || false).unwrap();
        verify(&second);
        assert_eq!(first.triangles, second.triangles);
        for (a, b) in first.input.vertices.iter().zip(&second.input.vertices) {
            assert!(same_point(*a, *b).unwrap());
        }
        if swapped {
            for ((_, a), (_, b)) in first.input.base.regions().zip(second.input.base.regions()) {
                assert_eq!(a.signs, [b.signs[1], b.signs[0]]);
            }
        }
    }
}
#[test]
fn maximum_refinement_detail_keeps_linear_triangle_count_and_shared_input() {
    let input = build([[-1, -1, 254, -1], [254, -1, -1, -1]], &[], MAX_BITS);
    let mesh = input.triangulate(Default::default(), || false).unwrap();
    verify(&mesh);
    assert!(Arc::ptr_eq(mesh.subdivision(), &input));
}
#[test]
fn exact_limits_predicate_pressure_and_every_cancellation_keep_prior_mesh() {
    let points = [[rational(1, 2), rational(3, 4)]];
    let input = build([[-1, -1, 2, 1], [0, -1, 0, 1]], &points, 1);
    let budget = MemoryBudget::new(32 * 1024 * 1024);
    let calls = Cell::new(0);
    let prior = input
        .triangulate_with_budget(Default::default(), budget.clone(), || {
            calls.set(calls.get() + 1);
            false
        })
        .unwrap();
    let original = prior.triangles.clone();
    let baseline = budget.used();
    let count = calls.get();
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
    let exact = TriangulationLimits {
        max_triangles: prior.triangles.len(),
        max_bytes: prior.bytes,
    };
    let second = input
        .triangulate_with_budget(exact, budget.clone(), || false)
        .unwrap();
    assert_eq!(second.triangles, original);
    drop(second);
    for limits in [
        TriangulationLimits {
            max_triangles: exact.max_triangles - 1,
            ..exact
        },
        TriangulationLimits {
            max_bytes: exact.max_bytes - 1,
            ..exact
        },
        TriangulationLimits {
            max_triangles: 2 * MAX_VERTICES + 1,
            ..exact
        },
        TriangulationLimits {
            max_bytes: MAX_BYTES + 1,
            ..exact
        },
    ] {
        assert!(matches!(
            input.triangulate_with_budget(limits, budget.clone(), || false),
            Err(SurfaceError::Limit)
        ));
        assert_eq!(budget.used(), baseline);
    }
    assert!(matches!(
        input.triangulate_with_budget(exact, MemoryBudget::new(0), || false),
        Err(SurfaceError::Memory)
    ));
    budget.set_limit(baseline + scratch_bytes::<JOINT_NODES>(input.vertices.len()).unwrap());
    assert!(matches!(
        input.triangulate_with_budget(exact, budget.clone(), || false),
        Err(SurfaceError::Memory)
    ));
    assert_eq!(budget.used(), baseline);
    verify(&prior);
    drop(prior);
    assert_eq!(budget.used(), 0);
}
#[test]
fn incorrect_isolated_region_or_exact_predicate_pressure_cannot_publish_mesh() {
    let budget = MemoryBudget::new(32 * 1024 * 1024);
    let mut bad = build([[1, -2, 4, -2], [4, -2, 1, -2]], &[[rational(1, 2); 2]], 0);
    let id = bad
        .base
        .nodes()
        .find(|(id, _)| bad.base.interior_region(*id).is_some())
        .unwrap()
        .0;
    let region = bad.base.interior_region(id).unwrap();
    let wrong = bad.base.regions().find(|(i, _)| *i != region).unwrap().0;
    Arc::get_mut(&mut bad).unwrap().base.interior[usize::from(id)] = Some(wrong);
    assert!(matches!(
        bad.triangulate_with_budget(Default::default(), budget.clone(), || false),
        Err(SurfaceError::Geometry)
    ));
    assert_eq!(budget.used(), 0);
    assert_eq!(Arc::strong_count(&bad), 1);
    let n = (1 << 20) - 3;
    let points = [n, n + 2].map(|a| {
        [
            coord(positive_root(a, n, -1), [1, 1], [2, 0]),
            rational(1, 2),
        ]
    });
    let input = build([[0; 4], [1; 4]], &points, 0);
    let prior = input
        .triangulate_with_budget(Default::default(), budget.clone(), || false)
        .unwrap();
    let baseline = budget.used();
    budget.set_limit(
        baseline + prior.bytes + scratch_bytes::<JOINT_NODES>(input.vertices.len()).unwrap(),
    );
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
