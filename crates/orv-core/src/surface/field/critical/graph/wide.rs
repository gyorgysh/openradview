// SPDX-License-Identifier: GPL-3.0-or-later
//! Fixed-width products for comparing squared radical terms without truncation.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Wide([u64; 4]);
impl Wide {
    pub fn product(a: u128, b: u128) -> Self {
        let a = [a as u64, (a >> 64) as u64];
        let b = [b as u64, (b >> 64) as u64];
        let mut result = [0; 4];
        for (i, a) in a.into_iter().enumerate() {
            let mut carry = 0;
            for (j, b) in b.into_iter().enumerate() {
                // One limb product plus two limb-sized addends fits u128.
                let value = u128::from(a) * u128::from(b) + u128::from(result[i + j]) + carry;
                result[i + j] = value as u64;
                carry = value >> 64;
            }
            result[i + 2] = carry as u64;
        }
        Self(result)
    }
    pub fn times(self, factor: u64) -> Result<Self, FieldError> {
        let mut result = [0; 4];
        let mut carry = 0;
        for (output, input) in result.iter_mut().zip(self.0) {
            let value = u128::from(input) * u128::from(factor) + carry;
            *output = value as u64;
            carry = value >> 64;
        }
        if carry != 0 {
            return Err(FieldError::Arithmetic);
        }
        Ok(Self(result))
    }
    pub fn compare(self, other: Self) -> std::cmp::Ordering {
        self.0.into_iter().rev().cmp(other.0.into_iter().rev())
    }
}

pub(super) fn radical_sign(p: i128, q: i128, d: u64) -> Result<i8, FieldError> {
    if q == 0 || d == 0 {
        return Ok(p.signum() as i8);
    }
    if p == 0 {
        return Ok(q.signum() as i8);
    }
    if p.signum() == q.signum() {
        return Ok(p.signum() as i8);
    }
    let p2 = Wide::product(p.unsigned_abs(), p.unsigned_abs());
    let q2d = Wide::product(q.unsigned_abs(), q.unsigned_abs()).times(d)?;
    Ok(match p2.compare(q2d) {
        std::cmp::Ordering::Greater => p.signum() as i8,
        std::cmp::Ordering::Less => q.signum() as i8,
        std::cmp::Ordering::Equal => 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn reference(a: u128, b: u128, factor: u64) -> Vec<u8> {
        let mut values = [0u32; 40];
        for (i, a) in a.to_le_bytes().into_iter().enumerate() {
            for (j, b) in b.to_le_bytes().into_iter().enumerate() {
                values[i + j] += u32::from(a) * u32::from(b);
            }
        }
        for i in 0..39 {
            values[i + 1] += values[i] >> 8;
            values[i] &= 255;
        }
        let mut result = [0u32; 40];
        for (i, a) in values.into_iter().enumerate() {
            for (j, b) in factor.to_le_bytes().into_iter().enumerate() {
                if i + j < 40 {
                    result[i + j] += a * u32::from(b);
                }
            }
        }
        for i in 0..39 {
            result[i + 1] += result[i] >> 8;
            result[i] &= 255;
        }
        result.into_iter().map(|v| v as u8).collect()
    }
    fn bytes(value: Wide) -> Vec<u8> {
        value.0.into_iter().flat_map(u64::to_le_bytes).collect()
    }
    #[test]
    fn fixed_products_and_carries_match_independent_byte_arithmetic() {
        let mut state = 17u128;
        let cases = [0, 1, u64::MAX as u128, 1 << 64, (1 << 127) + 1, u128::MAX];
        for a in cases {
            for b in cases {
                assert_eq!(bytes(Wide::product(a, b)), reference(a, b, 1)[..32]);
            }
        }
        for _ in 0..2048 {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let a = state;
            state = state.rotate_left(37).wrapping_add(1);
            let b = state;
            let value = Wide::product(a, b);
            assert_eq!(bytes(value), reference(a, b, 1)[..32]);
            for factor in [0, 1, 2, u64::MAX] {
                let expected = reference(a, b, factor);
                match value.times(factor) {
                    Ok(value) => {
                        assert!(expected[32..].iter().all(|v| *v == 0));
                        assert_eq!(bytes(value), expected[..32]);
                    }
                    Err(FieldError::Arithmetic) => assert!(expected[32..].iter().any(|v| *v != 0)),
                    other => panic!("Unexpected product result: {other:?}"),
                }
            }
        }
    }
    #[test]
    fn radical_comparison_keeps_products_wider_than_u128() {
        let p = 1i128 << 80;
        let q = 1i128 << 79;
        assert_eq!(radical_sign(p, -q, 3), Ok(1));
        assert_eq!(radical_sign(p, -q, 4), Ok(0));
        assert_eq!(radical_sign(p, -q, 5), Ok(-1));
        assert_eq!(radical_sign(-p, q, 3), Ok(-1));
        assert_eq!(radical_sign(-p, q, 5), Ok(1));
        assert!(Wide::product(u128::MAX, u128::MAX).times(2).is_err());
    }
}
