// SPDX-License-Identifier: GPL-3.0-or-later
use super::super::super::super::tests::{coord, positive_root};
use super::super::super::subdivision::refine::tests::{fraction, within};
use super::*;
use std::{cell::Cell, collections::BTreeSet};

fn rational(n: i128, d: i128) -> ExactCoordinate {
    rational_coordinate(Rational::new(n, d).unwrap()).unwrap()
}
fn build(values: [[i16; 4]; 2], points: &[[ExactCoordinate; 2]]) -> JointSubdivision {
    let [a, b] = values.map(|v| Bilinear::new(v).unwrap());
    a.joint_subdivision(b, points, || false).unwrap()
}
fn limits(bits: u8) -> RefinementLimits {
    RefinementLimits {
        precision_bits: bits,
        ..Default::default()
    }
}
fn verify(refined: &RefinedJointSubdivision, base: &JointSubdivision) {
    assert_eq!(
        refined.subdivision().half_edges().collect::<Vec<_>>(),
        base.half_edges().collect::<Vec<_>>()
    );
    assert_eq!(
        refined.subdivision().regions().collect::<Vec<_>>(),
        base.regions().collect::<Vec<_>>()
    );
    for (id, node) in base.nodes() {
        assert_eq!(
            refined.vertex(id.into()).unwrap().map(representation),
            node.coordinates.map(representation)
        );
        assert!(
            refined
                .cuts
                .iter()
                .any(|u| u.compare(node.coordinates[0]).unwrap() == Ordering::Equal)
        );
    }
    for pair in refined.cuts.windows(2) {
        assert_eq!(pair[0].compare(pair[1]), Ok(Ordering::Less));
    }
    let mut used = BTreeSet::new();
    for (id, edge) in base.edges() {
        let path = refined.edge_vertices(id).unwrap();
        assert_eq!(path.first(), Some(&u32::from(edge.from)));
        assert_eq!(path.last(), Some(&u32::from(edge.to)));
        let a = base.node(edge.from).unwrap().coordinates;
        let b = base.node(edge.to).unwrap().coordinates;
        if a[0].compare(b[0]).unwrap() == Ordering::Equal {
            assert_eq!(path.len(), 2);
        } else {
            let cuts: Vec<_> = refined
                .cuts
                .iter()
                .filter(|u| {
                    u.compare(a[0]).unwrap() != Ordering::Less
                        && u.compare(b[0]).unwrap() != Ordering::Greater
                })
                .collect();
            assert_eq!(path.len(), cuts.len());
            for (vertex, u) in path.iter().zip(cuts) {
                assert_eq!(
                    refined.vertex(*vertex).unwrap()[0].compare(*u),
                    Ok(Ordering::Equal)
                );
            }
        }
        for vertex in path {
            let point = *refined.vertex(*vertex).unwrap();
            if *vertex >= base.count as u32 {
                assert!(used.insert(*vertex));
            }
            for field in 0..2 {
                if edge.curves & (1 << field) != 0 {
                    assert_eq!(base.fields[field].sign_at(point), Ok(0));
                }
            }
        }
    }
    assert_eq!(used.len() + base.count, refined.vertices.len());
    // Every original isolated constraint remains inside the same polygonal
    // region: its own slice has exact boundary vertices on both sides.
    for (id, node) in base.nodes() {
        let Some(region) = base.interior_region(id) else {
            continue;
        };
        let mut boundaries = 0;
        for (_, h) in base.half_edges().filter(|(_, h)| h.region == Some(region)) {
            let a = base.node(h.from).unwrap().coordinates;
            let b = base.node(h.to).unwrap().coordinates;
            let du = a[0].compare(b[0]).unwrap();
            if du == Ordering::Equal {
                continue;
            }
            let lo = a[0].compare(node.coordinates[0]).unwrap();
            let hi = b[0].compare(node.coordinates[0]).unwrap();
            if !((lo != Ordering::Greater && hi != Ordering::Less)
                || (hi != Ordering::Greater && lo != Ordering::Less))
            {
                continue;
            }
            let p = refined
                .edge_vertices(h.edge)
                .unwrap()
                .iter()
                .map(|v| refined.vertex(*v).unwrap())
                .find(|p| p[0].compare(node.coordinates[0]).unwrap() == Ordering::Equal)
                .unwrap();
            assert_eq!(node.coordinates[1].compare(p[1]).unwrap(), du.reverse());
            boundaries += 1;
        }
        assert!(boundaries >= 2);
    }
    assert_eq!(
        refined.referenced_bytes(),
        bytes(
            refined.vertices.len(),
            refined.indices.len(),
            refined.cuts.len()
        )
        .unwrap()
    );
}
fn exact_error(refined: &RefinedJointSubdivision) {
    for (id, edge) in refined.base.edges() {
        if !matches!(edge.geometry, CommonZero::Curve(_)) {
            continue;
        }
        let [a, b, c, d] = coefficients(refined.base.curve_field(*edge).unwrap()).map(i128::from);
        if d == 0 {
            continue;
        }
        let path = refined.edge_vertices(id).unwrap();
        for pair in path.windows(2) {
            let raw = |id| {
                refined.vertex(id).unwrap().map(|v| {
                    v.rational_value()
                        .map(|r| (i128::from(r.numerator), i128::from(r.denominator)))
                })
            };
            let p = raw(pair[0]);
            let q = raw(pair[1]);
            let ([Some(pu), Some(pv)], [Some(qu), Some(qv)]) = (p, q) else {
                continue;
            };
            let p = [pu, pv];
            let q = [qu, qv];
            let (mut vertical, mut horizontal) = (true, true);
            for k in 1..16 {
                let chord: [(i128, i128); 2] = std::array::from_fn(|i| {
                    (
                        (16 - k) * p[i].0 * q[i].1 + k * q[i].0 * p[i].1,
                        16 * p[i].1 * q[i].1,
                    )
                });
                let [(u, uq), (v, vq)] = chord;
                vertical &= within(
                    chord[1],
                    fraction(-(a * uq + b * u), c * uq + d * u),
                    refined.bits,
                );
                horizontal &= within(
                    chord[0],
                    fraction(-(a * vq + c * v), b * vq + d * v),
                    refined.bits,
                );
            }
            assert!(
                vertical || horizontal,
                "Independent error samples exceed requested detail"
            );
        }
    }
}
fn same(a: &RefinedJointSubdivision, b: &RefinedJointSubdivision) {
    assert_eq!(a.indices, b.indices);
    assert_eq!(a.vertices.len(), b.vertices.len());
    assert_eq!(a.cuts.len(), b.cuts.len());
    for (a, b) in a.vertices.iter().zip(&b.vertices) {
        assert!(same_point(*a, *b).unwrap());
    }
    for (a, b) in a.cuts.iter().zip(&b.cuts) {
        assert_eq!(a.compare(*b), Ok(Ordering::Equal));
    }
}

