// SPDX-License-Identifier: GPL-3.0-or-later
use super::super::super::super::tests::{coord, positive_root};
use super::*;
use std::{cell::Cell, collections::BTreeSet};

fn build(values: [i16; 4]) -> PlaneSubdivision {
    Bilinear::new(values)
        .unwrap()
        .subdivision(&[], &[], || false)
        .unwrap()
}
fn limits(bits: u8) -> RefinementLimits {
    RefinementLimits {
        precision_bits: bits,
        ..Default::default()
    }
}
fn verify_prefix(refined: &RefinedSubdivision, original: &PlaneSubdivision) {
    for (id, node) in original.nodes() {
        let point = refined.vertex(u32::from(id)).unwrap();
        assert_eq!(
            point.map(representation),
            node.coordinates.map(representation)
        );
    }
    assert_eq!(
        refined.subdivision().half_edges().collect::<Vec<_>>(),
        original.half_edges().collect::<Vec<_>>()
    );
    for (id, edge) in original.edges() {
        let path = refined.edge_vertices(id).unwrap();
        assert_eq!(path.first(), Some(&u32::from(edge.from)));
        assert_eq!(path.last(), Some(&u32::from(edge.to)));
        assert!(path.len() >= 2);
        let mut used = BTreeSet::new();
        for vertex in path {
            assert!(used.insert(*vertex));
            assert!(refined.vertex(*vertex).is_some());
        }
    }
}
fn same(a: &RefinedSubdivision, b: &RefinedSubdivision) {
    assert_eq!(a.vertices.len(), b.vertices.len());
    assert_eq!(a.indices, b.indices);
    for (a, b) in a.vertices.iter().zip(&b.vertices) {
        assert_eq!(a.map(representation), b.map(representation));
    }
}
fn raw(point: [ExactCoordinate; 2]) -> [(i128, i128); 2] {
    point.map(|v| {
        let r = v.rational_value().unwrap();
        (i128::from(r.numerator), i128::from(r.denominator))
    })
}
fn zero(field: Bilinear, [(u, uq), (v, vq)]: [(i128, i128); 2]) -> i128 {
    let [a, b, c, d] = field.values.map(i128::from);
    a * (uq - u) * (vq - v) + b * u * (vq - v) + c * u * v + d * (uq - u) * v
}
// Independent base256 products/differences avoid the production interval and
// chord-certificate arithmetic when comparing exact sampled distances.
type Bytes = [u8; 48];
fn product(a: u128, b: u128) -> Bytes {
    let mut digits = [0u32; 48];
    for (i, a) in a.to_le_bytes().into_iter().enumerate() {
        for (j, b) in b.to_le_bytes().into_iter().enumerate() {
            digits[i + j] += u32::from(a) * u32::from(b);
        }
    }
    for i in 0..47 {
        digits[i + 1] += digits[i] >> 8;
        digits[i] &= 255;
    }
    digits.map(|v| v as u8)
}
fn magnitude(a: Bytes, b: Bytes) -> Bytes {
    let (a, b) = if a.into_iter().rev().cmp(b.into_iter().rev()) == Ordering::Less {
        (b, a)
    } else {
        (a, b)
    };
    let mut borrow = 0i16;
    std::array::from_fn(|i| {
        let value = i16::from(a[i]) - i16::from(b[i]) - borrow;
        borrow = i16::from(value < 0);
        (value + 256 * borrow) as u8
    })
}
pub(in super::super::super) fn within(a: (i128, i128), b: (i128, i128), bits: u8) -> bool {
    assert!(a.0 >= 0 && a.1 > 0 && b.0 >= 0 && b.1 > 0);
    let mut distance = magnitude(
        product(a.0 as u128, b.1 as u128),
        product(b.0 as u128, a.1 as u128),
    );
    for _ in 0..bits {
        let mut carry = 0u16;
        for digit in &mut distance {
            let v = 2 * u16::from(*digit) + carry;
            *digit = v as u8;
            carry = v >> 8;
        }
        assert_eq!(carry, 0);
    }
    distance
        .into_iter()
        .rev()
        .cmp(product(a.1 as u128, b.1 as u128).into_iter().rev())
        != Ordering::Greater
}
pub(in super::super::super) fn fraction(n: i128, d: i128) -> (i128, i128) {
    assert_ne!(d, 0);
    let sign = d.signum();
    let p = (n * sign, d.abs());
    assert!((0..=p.1).contains(&p.0));
    p
}
fn verify_error(refined: &RefinedSubdivision) {
    let field = refined.subdivision().field();
    let [a, b, c, d] = coefficients(field).map(i128::from);
    for (id, edge) in refined.subdivision().edges() {
        let path = refined.edge_vertices(id).unwrap();
        if !edge.zero_curve || edge.boundary.is_some() || d == 0 || a * d == b * c {
            assert_eq!(path.len(), 2);
            continue;
        }
        for pair in path.windows(2) {
            let p = raw(*refined.vertex(pair[0]).unwrap());
            let q = raw(*refined.vertex(pair[1]).unwrap());
            assert_eq!(zero(field, p), 0);
            assert_eq!(zero(field, q), 0);
            assert!(p[0].0 * q[0].1 < q[0].0 * p[0].1);
            let (mut vertical, mut horizontal) = (true, true);
            for k in 1..16 {
                let chord: [(i128, i128); 2] = std::array::from_fn(|i| {
                    (
                        (16 - k) * p[i].0 * q[i].1 + k * q[i].0 * p[i].1,
                        16 * p[i].1 * q[i].1,
                    )
                });
                let [(u, uq), (v, vq)] = chord;
                let curve_v = fraction(-(a * uq + b * u), c * uq + d * u);
                let curve_u = fraction(-(a * vq + c * v), b * vq + d * v);
                vertical &= within(chord[1], curve_v, refined.bits);
                horizontal &= within(chord[0], curve_u, refined.bits);
            }
            assert!(
                vertical || horizontal,
                "Independent exact samples exceed tolerance: {:?}",
                field
            );
        }
    }
}

