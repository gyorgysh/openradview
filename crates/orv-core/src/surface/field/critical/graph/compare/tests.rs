// SPDX-License-Identifier: GPL-3.0-or-later
use super::super::tests::{coord, positive_root, rational};
use super::*;

#[test]
fn unrelated_quadratic_coordinates_order_with_signed_denominators() {
    let half = coord(positive_root(2, 0, -1), [0, 1], [1, 0]);
    let third = coord(positive_root(3, 0, -1), [0, 1], [1, 0]);
    let negative = coord(positive_root(3, 0, -1), [0, -1], [-1, 0]);
    assert_eq!(half.compare(third), Ok(Ordering::Greater));
    assert_eq!(half.compare(negative), Ok(Ordering::Greater));
    // Both conjugate denominators change sign in these rationalizations.
    let x = coord(positive_root(2, 0, -1), [0, 1], [0, 2]);
    assert_eq!(x.compare(third), Ok(Ordering::Less));
    let x = coord(positive_root(2, 0, -1), [1, 0], [1, 1]);
    let y = coord(positive_root(3, 0, -1), [1, 0], [1, 1]);
    assert_eq!(x.compare(y), Ok(Ordering::Less));
    let lower_x = coord(positive_root(2, -4, 1), [0, 1], [1, 0]);
    let lower_y = coord(positive_root(3, -6, 2), [0, 1], [1, 0]);
    assert_eq!(lower_x.compare(lower_y), Ok(Ordering::Less));
    let x = coord(positive_root(2, 0, -1), [1, -1], [-1, 3]);
    let y = coord(positive_root(3, 0, -1), [1, -1], [-1, 3]);
    assert_eq!(x.compare(y), Ok(Ordering::Less));
    for a in [half, third, negative, x, y, rational(0, 1), rational(1, 1)] {
        assert_eq!(a.compare(a), Ok(Ordering::Equal));
        for b in [half, third, negative, x, y] {
            assert_eq!(a.compare(b).unwrap(), b.compare(a).unwrap().reverse());
        }
    }
}

#[test]
fn two_radical_sign_isolates_opposite_terms_and_retains_exact_ties() {
    let i = Integer::new;
    // sqrt(2)+sqrt(3) is between 3 and 4; squaring once leaves sqrt(2).
    for (a, b, d, c, e, expected) in [
        (-3, 1, 2, 1, 3, 1),
        (-4, 1, 2, 1, 3, -1),
        (3, -1, 2, -1, 3, -1),
        (4, -1, 2, -1, 3, 1),
        (0, 1, 2, -1, 3, -1),
        (0, -1, 2, 1, 3, 1),
        (0, 2, 2, -1, 8, 0),
        (0, -2, 2, 1, 8, 0),
        (1, 0, 2, -1, 1, 0),
        (0, 0, 2, 1, 3, 1),
        (-1, 0, 2, 0, 3, -1),
        (1, 1, 2, 1, 3, 1),
        (0, 1, 0, -1, 3, -1),
        (1, 1, 2, 4, 0, 1),
    ] {
        assert_eq!(two_radical_sign(i(a), i(b), d, i(c), e), Ok(expected));
        assert_eq!(two_radical_sign(i(-a), i(-b), d, i(-c), e), Ok(-expected));
    }
    // Cancellation between 110-bit coefficients leaves only a small radical.
    let large = i(1i128 << 110);
    assert_eq!(two_radical_sign(large, i(1), 2, large.negative(), 1), Ok(1));
    assert_eq!(
        two_radical_sign(large, i(-1), 2, large.negative(), 1),
        Ok(-1)
    );
    assert_eq!(
        two_radical_sign(large, i(1), 2, large.negative(), 2),
        Ok(-1)
    );
}

#[test]
fn unrelated_coordinates_that_round_to_the_same_float_remain_ordered() {
    let n = (1 << 20) - 3;
    let x = coord(positive_root(n, n, -1), [1, 1], [2, 0]);
    let y = coord(positive_root(n + 2, n, -1), [1, 1], [2, 0]);
    assert_eq!(x.approximate(), y.approximate());
    assert!(x.compare_same_field(y).is_err());
    // The second polynomial evaluated at the first positive root is 2t²>0;
    // both have one negative and one positive root, hence its positive root
    // is strictly smaller. This establishes the order without rounded roots.
    assert_eq!(x.compare(y), Ok(Ordering::Greater));
    assert_eq!(y.compare(x), Ok(Ordering::Less));
}