fn verify_polygon_coverage(refined: &RefinedJointSubdivision) -> usize {
    let mut polygons = Vec::new();
    let mut total_area = 0.;
    for (_, region) in refined.base.regions() {
        let mut polygon = Vec::new();
        let mut half = region.first;
        for _ in 0..region.edge_count {
            let h = refined.base.half_edge(half).unwrap();
            let edge = refined.base.edge(h.edge).unwrap();
            let path = refined.edge_vertices(h.edge).unwrap();
            let forward = edge.from == h.from;
            for i in 0..path.len() - 1 {
                let index = if forward { i } else { path.len() - 1 - i };
                polygon.push(
                    refined
                        .vertex(path[index])
                        .unwrap()
                        .map(|v| v.approximate().unwrap()),
                );
            }
            half = h.next;
        }
        assert_eq!(half, region.first);
        assert!(polygon.len() >= 3);
        let anchor = polygon[0];
        let area = polygon
            .iter()
            .zip(polygon.iter().cycle().skip(1))
            .map(|(a, b)| {
                (a[0] - anchor[0]) * (b[1] - anchor[1]) - (a[1] - anchor[1]) * (b[0] - anchor[0])
            })
            .sum::<f64>()
            * 0.5;
        assert!(
            area > 0.,
            "A nonempty region must retain positive polygonal area"
        );
        total_area += area;
        polygons.push((region.signs, polygon));
    }
    assert!((total_area - 1.).abs() < 1e-10);
    let tolerance = refined.maximum_coordinate_error().as_f64();
    let mut checked = 0;
    for i in 0..12 {
        for j in 0..12 {
            let [u, v] = [(f64::from(i) + 0.317) / 12., (f64::from(j) + 0.713) / 12.];
            let mut expected = [0i8; 2];
            let mut separated = true;
            for (index, field) in refined.base.fields.iter().enumerate() {
                let [a, b, c, d] = field.values.map(f64::from);
                let value =
                    a * (1. - u) * (1. - v) + b * u * (1. - v) + c * u * v + d * (1. - u) * v;
                expected[index] = if value > 0. {
                    1
                } else if value < 0. {
                    -1
                } else {
                    0
                };
                // The maximum Bernstein edge differences bound the field's
                // L-infinity Lipschitz constant. Sample outside the allowed
                // displacement band, where chord approximation cannot change
                // the original region. Whole-zero fields impose no boundary.
                let gradient = (b - a).abs().max((c - d).abs()) + (d - a).abs().max((c - b).abs());
                if field.values != [0; 4] && value.abs() <= gradient * tolerance + 1e-9 {
                    separated = false;
                }
            }
            if !separated {
                continue;
            }
            let mut found = Vec::new();
            for (signs, polygon) in &polygons {
                let mut inside = false;
                for (a, b) in polygon.iter().zip(polygon.iter().cycle().skip(1)) {
                    if (a[1] > v) != (b[1] > v)
                        && u < (b[0] - a[0]) * (v - a[1]) / (b[1] - a[1]) + a[0]
                    {
                        inside = !inside;
                    }
                }
                if inside {
                    found.push(*signs);
                }
            }
            assert_eq!(
                found,
                vec![expected],
                "Original field signs and completed polygon coverage disagree"
            );
            checked += 1;
        }
    }
    checked
}

