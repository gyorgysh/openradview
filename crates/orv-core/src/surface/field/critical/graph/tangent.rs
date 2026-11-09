// SPDX-License-Identifier: GPL-3.0-or-later
//! Exact ordering of axial and face-contour tangents at a regular zero point.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TangentRay {
    /// Signed lattice axis. Its field derivative must be zero.
    Axis { axis: u8, positive: bool },
    /// Signed (outward surface normal cross positive lattice axis).
    /// The corresponding face gradient must be nonzero.
    Face { axis: u8, positive: bool },
}
impl TangentRay {
    fn parts(self) -> (usize, i8) {
        let (Self::Axis { axis, positive } | Self::Face { axis, positive }) = self;
        (usize::from(axis), if positive { 1 } else { -1 })
    }
    pub fn reversed(self) -> Self {
        match self {
            Self::Axis { axis, positive } => Self::Axis {
                axis,
                positive: !positive,
            },
            Self::Face { axis, positive } => Self::Face {
                axis,
                positive: !positive,
            },
        }
    }
}
/// A regular tangent-plane context using exact derivative signs. The outward
/// normal of F>=0 is -grad F. Directions do not establish edge/node identity,
/// curve curvature or inside-cell sector ownership.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RegularTangents {
    normal: [i8; 3],
}
impl RegularTangents {
    pub fn new(gradient_signs: [i8; 3]) -> Result<Self, FieldError> {
        if gradient_signs == [0; 3] || gradient_signs.into_iter().any(|s| !(-1..=1).contains(&s)) {
            return Err(FieldError::Arithmetic);
        }
        Ok(Self {
            normal: gradient_signs.map(|s| -s),
        })
    }
    fn validate(self, ray: TangentRay) -> Result<(), FieldError> {
        let (axis, _) = ray.parts();
        if axis >= 3 {
            return Err(FieldError::Bounds);
        }
        let valid = match ray {
            TangentRay::Axis { .. } => self.normal[axis] == 0,
            TangentRay::Face { .. } => (0..3).any(|a| a != axis && self.normal[a] != 0),
        };
        if valid {
            Ok(())
        } else {
            Err(FieldError::Arithmetic)
        }
    }
    /// Ascending face coordinates use (-Fv,Fu). A singular face cross must
    /// retain its axial branches instead of a vanishing regular face tangent.
    pub fn ascending_face(self, axis: u8, forward: bool) -> Result<TangentRay, FieldError> {
        let ray = TangentRay::Face {
            axis,
            positive: forward == (axis != 1),
        };
        self.validate(ray)?;
        Ok(ray)
    }
    /// Positive cell-surface boundary direction, with the cell interior on
    /// its left. This supplies a direction, without choosing a complete sector.
    pub fn cell_boundary(self, axis: u8, upper: bool) -> Result<TangentRay, FieldError> {
        let ray = TangentRay::Face {
            axis,
            positive: upper,
        };
        self.validate(ray)?;
        Ok(ray)
    }
    fn dot(self, a: TangentRay, b: TangentRay) -> i8 {
        let (i, sa) = a.parts();
        let (j, sb) = b.parts();
        let value = match (a, b) {
            (TangentRay::Axis { .. }, TangentRay::Axis { .. }) => i8::from(i == j),
            (TangentRay::Axis { .. }, TangentRay::Face { .. }) => {
                axes(i, j).map_or(0, |(k, parity)| -parity * self.normal[k])
            }
            (TangentRay::Face { .. }, TangentRay::Axis { .. }) => return self.dot(b, a),
            (TangentRay::Face { .. }, TangentRay::Face { .. }) => {
                if i == j {
                    1
                } else {
                    -self.normal[i] * self.normal[j]
                }
            }
        };
        value * sa * sb
    }
    fn cross(self, a: TangentRay, b: TangentRay) -> i8 {
        let (i, sa) = a.parts();
        let (j, sb) = b.parts();
        // Positive norm factors cancel: face/face cross-dot-n has sign
        // epsilon_ijk*n_k. With n_i=0, ei cross (n cross ej) dotted with n
        // is |n|^2*delta_ij. No derivative magnitude is required.
        let value = match (a, b) {
            (TangentRay::Axis { .. }, TangentRay::Axis { .. })
            | (TangentRay::Face { .. }, TangentRay::Face { .. }) => {
                axes(i, j).map_or(0, |(k, parity)| parity * self.normal[k])
            }
            (TangentRay::Axis { .. }, TangentRay::Face { .. }) => i8::from(i == j),
            (TangentRay::Face { .. }, TangentRay::Axis { .. }) => return -self.cross(b, a),
        };
        value * sa * sb
    }
    pub fn dot_sign(self, a: TangentRay, b: TangentRay) -> Result<i8, FieldError> {
        self.validate(a)?;
        self.validate(b)?;
        Ok(self.dot(a, b))
    }
    /// Sign of (a cross b) dot the outward normal, without a magnitude or angle.
    pub fn cross_sign(self, a: TangentRay, b: TangentRay) -> Result<i8, FieldError> {
        self.validate(a)?;
        self.validate(b)?;
        Ok(self.cross(a, b))
    }
    fn half(self, reference: TangentRay, ray: TangentRay) -> Result<u8, FieldError> {
        match (self.cross(reference, ray), self.dot(reference, ray)) {
            (1, _) | (0, 1) => Ok(0),
            (-1, _) | (0, -1) => Ok(1),
            _ => Err(FieldError::Arithmetic),
        }
    }
    /// Counterclockwise order starting at reference. Equal means the same
    /// direction, possibly with a different representation. Callers must prove
    /// geometric identity before merging or creating half-edges at such a tie.
    pub fn compare(
        self,
        reference: TangentRay,
        a: TangentRay,
        b: TangentRay,
    ) -> Result<Ordering, FieldError> {
        for ray in [reference, a, b] {
            self.validate(ray)?;
        }
        let half = self.half(reference, a)?.cmp(&self.half(reference, b)?);
        if half != Ordering::Equal {
            return Ok(half);
        }
        let turn = self.cross(a, b);
        if turn == 0 && self.dot(a, b) != 1 {
            return Err(FieldError::Arithmetic);
        }
        Ok(0.cmp(&turn))
    }
}
fn axes(i: usize, j: usize) -> Option<(usize, i8)> {
    (i != j).then(|| (3 - i - j, if (i + 1) % 3 == j { 1 } else { -1 }))
}

#[cfg(test)]
mod tests;
