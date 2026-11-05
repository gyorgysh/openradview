// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

fn bytes(value: Integer) -> Vec<u8> {
    value.limbs.into_iter().flat_map(u64::to_le_bytes).collect()
}
fn reference_product(a: Integer, b: Integer) -> Vec<u8> {
    let mut result = vec![0u32; 256];
    for (i, left) in bytes(a).into_iter().enumerate() {
        for (j, right) in bytes(b).into_iter().enumerate() {
            result[i + j] += u32::from(left) * u32::from(right);
        }
    }
    for i in 0..255 {
        result[i + 1] += result[i] >> 8;
        result[i] &= 255;
    }
    result.into_iter().map(|v| v as u8).collect()
}
fn reference_sum(a: Integer, b: Integer) -> Vec<u8> {
    let mut carry = 0u16;
    let mut result: Vec<_> = bytes(a)
        .into_iter()
        .zip(bytes(b))
        .map(|(a, b)| {
            carry += u16::from(a) + u16::from(b);
            let value = carry as u8;
            carry >>= 8;
            value
        })
        .collect();
    result.push(carry as u8);
    result
}
#[test]
fn signed_arithmetic_and_zero_are_exact_at_small_and_extreme_values() {
    for a in -40..=40 {
        for b in -40..=40 {
            let left = Integer::new(a);
            let right = Integer::new(b);
            assert_eq!(left.sum(right), Ok(Integer::new(a + b)));
            assert_eq!(left.difference(right), Ok(Integer::new(a - b)));
            assert_eq!(left.product(right), Ok(Integer::new(a * b)));
        }
    }
    for value in [i128::MIN, i128::MIN + 1, -1, 0, 1, i128::MAX] {
        let a = Integer::new(value);
        assert_eq!(a.difference(a), Ok(Integer::new(0)));
        assert_eq!(a.sum(a.negative()), Ok(Integer::new(0)));
        assert_eq!(a.product(Integer::new(0)), Ok(Integer::new(0)));
        assert_eq!(a.product(Integer::new(-1)), Ok(a.negative()));
        assert_eq!(a.negative().negative(), a);
    }
    let maximum = Integer::with_sign([u64::MAX; LIMBS], 1);
    assert_eq!(maximum.sum(Integer::new(1)), Err(FieldError::Arithmetic));
    assert_eq!(
        maximum.product(Integer::new(2)),
        Err(FieldError::Arithmetic)
    );
    assert_eq!(maximum.product(maximum), Err(FieldError::Arithmetic));
    assert_eq!(maximum.difference(maximum), Ok(Integer::new(0)));
    assert_eq!(
        maximum
            .difference(Integer::new(1))
            .unwrap()
            .sum(Integer::new(1)),
        Ok(maximum)
    );
    let mut power = [0; LIMBS];
    power[LIMBS - 1] = 1;
    let power = Integer::with_sign(power, 1);
    let borrow = power.difference(Integer::new(1)).unwrap();
    assert_eq!(borrow.limbs[..LIMBS - 1], [u64::MAX; LIMBS - 1]);
    assert_eq!(borrow.limbs[LIMBS - 1], 0);
    assert_eq!(borrow.sum(Integer::new(1)), Ok(power));
}

#[test]
fn full_width_products_sums_and_overflows_match_independent_byte_arithmetic() {
    let mut state = 37u64;
    let mut next = || {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        state
    };
    for case in 0..256 {
        let a_used = case % LIMBS + 1;
        let b_used = case / LIMBS + 1;
        let a = Integer::with_sign(
            std::array::from_fn(|i| if i < a_used { next() } else { 0 }),
            1,
        );
        let b = Integer::with_sign(
            std::array::from_fn(|i| if i < b_used { next() } else { 0 }),
            1,
        );
        let expected = reference_product(a, b);
        match a.product(b) {
            Ok(value) => {
                assert!(expected[128..].iter().all(|v| *v == 0));
                assert_eq!(bytes(value), expected[..128]);
                assert_eq!(a.negative().product(b), Ok(value.negative()));
            }
            Err(FieldError::Arithmetic) => assert!(expected[128..].iter().any(|v| *v != 0)),
            other => panic!("Unexpected product: {other:?}"),
        }
        let expected = reference_sum(a, b);
        match a.sum(b) {
            Ok(value) => {
                assert_eq!(expected[128], 0);
                assert_eq!(bytes(value), expected[..128]);
                assert_eq!(value.difference(b), Ok(a));
                assert_eq!(a.negative().sum(b.negative()), Ok(value.negative()));
            }
            Err(FieldError::Arithmetic) => assert_ne!(expected[128], 0),
            other => panic!("Unexpected sum: {other:?}"),
        }
        let difference = a.difference(b).unwrap();
        assert_eq!(difference.negative(), b.difference(a).unwrap());
        assert_eq!(difference.sum(b), Ok(a));
    }
}