#[test]
fn straight_and_nearly_flat_edges_remain_sparse_while_curves_refine() {
    for field in [
        [1; 4],
        [-1; 4],
        [0; 4],
        [-1, 1, 1, -1],
        [1, -1, 1, -1],
        [0, -1, 1, 0],
        [-1, -1, 200, 199],
    ] {
        let base = build(field);
        let refined = base.refine(limits(24), || false).unwrap();
        verify_prefix(&refined, &base);
        assert_eq!(refined.vertices().len(), base.nodes().count());
        assert!(
            base.edges()
                .all(|(id, _)| refined.edge_vertices(id).unwrap().len() == 2)
        );
    }
    let base = build([-1, -1, 2, 1]);
    let coarse = base.refine(limits(8), || false).unwrap();
    let fine = base.refine(limits(12), || false).unwrap();
    assert!(coarse.vertices.len() > base.nodes().count());
    assert!(fine.vertices.len() > coarse.vertices.len());
    assert!(coarse.vertices.len() < base.nodes().count() + 64);
    for point in &coarse.vertices {
        assert!(
            fine.vertices
                .iter()
                .any(|other| same_point(*point, *other).unwrap())
        );
    }
    verify_error(&coarse);
    verify_error(&fine);
}

#[test]
fn generated_refinement_has_exact_zero_vertices_and_independently_bounded_errors() {
    let mut fields = Vec::new();
    for mut code in 0..81 {
        fields.push(std::array::from_fn(|_| {
            let v = code % 3;
            code /= 3;
            v as i16 - 1
        }));
    }
    let mut state = 277u64;
    for _ in 0..256 {
        fields.push(std::array::from_fn(|_| {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (state % 511) as i16 - 255
        }));
    }
    for values in fields {
        let base = build(values);
        let budget = MemoryBudget::new(MAX_BYTES);
        let refined = base
            .refine_with_budget(limits(6), budget.clone(), || false)
            .unwrap();
        verify_prefix(&refined, &base);
        verify_error(&refined);
        assert_eq!(budget.used(), refined.referenced_bytes());
        assert_eq!(
            refined.maximum_coordinate_error(),
            Rational::new(1, 64).unwrap()
        );
        assert_eq!(
            refined.referenced_bytes(),
            bytes(refined.vertices.len(), refined.indices.len()).unwrap()
        );
        drop(refined);
        assert_eq!(budget.used(), 0);
    }
}

