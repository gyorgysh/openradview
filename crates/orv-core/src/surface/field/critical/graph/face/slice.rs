// SPDX-License-Identifier: GPL-3.0-or-later
//! Exact zero-set intersections with a shared source-coordinate slice.
use super::*;

/// The zero set on one closed unit-square slice. Whole retains a line, including
/// factor/whole-field cases; it must not be replaced by an arbitrary point.
#[derive(Clone, Copy, Debug)]
pub enum BilinearSlice {
    Empty,
    Whole,
    Point(ExactCoordinate),
}
impl Bilinear {
    /// Fix u (axis0) or v (axis1) and solve for the other coordinate. Rational
    /// slices are checked against retained i64/u64 ratio limits. Irrational
    /// slices accept original source forms only, so transferring the result
    /// again cannot silently grow the proved expression bounds.
    pub fn zero_slice(
        self,
        axis: usize,
        fixed: ExactCoordinate,
    ) -> Result<BilinearSlice, FieldError> {
        let [a, b, c, d] = coefficients(self);
        let (constant, slope) = match axis {
            0 => ([a, b], [c, d]),
            1 => ([a, c], [b, d]),
            _ => return Err(FieldError::Bounds),
        };
        if let Some(at) = fixed.rational_value() {
            let evaluate = |[p, q]: [i32; 2]| {
                i128::from(p) * i128::from(at.denominator)
                    + i128::from(q) * i128::from(at.numerator)
            };
            let (n, denominator) = (-evaluate(constant), evaluate(slope));
            if denominator == 0 {
                return Ok(if n == 0 {
                    BilinearSlice::Whole
                } else {
                    BilinearSlice::Empty
                });
            }
            // An out-of-square zero needs no retained ratio, even when that
            // unnecessary ratio would exceed the coordinate storage bound.
            if n.signum() * denominator.signum() < 0
                || (denominator - n).signum() * denominator.signum() < 0
            {
                return Ok(BilinearSlice::Empty);
            }
            return Ok(BilinearSlice::Point(
                ExactCoordinate::from_rational(Rational::new(n, denominator)?)?
                    .ok_or(FieldError::Arithmetic)?,
            ));
        }
        if [fixed.numerator, fixed.denominator].into_iter().any(|f| {
            f.constant.unsigned_abs() > SOURCE_FORM || f.slope.unsigned_abs() > SOURCE_FORM
        }) {
            return Err(FieldError::Arithmetic);
        }
        let combine = |[p, q]: [i32; 2]| -> Result<Linear, FieldError> {
            let coefficient = |n: i32, denominator: i32| {
                i32::try_from(i64::from(p) * i64::from(denominator) + i64::from(q) * i64::from(n))
                    .map_err(|_| FieldError::Arithmetic)
            };
            Ok(Linear {
                constant: coefficient(fixed.numerator.constant, fixed.denominator.constant)?,
                slope: coefficient(fixed.numerator.slope, fixed.denominator.slope)?,
            })
        };
        let n = negative(combine(constant)?);
        let denominator = combine(slope)?;
        if denominator.sign(fixed.parameter) == 0 {
            return Ok(if n.sign(fixed.parameter) == 0 {
                BilinearSlice::Whole
            } else {
                BilinearSlice::Empty
            });
        }
        Ok(ExactCoordinate::unit(fixed.parameter, n, denominator)?
            .map_or(BilinearSlice::Empty, BilinearSlice::Point))
    }
}

#[cfg(test)]
mod tests;
