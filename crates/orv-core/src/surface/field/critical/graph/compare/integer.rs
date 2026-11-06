// SPDX-License-Identifier: GPL-3.0-or-later
//! Checked signed magnitude arithmetic for two-radical coordinate comparisons.
use super::*;

const LIMBS: usize = 16;
// Root coefficients <=2^20 give d<2^43. Original linear forms have rational
// coefficients <2^33 and radical coefficients <2^11. Rationalization therefore
// gives |P|,R<2^67 and |Q|<2^45. Cross-coordinate coefficients satisfy
// |A|<2^135, |B|,|C|<2^112. The final sign test squares a term <2^272,
// fitting in 544 bits. 1024 bits also cover unreduced rational inputs
// (i64/u64) and leave arithmetic failure explicit instead of truncating it.
// A bilinear sign with coefficients <2^11 has numerator
// (L+M sqrt(d))+(N+O sqrt(d))*sqrt(e), where |L|<2^147,
// |M|,|N|<2^124, |O|<2^101. Opposing-term squaring gives a rational
// coefficient <2^296 and a radical coefficient <2^273. The final squared
// comparison fits592bits, also within this fixed width.
// Allowing polynomial coefficients <=2^20 adds at most10bits to L/M/N/O
// and40bits to the final comparison, still below640bits. Coordinate forms
// and their rationalization bounds remain unchanged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct FixedInteger<const N: usize> {
    limbs: [u64; N],
    sign: i8,
}
pub(super) type Integer = FixedInteger<LIMBS>;
impl<const N: usize> FixedInteger<N> {
    pub fn new(value: i128) -> Self {
        let magnitude = value.unsigned_abs();
        let mut limbs = [0; N];
        limbs[0] = magnitude as u64;
        limbs[1] = (magnitude >> 64) as u64;
        Self {
            limbs,
            sign: value.signum() as i8,
        }
    }
    fn with_sign(limbs: [u64; N], sign: i8) -> Self {
        Self {
            limbs,
            sign: if limbs.iter().all(|v| *v == 0) {
                0
            } else {
                sign
            },
        }
    }
    pub fn sign(self) -> i8 {
        self.sign
    }
    pub fn negative(self) -> Self {
        Self {
            sign: -self.sign,
            ..self
        }
    }
    pub fn compare_magnitude(self, other: Self) -> Ordering {
        self.limbs
            .into_iter()
            .rev()
            .cmp(other.limbs.into_iter().rev())
    }
    pub fn sum(self, other: Self) -> Result<Self, FieldError> {
        if self.sign == 0 {
            return Ok(other);
        }
        if other.sign == 0 {
            return Ok(self);
        }
        let mut limbs = [0; N];
        if self.sign == other.sign {
            let mut carry = 0u128;
            for ((output, left), right) in limbs.iter_mut().zip(self.limbs).zip(other.limbs) {
                let value = u128::from(left) + u128::from(right) + carry;
                *output = value as u64;
                carry = value >> 64;
            }
            if carry != 0 {
                return Err(FieldError::Arithmetic);
            }
            return Ok(Self::with_sign(limbs, self.sign));
        }
        let (larger, smaller) = if self.compare_magnitude(other) == Ordering::Less {
            (other, self)
        } else {
            (self, other)
        };
        let mut borrow = false;
        for ((output, left), right) in limbs.iter_mut().zip(larger.limbs).zip(smaller.limbs) {
            let (value, first) = left.overflowing_sub(right);
            let (value, second) = value.overflowing_sub(u64::from(borrow));
            *output = value;
            borrow = first || second;
        }
        Ok(Self::with_sign(limbs, larger.sign))
    }
    pub fn difference(self, other: Self) -> Result<Self, FieldError> {
        self.sum(other.negative())
    }
    pub fn times(self, value: u64) -> Result<Self, FieldError> {
        self.product(Self::new(i128::from(value)))
    }
    pub fn product(self, other: Self) -> Result<Self, FieldError> {
        let used = |limbs: &[u64; N]| limbs.iter().rposition(|v| *v != 0).map_or(0, |v| v + 1);
        let mut output = [0u64; N];
        let right_used = used(&other.limbs);
        for (i, left) in self.limbs.into_iter().take(used(&self.limbs)).enumerate() {
            let mut carry = 0;
            for (j, right) in other.limbs.into_iter().take(right_used).enumerate() {
                if i + j >= N {
                    if left != 0 && right != 0 || carry != 0 {
                        return Err(FieldError::Arithmetic);
                    }
                    continue;
                }
                // One limb product and two limb-sized addends fit u128.
                let value =
                    u128::from(left) * u128::from(right) + u128::from(output[i + j]) + carry;
                output[i + j] = value as u64;
                carry = value >> 64;
            }
            if carry != 0 {
                *output
                    .get_mut(i + right_used)
                    .ok_or(FieldError::Arithmetic)? = carry as u64;
            }
        }
        Ok(Self::with_sign(output, self.sign * other.sign))
    }
}

#[cfg(test)]
mod tests;