#[test]
fn completed_curve_vertices_are_deterministic_under_sign_scale_and_input_order() {
    let r = positive_root(2, 0, -1);
    let point = [coord(r, [0, 1], [1, 0]), coord(r, [1, 0], [2, 1])];
    let other = [
        coord(positive_root(3, 0, -1), [0, 1], [1, 0]),
        coord(positive_root(3, 0, -1), [1, 0], [2, 1]),
    ];
    let base = Bilinear::new([-1, -1, 2, 1])
        .unwrap()
        .subdivision(&[], &[point, other], || false)
        .unwrap();
    let refined = base.refine(limits(10), || false).unwrap();
    for values in [[1, 1, -2, -1], [-2, -2, 4, 2]] {
        let changed = Bilinear::new(values)
            .unwrap()
            .subdivision(&[], &[other, point], || false)
            .unwrap()
            .refine(limits(10), || false)
            .unwrap();
        same(&refined, &changed);
    }
    verify_prefix(&refined, &base);
}

#[test]
fn mathematically_distinct_rounded_equal_endpoints_are_not_welded() {
    let n = (1 << 20) - 3;
    let a = positive_root(n, n, -1);
    let b = positive_root(n + 2, n, -1);
    let points = [a, b].map(|r| [coord(r, [1, 1], [2, 0]), coord(r, [2, 0], [5, 1])]);
    assert_eq!(points[0][0].approximate(), points[1][0].approximate());
    assert_ne!(points[0][0].compare(points[1][0]).unwrap(), Ordering::Equal);
    let base = Bilinear::new([-1, -1, 2, 1])
        .unwrap()
        .subdivision(&[], &points, || false)
        .unwrap();
    let ids = points.map(|p| {
        base.nodes()
            .find(|(_, n)| same_point(n.coordinates, p).unwrap())
            .unwrap()
            .0
    });
    let edge = base
        .edges()
        .find(|(_, e)| (e.from, e.to) == (ids[0].min(ids[1]), ids[0].max(ids[1])))
        .unwrap()
        .0;
    let refined = base.refine(limits(8), || false).unwrap();
    assert_eq!(
        refined.edge_vertices(edge).unwrap(),
        [u32::from(ids[0].min(ids[1])), u32::from(ids[0].max(ids[1]))]
    );
    assert_ne!(ids[0], ids[1]);
    verify_prefix(&refined, &base);
}

#[test]
fn count_admission_refuses_pressure_without_coarsening_or_changing_prior_results() {
    let base = build([-1, -1, 2, 1]);
    let budget = MemoryBudget::new(MAX_BYTES);
    let prior = base
        .refine_with_budget(limits(8), budget.clone(), || false)
        .unwrap();
    let original = prior.indices.clone();
    let baseline = budget.used();
    for request in [
        RefinementLimits {
            max_vertices: base.nodes().count(),
            ..limits(16)
        },
        RefinementLimits {
            max_bytes: 1,
            ..limits(8)
        },
        limits(25),
        RefinementLimits {
            max_vertices: MAX_VERTICES + 1,
            ..limits(8)
        },
    ] {
        assert!(matches!(
            base.refine_with_budget(request, budget.clone(), || false),
            Err(SurfaceError::Limit)
        ));
        assert_eq!(budget.used(), baseline);
        assert_eq!(prior.indices, original);
    }
    assert!(matches!(
        base.refine_with_budget(limits(8), MemoryBudget::new(0), || false),
        Err(SurfaceError::Memory)
    ));
    let scratch = std::mem::size_of::<[Option<Enclosed>; NODES]>()
        + std::mem::size_of::<[Option<Segment>; DEPTH]>();
    budget.set_limit(baseline + scratch as u64);
    assert!(matches!(
        base.refine_with_budget(limits(8), budget.clone(), || false),
        Err(SurfaceError::Memory)
    ));
    assert_eq!(budget.used(), baseline);
    assert_eq!(prior.indices, original);
    drop(prior);
    assert_eq!(budget.used(), 0);
}

