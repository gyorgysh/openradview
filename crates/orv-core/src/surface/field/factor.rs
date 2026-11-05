// SPDX-License-Identifier: GPL-3.0-or-later
//! Exact disjoint-axis factors and inclusive half-level cell phases.
use super::*;

/// Selection uses F>=0, matching retained fractional half-maximum ties.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CellPhase {
    /// Every point in the closed cell is selected, including a whole zero cell.
    Selected,
    /// Every point in the closed cell is strictly below the selected level.
    Unselected,
    /// Strict interior is unselected; selected points occupy zero boundary strata.
    BoundaryOnly,
    /// Positive and negative samples force an interface inside the cell.
    Crossing,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ZeroBoundary {
    /// Corner bits use x+2*y+4*z.
    pub vertices: u8,
    /// Bit4*axis+offset; offset uses the two ascending transverse-axis bits.
    pub edges: u16,
    /// Bit2*axis+upper.
    pub faces: u8,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResidualKind {
    Whole,
    Constant,
    Bilinear,
    Trilinear,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AxisFactor {
    axis: u8,
    values: [i16; 2],
}
impl AxisFactor {
    pub fn axis(self) -> usize {
        usize::from(self.axis)
    }
    /// Primitive integer endpoint values; first nonzero endpoint is positive.
    pub fn values(self) -> [i16; 2] {
        self.values
    }
    /// An exact zero plane only when it intersects the closed unit cell.
    pub fn zero(self) -> Result<Option<Rational>, FieldError> {
        let [a, b] = self.values;
        let t = Rational::new(i128::from(a), i128::from(a) - i128::from(b))?;
        Ok((t.numerator >= 0 && i128::from(t.numerator) <= i128::from(t.denominator)).then_some(t))
    }
    /// Sign at an exact coordinate, including a boundary plane without an epsilon.
    pub fn sign_at(self, position: ExactCoordinate) -> Result<i8, FieldError> {
        let Some(zero) = self.zero()? else {
            return Ok(1);
        };
        let zero = ExactCoordinate::from_rational(zero)?.ok_or(FieldError::Arithmetic)?;
        let order = position.compare(zero)?;
        let sign = match order {
            std::cmp::Ordering::Less => -1,
            std::cmp::Ordering::Equal => 0,
            std::cmp::Ordering::Greater => 1,
        };
        Ok(sign * (self.values[1] - self.values[0]).signum() as i8)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellResidual {
    field: Trilinear,
    axes: u8,
}
impl CellResidual {
    pub fn field(self) -> Trilinear {
        self.field
    }
    /// Global axes retained by the residual; removed factor axes are independent.
    pub fn dependent_axes(self) -> u8 {
        self.axes
    }
    pub fn kind(self) -> ResidualKind {
        match self.axes.count_ones() {
            0 => {
                if self.field.values[0] == 0 {
                    ResidualKind::Whole
                } else {
                    ResidualKind::Constant
                }
            }
            2 => ResidualKind::Bilinear,
            _ => ResidualKind::Trilinear,
        }
    }
    /// An irreducible bilinear field on its two ascending source-cell axes.
    /// Extrusion along the third axis remains a surface-construction step.
    pub fn bilinear(self) -> Option<([usize; 2], Bilinear)> {
        if self.axes.count_ones() != 2 {
            return None;
        }
        let missing = (0..3).find(|axis| self.axes & (1 << axis) == 0)?;
        Some((transverse(missing), self.field.face(missing, false)?))
    }
}
/// Certified product of nonconstant axis factors and one irreducible residual.
/// Zero sets still need phase-aware patch construction, intersections, shared
/// boundary ownership and finite-grid policy before triangulation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellFactorization {
    original: Trilinear,
    factors: [Option<AxisFactor>; 3],
    residual: CellResidual,
}
impl CellFactorization {
    pub fn original(self) -> Trilinear {
        self.original
    }
    pub fn factors(&self) -> impl Iterator<Item = &AxisFactor> {
        self.factors.iter().flatten()
    }
    pub fn residual(self) -> CellResidual {
        self.residual
    }
    /// Exact corner reconstruction certifies the complete multi-affine identity.
    pub fn reconstruct_values(self) -> Result<[i16; 8], FieldError> {
        let mut values = [0; 8];
        for (corner, out) in values.iter_mut().enumerate() {
            let mut value = i64::from(self.residual.field.values[corner]);
            for factor in self.factors() {
                value = value
                    .checked_mul(i64::from(factor.values[(corner >> factor.axis()) & 1]))
                    .ok_or(FieldError::Arithmetic)?;
            }
            *out = i16::try_from(value).map_err(|_| FieldError::Arithmetic)?;
        }
        Ok(values)
    }
}
fn transverse(axis: usize) -> [usize; 2] {
    [usize::from(axis == 0), if axis == 2 { 1 } else { 2 }]
}
fn check_cancel(cancel: &impl Fn() -> bool) -> Result<(), FieldError> {
    if cancel() {
        Err(FieldError::Cancelled)
    } else {
        Ok(())
    }
}
struct AxisDivision {
    factor: [i16; 2],
    quotient: [i16; 4],
}
fn axis_factor(field: Trilinear, axis: usize) -> Result<Option<AxisDivision>, FieldError> {
    let lo = field.face(axis, false).ok_or(FieldError::Bounds)?.values;
    let hi = field.face(axis, true).ok_or(FieldError::Bounds)?.values;
    let Some(anchor) = (0..4).find(|i| lo[*i] != 0 || hi[*i] != 0) else {
        return Ok(None);
    };
    let (mut a, mut b) = (lo[anchor].unsigned_abs(), hi[anchor].unsigned_abs());
    while b != 0 {
        (a, b) = (b, a % b);
    }
    if a == 0 {
        return Err(FieldError::Arithmetic);
    }
    let mut factor = [lo[anchor] / a as i16, hi[anchor] / a as i16];
    if factor.into_iter().find(|v| *v != 0).is_some_and(|v| v < 0) {
        factor = factor.map(|v| -v);
    }
    if (0..4)
        .any(|i| i32::from(lo[i]) * i32::from(factor[1]) != i32::from(hi[i]) * i32::from(factor[0]))
    {
        return Ok(None);
    }
    let mut quotient = [0; 4];
    for i in 0..4 {
        let (value, divisor) = if factor[0] != 0 {
            (lo[i], factor[0])
        } else {
            (hi[i], factor[1])
        };
        // Primitive proportional rows have integer quotients by Bezout's identity.
        if value % divisor != 0 {
            return Err(FieldError::Arithmetic);
        }
        quotient[i] = value / divisor;
    }
    Ok(Some(AxisDivision { factor, quotient }))
}
impl Trilinear {
    pub fn phase(self) -> CellPhase {
        let positive = self.values.iter().any(|v| *v > 0);
        let negative = self.values.iter().any(|v| *v < 0);
        if !negative {
            CellPhase::Selected
        } else if positive {
            CellPhase::Crossing
        } else if self.values.contains(&0) {
            CellPhase::BoundaryOnly
        } else {
            CellPhase::Unselected
        }
    }
    /// All-zero boundary strata. For BoundaryOnly cells this is the complete
    /// selected set: open-face tensor weights are strictly positive, so a
    /// nonpositive field can vanish there only when every incident corner is zero.
    pub fn zero_boundary(self) -> ZeroBoundary {
        let mut result = ZeroBoundary {
            vertices: 0,
            edges: 0,
            faces: 0,
        };
        for (i, value) in self.values.into_iter().enumerate() {
            result.vertices |= u8::from(value == 0) << i;
        }
        for axis in 0..3 {
            let [u, v] = transverse(axis);
            for offset in 0..4 {
                let lower = ((offset & 1) << u) | ((offset >> 1) << v);
                if self.values[lower] == 0 && self.values[lower | (1 << axis)] == 0 {
                    result.edges |= 1 << (4 * axis + offset);
                }
            }
            for upper in [false, true] {
                if self.face(axis, upper).is_some_and(|f| f.values == [0; 4]) {
                    result.faces |= 1 << (2 * axis + usize::from(upper));
                }
            }
        }
        result
    }
    /// Exact worker-side separation of disjoint-axis factors. Multiaffine
    /// polynomials cannot share a variable between nonconstant factors, since
    /// degrees add in an integral domain. Any reducible three-variable field
    /// therefore has a one-axis factor, detected by a rank-one 2x4 flattening.
    pub fn factorization(self, cancel: impl Fn() -> bool) -> Result<CellFactorization, FieldError> {
        check_cancel(&cancel)?;
        let mut current = self;
        let mut factors = [None; 3];
        if self.values != [0; 8] {
            for (axis, slot) in factors.iter_mut().enumerate() {
                check_cancel(&cancel)?;
                let Some(AxisDivision {
                    factor: values,
                    quotient,
                }) = axis_factor(current, axis)?
                else {
                    continue;
                };
                if values[0] != values[1] {
                    *slot = Some(AxisFactor {
                        axis: axis as u8,
                        values,
                    });
                }
                let indices = face_indices(axis, false).ok_or(FieldError::Bounds)?;
                let mut residual = [0; 8];
                for (index, value) in indices.into_iter().zip(quotient) {
                    residual[index] = value;
                    residual[index | (1 << axis)] = value;
                }
                current = Self::new(residual)?;
            }
        }
        let mut axes = 0u8;
        for axis in 0..3 {
            if current.face(axis, false) != current.face(axis, true) {
                axes |= 1 << axis;
            }
        }
        if axes.count_ones() == 1 {
            return Err(FieldError::Arithmetic);
        }
        let residual = CellResidual {
            field: current,
            axes,
        };
        if let Some((_, field)) = residual.bilinear() {
            let [a, b, c, d] = field.values.map(i32::from);
            if a * c == b * d {
                return Err(FieldError::Arithmetic);
            }
        }
        if residual.kind() == ResidualKind::Trilinear {
            for axis in 0..3 {
                let q = current
                    .section_determinant(axis)
                    .ok_or(FieldError::Bounds)?;
                if q.quadratic == 0 && q.linear == 0 && q.constant == 0 {
                    return Err(FieldError::Arithmetic);
                }
            }
        }
        let result = CellFactorization {
            original: self,
            factors,
            residual,
        };
        if result.reconstruct_values()? != self.values {
            return Err(FieldError::Arithmetic);
        }
        check_cancel(&cancel)?;
        Ok(result)
    }
}

#[cfg(test)]
mod tests;