#[test]
fn common_slices_preserve_lenses_tangencies_axes_and_whole_fields_even_at_coarse_detail() {
    for fields in [
        [[0, -1, 0, 1], [3, -29, 3, 19]],
        [[0, -1, 0, 1], [0, -1, 0, 2]],
        [[0, -1, 0, 1], [1, -7, 1, 5]],
        [[1, -2, 4, -2], [4, -2, 1, -2]],
        [[0; 4], [0; 4]],
        [[1; 4], [-1; 4]],
    ] {
        let points: Vec<_> = (1..4)
            .flat_map(|u| (1..4).map(move |v| [rational(u, 4), rational(v, 4)]))
            .collect();
        let base = build(fields, &points);
        for bits in [0, 6] {
            let r = base.refine(limits(bits), || false).unwrap();
            verify(&r, &base);
            exact_error(&r);
            verify_polygon_coverage(&r);
        }
    }
    let base = build([[0, -1, 0, 1], [3, -29, 3, 19]], &[]);
    let r = base.refine(limits(0), || false).unwrap();
    verify(&r, &base);
    let mut checked = false;
    for (i, a) in base.edges() {
        for (j, b) in base.edges() {
            if i < j && (a.from, a.to) == (b.from, b.to) {
                let p = r.edge_vertices(i).unwrap();
                let q = r.edge_vertices(j).unwrap();
                assert_eq!(p.len(), q.len());
                assert!(p.len() > 2);
                assert!(
                    p[1..p.len() - 1]
                        .iter()
                        .zip(&q[1..q.len() - 1])
                        .any(
                            |(p, q)| !same_point(*r.vertex(*p).unwrap(), *r.vertex(*q).unwrap())
                                .unwrap()
                        )
                );
                checked = true;
            }
        }
    }
    assert!(checked);
}

