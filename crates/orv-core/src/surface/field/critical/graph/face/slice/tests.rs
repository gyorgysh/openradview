// SPDX-License-Identifier: GPL-3.0-or-later
use super::super::super::tests::{coord, positive_root, rational};
use super::*;

#[test]
fn rational_slices_match_independent_endpoint_interpolation_including_whole_lines() {
    for case in 0..81usize {
        let mut digits = case;
        let values = std::array::from_fn(|_| {
            let value = (digits % 3) as i16 - 1;
            digits /= 3;
            value
        });
        let field = Bilinear::new(values).unwrap();
        for axis in 0..2 {
            let edges = if axis == 0 {
                [[0, 1], [3, 2]]
            } else {
                [[0, 3], [1, 2]]
            };
            for n in 0..=16i64 {
                let endpoint = edges.map(|[lo, hi]| {
                    i128::from(values[lo]) * i128::from(16 - n)
                        + i128::from(values[hi]) * i128::from(n)
                });
                let result = field.zero_slice(axis, rational(n, 16)).unwrap();
                match (endpoint, result) {
                    ([0, 0], BilinearSlice::Whole) => {}
                    ([a, b], BilinearSlice::Empty) => assert!(a.signum() == b.signum() && a != 0),
                    ([a, b], BilinearSlice::Point(value)) => {
                        let value = value.rational_value().unwrap();
                        assert_eq!(
                            a * i128::from(value.denominator),
                            (a - b) * i128::from(value.numerator)
                        );
                        assert!(
                            (0..=i128::from(value.denominator))
                                .contains(&i128::from(value.numerator))
                        );
                    }
                    other => panic!("Incorrect slice for {values:?}/{axis}/{n}: {other:?}"),
                }
            }
        }
    }
}

#[test]
fn irrational_slices_keep_exact_membership_and_axis_symmetry_for_extreme_fields() {
    let mut state = 319u64;
    let mut next = || {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        state
    };
    let root = positive_root((1 << 20) - 1, (1 << 20) - 2, -((1 << 20) - 3));
    let fixed = coord(root, [2039, -2040], [2040, 2039]);
    let negative = coord(root, [-2039, 2040], [-2040, -2039]);
    let mut points = 0;
    for case in 0..1024 {
        let values = std::array::from_fn(|i| {
            if case < 16 {
                if case & (1 << i) != 0 { 255 } else { -255 }
            } else {
                (next() % 511) as i16 - 255
            }
        });
        let field = Bilinear::new(values).unwrap();
        let transposed = Bilinear::new([values[0], values[3], values[2], values[1]]).unwrap();
        for axis in 0..2 {
            let result = field.zero_slice(axis, fixed).unwrap();
            let other = transposed.zero_slice(1 - axis, negative).unwrap();
            match (result, other) {
                (BilinearSlice::Point(value), BilinearSlice::Point(same)) => {
                    assert_eq!(value.compare(same), Ok(Ordering::Equal));
                    let point = if axis == 0 {
                        [fixed, value]
                    } else {
                        [value, fixed]
                    };
                    assert_eq!(bilinear_sign(coefficients(field), point), Ok(0));
                    assert!(value.approximate().is_some());
                    points += 1;
                }
                (BilinearSlice::Empty, BilinearSlice::Empty)
                | (BilinearSlice::Whole, BilinearSlice::Whole) => {}
                other => panic!("Axis or denominator-sign mismatch: {other:?}"),
            }
        }
    }
    assert!(points > 800);
}

#[test]
fn slice_poles_lines_bounds_and_large_ratios_are_explicit() {
    let factor = Bilinear::new([0, -1, 0, 0]).unwrap(); // -u*(1-v)
    assert!(matches!(
        factor.zero_slice(0, rational(0, 1)),
        Ok(BilinearSlice::Whole)
    ));
    let BilinearSlice::Point(end) = factor.zero_slice(0, rational(1, 2)).unwrap() else {
        panic!("Endpoint required")
    };
    assert_eq!(end.compare(rational(1, 1)), Ok(Ordering::Equal));
    let pole = Bilinear::new([1, 1, 2, 0]).unwrap();
    assert!(matches!(
        pole.zero_slice(0, rational(1, 2)),
        Ok(BilinearSlice::Empty)
    ));
    assert!(matches!(
        pole.zero_slice(2, rational(1, 2)),
        Err(FieldError::Bounds)
    ));
    let large = rational(1, u64::MAX.into());
    let field = Bilinear::new([1, 1, -1, -2]).unwrap();
    // v=q/(3q-n) is inside the square but its reduced denominator exceeds u64.
    assert!(matches!(
        field.zero_slice(0, large),
        Err(FieldError::Arithmetic)
    ));
    let outside = Bilinear::new([1, 2, 2, 1]).unwrap();
    assert!(matches!(
        outside.zero_slice(0, large),
        Ok(BilinearSlice::Empty)
    ));
    let root = positive_root(2, 0, -1);
    let grown = coord(root, [3000, 3001], [4000, 4001]);
    assert!(matches!(
        field.zero_slice(0, grown),
        Err(FieldError::Arithmetic)
    ));
    let at = coord(root, [0, 1], [1, 0]);
    let diagonal = Bilinear::new([0, 1, 0, -1]).unwrap();
    let BilinearSlice::Point(other) = diagonal.zero_slice(0, at).unwrap() else {
        panic!("Diagonal point required")
    };
    assert_eq!(at.compare_same_field(other), Ok(Ordering::Equal));
}