// Independent enclosure: bisect the root using its polynomial at exact dyadic
// rationals, then bound the fractional-linear coordinate by its two endpoints.
// No radical signs or comparator arithmetic are used by this reference.
pub(super) fn coordinate_bounds(value: ExactCoordinate, depth: u32) -> Option<[(i128, i128); 2]> {
    if let Some(value) = value.rational_value() {
        let point = (i128::from(value.numerator), i128::from(value.denominator));
        return Some([point, point]);
    }
    let Root::Surd { a, b, c, upper } = value.parameter.0 else {
        return None;
    };
    let below = |at: i128, denominator: i128| {
        let vertex = 2 * i128::from(a) * at + i128::from(b) * denominator;
        let evaluation = i128::from(a) * at * at
            + i128::from(b) * at * denominator
            + i128::from(c) * denominator * denominator;
        if upper {
            vertex <= 0 || evaluation < 0
        } else {
            vertex < 0 && evaluation > 0
        }
    };
    if !below(0, 1) || below(1, 1) {
        return None;
    }
    let (mut lower, mut denominator) = (0i128, 1i128);
    for _ in 0..depth {
        let middle = lower * 2 + 1;
        denominator *= 2;
        lower = 2 * lower + i128::from(below(middle, denominator));
    }
    let endpoint = |at: i128| {
        let n = i128::from(value.numerator.constant) * denominator
            + i128::from(value.numerator.slope) * at;
        let d = i128::from(value.denominator.constant) * denominator
            + i128::from(value.denominator.slope) * at;
        (n * d.signum(), d.abs())
    };
    let low = endpoint(lower);
    let high = endpoint(lower + 1);
    let dl = i128::from(value.denominator.constant) * denominator
        + i128::from(value.denominator.slope) * lower;
    let dh = i128::from(value.denominator.constant) * denominator
        + i128::from(value.denominator.slope) * (lower + 1);
    if dl.signum() != dh.signum() || dl == 0 {
        return None;
    }
    Some(if low.0 * high.1 <= high.0 * low.1 {
        [low, high]
    } else {
        [high, low]
    })
}
fn independent_order(a: ExactCoordinate, b: ExactCoordinate) -> Option<Ordering> {
    for depth in [8, 16, 24, 32, 40, 48] {
        let Some([al, ah]) = coordinate_bounds(a, depth) else {
            continue;
        };
        let Some([bl, bh]) = coordinate_bounds(b, depth) else {
            continue;
        };
        if ah.0 * bl.1 < bl.0 * ah.1 {
            return Some(Ordering::Less);
        }
        if bh.0 * al.1 < al.0 * bh.1 {
            return Some(Ordering::Greater);
        }
        if al == ah && bl == bh && al.0 * bl.1 == bl.0 * al.1 {
            return Some(Ordering::Equal);
        }
    }
    None
}

#[test]
fn shared_face_anchor_points_from_both_incident_cells_have_one_exact_order() {
    let mut state = 227u64;
    let mut next = || {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (state % 511) as i16 - 255
    };
    let mut cross_field_orders = 0;
    for _ in 0..512 {
        let face: [i16; 4] = std::array::from_fn(|_| next());
        let left = Trilinear::new(std::array::from_fn(|i| {
            if i & 1 == 1 { face[i >> 1] } else { next() }
        }))
        .unwrap()
        .critical_graph(|| false)
        .unwrap();
        let right = Trilinear::new(std::array::from_fn(|i| {
            if i & 1 == 0 { face[i >> 1] } else { next() }
        }))
        .unwrap()
        .critical_graph(|| false)
        .unwrap();
        for (_, a) in left.nodes().filter(|(_, node)| node.boundary() & 2 != 0) {
            for (_, b) in right.nodes().filter(|(_, node)| node.boundary() & 1 != 0) {
                for axis in [1, 2] {
                    let a = a.coordinates()[axis];
                    let b = b.coordinates()[axis];
                    let expected = independent_order(a, b)
                        .expect("Shared-face coordinates must have independent enclosures");
                    assert_eq!(a.compare(b), Ok(expected));
                    assert_eq!(b.compare(a), Ok(expected.reverse()));
                    cross_field_orders += usize::from(a.compare_same_field(b).is_err());
                }
            }
        }
    }
    assert!(
        cross_field_orders > 20,
        "Only {cross_field_orders} unrelated shared-face coordinate pairs"
    );
}