#[test]
fn every_refinement_cancellation_releases_scratch_and_unpublished_output() {
    let base = build([-1, -1, 2, 1]);
    let budget = MemoryBudget::new(MAX_BYTES);
    let calls = Cell::new(0);
    let prior = base
        .refine_with_budget(limits(8), budget.clone(), || {
            calls.set(calls.get() + 1);
            false
        })
        .unwrap();
    let baseline = budget.used();
    let vertices = prior.vertices.len();
    for at in 0..calls.get() {
        let n = Cell::new(0);
        assert!(matches!(
            base.refine_with_budget(limits(8), budget.clone(), || {
                let i = n.get();
                n.set(i + 1);
                i == at
            }),
            Err(SurfaceError::Cancelled)
        ));
        assert_eq!(budget.used(), baseline);
        assert_eq!(prior.vertices.len(), vertices);
    }
    drop(prior);
    assert_eq!(budget.used(), 0);
}

#[test]
fn maximum_precision_converges_near_asymptotes_without_losing_the_error_bound() {
    for values in [[-1, -1, 254, -1], [254, -1, -1, -1], [255, -255, 254, -255]] {
        let base = build(values);
        let refined = base.refine(limits(MAX_BITS), || false).unwrap();
        verify_prefix(&refined, &base);
        verify_error(&refined);
        assert!(refined.vertices.len() > base.nodes().count());
        assert!(refined.vertices.len() <= MAX_VERTICES);
    }
}

#[test]
fn coordinate_enclosures_keep_exact_grid_ties_and_irrational_polynomial_brackets() {
    let root = positive_root(2, 0, -1);
    let point = [coord(root, [0, 1], [1, 0]), coord(root, [1, 0], [2, 1])];
    let enclosed = Enclosed::new(point, None, &|| false).unwrap();
    let q = i128::from(GRID);
    let k = i128::from(enclosed.lower[0]);
    assert!(2 * k * k < q * q && 2 * (k + 1) * (k + 1) > q * q);
    let k = i128::from(enclosed.lower[1]);
    let polynomial = |v: i128| 7 * v * v - 8 * v * q + 2 * q * q;
    assert!(polynomial(k) > 0 && polynomial(k + 1) < 0);
    for value in [
        Rational::new(0, 1).unwrap(),
        Rational::new(1, 2).unwrap(),
        Rational::new(1, 3).unwrap(),
        Rational::new(1, 1).unwrap(),
        Rational::new(i128::from(i64::MAX), i128::from(i64::MAX) + 1).unwrap(),
    ] {
        let coordinate = ExactCoordinate::from_rational(value).unwrap().unwrap();
        let e = Enclosed::new([coordinate; 2], None, &|| false).unwrap();
        assert!(e.upper[0] - e.lower[0] <= 1);
        let lower = super::coordinate(e.lower[0], GRID).unwrap();
        let upper = super::coordinate(e.upper[0], GRID).unwrap();
        assert_ne!(coordinate.compare(lower).unwrap(), Ordering::Less);
        assert_ne!(coordinate.compare(upper).unwrap(), Ordering::Greater);
        if value.denominator().is_power_of_two() && value.denominator() <= GRID {
            assert_eq!(e.lower, e.upper);
        }
    }
}