#[test]
fn all_ternary_field_pairs_keep_positive_regions_and_complete_common_paths() {
    let fields: Vec<_> = (0..81)
        .map(|mut code| {
            std::array::from_fn(|_| {
                let value = (code % 3) as i16 - 1;
                code /= 3;
                value
            })
        })
        .collect();
    for a in &fields {
        for b in &fields {
            let base = build([*a, *b], &[]);
            let r = base
                .refine(limits(0), || false)
                .unwrap_or_else(|e| panic!("{a:?}/{b:?}: {e:?}"));
            verify(&r, &base);
            verify_polygon_coverage(&r);
        }
    }
}

#[test]
fn generated_joint_paths_retain_all_constraints_and_exact_shared_partitions() {
    let points: Vec<_> = (1..4)
        .flat_map(|u| (1..4).map(move |v| [rational(u, 4), rational(v, 4)]))
        .collect();
    let mut state = 717u64;
    let mut samples = 0;
    for case in 0..384 {
        let fields = std::array::from_fn(|slot| {
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
        let base = build(fields, &points);
        let r = base
            .refine(limits(5), || false)
            .unwrap_or_else(|e| panic!("{fields:?}: {e:?}"));
        verify(&r, &base);
        exact_error(&r);
        samples += verify_polygon_coverage(&r);
    }
    assert!(samples > 10000);
}

#[test]
fn full_constraint_capacity_and_exact_storage_limits_preserve_all_region_paths() {
    let points: Vec<_> = (1..=80)
        .map(|i| [rational(i, 83), rational(2, 5)])
        .collect();
    let base = build([[1, -2, 4, -2], [4, -2, 1, -2]], &points);
    let budget = MemoryBudget::new(MAX_BYTES);
    let r = base
        .refine_with_budget(limits(24), budget.clone(), || false)
        .unwrap();
    verify(&r, &base);
    verify_polygon_coverage(&r);
    let request = RefinementLimits {
        precision_bits: 24,
        max_vertices: r.vertices.len(),
        max_bytes: r.referenced_bytes(),
    };
    let other = base
        .refine_with_budget(request, budget.clone(), || false)
        .unwrap();
    same(&r, &other);
    let baseline = budget.used();
    for request in [
        RefinementLimits {
            max_vertices: request.max_vertices - 1,
            ..request
        },
        RefinementLimits {
            max_bytes: request.max_bytes - 1,
            ..request
        },
    ] {
        assert!(matches!(
            base.refine_with_budget(request, budget.clone(), || false),
            Err(SurfaceError::Limit)
        ));
        assert_eq!(budget.used(), baseline);
    }
    drop(other);
    drop(r);
    assert_eq!(budget.used(), 0);
}

#[test]
fn unrepresentable_slice_transfers_refuse_without_welding_or_publishing_partial_paths() {
    let budget = MemoryBudget::new(MAX_BYTES);
    let root = positive_root(2, 0, -1);
    let grown = coord(root, [3000, 3001], [4000, 4001]);
    for u in [grown, rational(1, u64::MAX.into())] {
        let base = build([[-1, -1, 2, 1], [0; 4]], &[[u, rational(3, 4)]]);
        assert!(matches!(
            base.refine_with_budget(limits(8), budget.clone(), || false),
            Err(SurfaceError::Geometry)
        ));
        assert_eq!(budget.used(), 0);
        assert!(
            base.nodes()
                .any(|(_, n)| n.coordinates[0].compare(u).unwrap() == Ordering::Equal)
        );
    }
}

#[test]
fn refined_paths_are_independent_of_field_order_sign_and_constraint_order() {
    let fields = [[-1, -1, 2, 1], [3, -29, 3, 19]];
    let root = positive_root(2, 0, -1);
    let points = [
        [coord(root, [0, 1], [1, 0]), rational(1, 3)],
        [rational(3, 7), rational(4, 9)],
    ];
    let r = build(fields, &points).refine(limits(9), || false).unwrap();
    for fields in [
        [fields[1], fields[0]],
        [fields[0].map(|v| -v), fields[1]],
        [fields[0], fields[1].map(|v| 2 * v)],
    ] {
        let base = build(fields, &[points[1], points[0]]);
        let other = base.refine(limits(9), || false).unwrap();
        verify(&other, &base);
        same(&r, &other);
    }
}

#[test]
fn rounded_equal_constraint_slices_stay_separate_and_original_vertices_are_unchanged() {
    let n = (1 << 20) - 3;
    let a = coord(positive_root(n, n, -1), [1, 1], [2, 0]);
    let b = coord(positive_root(n + 2, n, -1), [1, 1], [2, 0]);
    assert_eq!(a.approximate(), b.approximate());
    let base = build(
        [[-1, -1, 2, 1], [0; 4]],
        &[[a, rational(1, 3)], [b, rational(1, 3)]],
    );
    let r = base.refine(limits(8), || false).unwrap();
    verify(&r, &base);
    assert!(
        r.cuts
            .iter()
            .any(|u| u.compare(a).unwrap() == Ordering::Equal)
    );
    assert!(
        r.cuts
            .iter()
            .any(|u| u.compare(b).unwrap() == Ordering::Equal)
    );
}

#[test]
fn maximum_detail_converges_with_two_near_asymptote_graphs_and_preserves_error() {
    let base = build([[-1, -1, 254, -1], [254, -1, -1, -1]], &[]);
    let r = base.refine(limits(MAX_BITS), || false).unwrap();
    verify(&r, &base);
    exact_error(&r);
    assert!(r.vertices.len() > base.count);
}

#[test]
fn refinement_limits_memory_and_every_cancellation_preserve_the_prior_result() {
    let base = build([[-1, -1, 2, 1], [0, -1, 0, 1]], &[]);
    let budget = MemoryBudget::new(MAX_BYTES);
    let request = RefinementLimits {
        max_vertices: 256,
        ..limits(4)
    };
    let calls = Cell::new(0);
    let prior = base
        .refine_with_budget(request, budget.clone(), || {
            calls.set(calls.get() + 1);
            false
        })
        .unwrap();
    let original = prior.indices.clone();
    let baseline = budget.used();
    for at in 0..calls.get() {
        let n = Cell::new(0);
        assert!(matches!(
            base.refine_with_budget(request, budget.clone(), || {
                let i = n.get();
                n.set(i + 1);
                i == at
            }),
            Err(SurfaceError::Cancelled)
        ));
        assert_eq!(budget.used(), baseline);
        assert_eq!(prior.indices, original);
    }
    for request in [
        limits(25),
        RefinementLimits {
            max_vertices: base.count,
            ..limits(16)
        },
        RefinementLimits {
            max_bytes: 1,
            ..request
        },
        RefinementLimits {
            max_vertices: MAX_VERTICES + 1,
            ..request
        },
    ] {
        assert!(matches!(
            base.refine_with_budget(request, budget.clone(), || false),
            Err(SurfaceError::Limit)
        ));
        assert_eq!(budget.used(), baseline);
    }
    assert!(matches!(
        base.refine_with_budget(request, MemoryBudget::new(0), || false),
        Err(SurfaceError::Memory)
    ));
    let scratch = std::mem::size_of::<Scratch>()
        + request.max_vertices * std::mem::size_of::<ExactCoordinate>();
    budget.set_limit(baseline + scratch as u64);
    assert!(matches!(
        base.refine_with_budget(request, budget.clone(), || false),
        Err(SurfaceError::Memory)
    ));
    assert_eq!(budget.used(), baseline);
    assert_eq!(prior.indices, original);
    drop(prior);
    assert_eq!(budget.used(), 0);
}