#[test]
fn biquadratic_sign_keeps_exact_products_and_cancellation_between_fields() {
    let root = positive_root(2, 0, -1);
    let u = coord(root, [0, 1], [1, 0]);
    let scaled = coord(positive_root(8, 0, -1), [0, 2], [1, 0]);
    assert_eq!(bilinear_sign([-1, 0, 0, 2], [u, scaled]), Ok(0));
    let other = coord(positive_root(3, 0, -1), [0, 1], [1, 0]);
    assert_eq!(bilinear_sign([-1, 0, 0, 2], [u, other]), Ok(-1));
    assert_eq!(bilinear_sign([-1, 1, 1, 1], [u, other]), Ok(1));
    let complement = coord(root, [1, -1], [1, 0]);
    assert_eq!(bilinear_sign([-1, 1, 1, 0], [u, complement]), Ok(0));
    assert_eq!(
        bilinear_sign([2041, 0, 0, 0], [u, other]),
        Err(FieldError::Arithmetic)
    );

    let mut state = 719u64;
    let mut next = || {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        state
    };
    let mut points = Vec::new();
    while points.len() < 32 {
        let a = 2 + (next() % ((1 << 20) - 2)) as i64;
        let root = positive_root(a, 0, -1);
        if root.rational().is_some() {
            continue;
        }
        points.push(coord(root, [1000, 999], [2000, 1999]));
    }
    for u in points.iter().copied() {
        for v in points.iter().copied() {
            for _ in 0..8 {
                let coefficients = std::array::from_fn(|_| (next() % 4081) as i32 - 2040);
                let mut reference = None;
                for depth in [8, 16, 24, 32, 40] {
                    let [ul, uh] = coordinate_bounds(u, depth).unwrap();
                    let [vl, vh] = coordinate_bounds(v, depth).unwrap();
                    let signs =
                        [(ul, vl), (ul, vh), (uh, vl), (uh, vh)].map(|((u, uq), (v, vq))| {
                            let [a, b, c, d] = coefficients.map(i128::from);
                            (a * uq * vq + b * u * vq + c * v * uq + d * u * v).signum() as i8
                        });
                    if signs.iter().all(|s| *s == signs[0]) {
                        reference = Some(signs[0]);
                        break;
                    }
                }
                let expected = reference.expect(
                    "Independent bilinear interval bounds must separate this generated value",
                );
                assert_eq!(bilinear_sign(coefficients, [u, v]), Ok(expected));
                assert_eq!(
                    bilinear_sign(coefficients.map(|v| -v), [u, v]),
                    Ok(-expected)
                );
            }
        }
    }
}

#[test]
fn generated_cross_field_order_matches_independent_polynomial_enclosures() {
    let mut state = 91u64;
    let mut next = || {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        state
    };
    let mut values = Vec::new();
    while values.len() < 96 {
        let a = 2 + (next() % ((1 << 20) - 2)) as i64;
        let b = (next() % (a as u64 / 2 + 1)) as i64 - a / 4;
        let c = -(1 + (next() % (a as u64 / 2)) as i64);
        let root = positive_root(a, b, c);
        if root.rational().is_some() {
            continue;
        }
        let n = [(next() % 900) as i32, (next() % 900) as i32];
        let d = [
            n[0] + 1 + (next() % 900) as i32,
            n[1] + (next() % 900) as i32,
        ];
        let sign = if next().is_multiple_of(2) { 1 } else { -1 };
        values.push(coord(root, n.map(|v| sign * v), d.map(|v| sign * v)));
    }
    let mut unrelated = 0;
    for (index, a) in values.iter().copied().enumerate() {
        for b in values.iter().copied().skip(index + 1) {
            let expected =
                independent_order(a, b).expect("Generated coordinates separate by 48 bits");
            assert_eq!(a.compare(b), Ok(expected));
            assert_eq!(b.compare(a), Ok(expected.reverse()));
            unrelated += usize::from(a.compare_same_field(b).is_err());
        }
    }
    assert!(unrelated > 4500);
    // Check every pair after ordering, rather than only adjacent local decisions.
    values.sort_by(|a, b| a.compare(*b).unwrap());
    for (index, a) in values.iter().copied().enumerate() {
        for b in values.iter().copied().skip(index + 1) {
            assert_eq!(independent_order(a, b), Some(Ordering::Less));
            assert_eq!(a.compare(b), Ok(Ordering::Less));
        }
    }
}
