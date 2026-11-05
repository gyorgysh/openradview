// SPDX-License-Identifier: GPL-3.0-or-later
//! Projection-critical points of the exact trilinear zero set. A point is not
//! itself a connection decision; contours and singular sections still matter.
use super::*;
mod graph;
pub use graph::{
    AnchorLine, ContourNode, ContourSegment, CriticalGraph, ExactCoordinate, FaceArrangement,
    GraphEdge, GraphNode, WholeSection,
};

// For corner magnitudes <=255, each section coefficient has magnitude <=520200.
// The bound also keeps sign tests with arbitrary i32 linear forms inside i128.
const MAX_COEFFICIENT: u64 = 1 << 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Root {
    Rational(Rational),
    Surd { a: i32, b: i32, c: i32, upper: bool },
}
/// A real root of a bounded integer quadratic, without rounding its sign tests.
/// Irrational roots retain their defining polynomial and ordered branch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AlgebraicRoot(Root);
impl AlgebraicRoot {
    pub fn rational(self) -> Option<Rational> {
        match self.0 {
            Root::Rational(value) => Some(value),
            Root::Surd { .. } => None,
        }
    }
    /// Exact sign of constant + slope*t; the result is -1, 0 or 1.
    pub fn sign_linear(self, constant: i32, slope: i32) -> i8 {
        let constant = i128::from(constant);
        let slope = i128::from(slope);
        match self.0 {
            Root::Rational(value) => (constant * i128::from(value.denominator)
                + slope * i128::from(value.numerator))
            .signum() as i8,
            Root::Surd { a, b, c, upper, .. } => {
                let x = 2 * i128::from(a) * constant - i128::from(b) * slope;
                let y = if upper { slope } else { -slope };
                if x == 0 {
                    return y.signum() as i8;
                }
                if y == 0 || x.signum() == y.signum() {
                    return x.signum() as i8;
                }
                match (x * x).cmp(&(y * y * discriminant(a, b, c))) {
                    std::cmp::Ordering::Greater => x.signum() as i8,
                    std::cmp::Ordering::Less => y.signum() as i8,
                    std::cmp::Ordering::Equal => 0,
                }
            }
        }
    }
    fn approximate_linear(self, constant: i32, slope: i32) -> f64 {
        match self.0 {
            Root::Rational(value) => {
                (i128::from(constant) * i128::from(value.denominator)
                    + i128::from(slope) * i128::from(value.numerator)) as f64
                    / value.denominator as f64
            }
            Root::Surd { a, b, c, upper } => {
                let x =
                    2 * i128::from(a) * i128::from(constant) - i128::from(b) * i128::from(slope);
                let y = if upper {
                    i128::from(slope)
                } else {
                    -i128::from(slope)
                };
                let d = discriminant(a, b, c);
                let radical = (d as f64).sqrt();
                // The conjugate has like-signed terms when the original terms
                // oppose. Keep its exact numerator before converting to f64.
                let value = if x != 0 && y != 0 && x.signum() != y.signum() {
                    (x * x - y * y * d) as f64 / (x as f64 - y as f64 * radical)
                } else {
                    x as f64 + y as f64 * radical
                };
                value / (2. * f64::from(a))
            }
        }
    }
    /// Display/geometry approximation only. It must not select a topology.
    pub fn approximate(self) -> f64 {
        match self.0 {
            Root::Rational(value) => value.as_f64(),
            Root::Surd { a, b, c, upper } => {
                let radical = (discriminant(a, b, c) as f64).sqrt();
                let q = -0.5 * (b as f64 + if b >= 0 { radical } else { -radical });
                if upper == (b < 0) {
                    q / a as f64
                } else {
                    c as f64 / q
                }
            }
        }
    }
}

fn discriminant(a: i32, b: i32, c: i32) -> i128 {
    i128::from(b) * i128::from(b) - 4 * i128::from(a) * i128::from(c)
}

enum Roots {
    Finite([Option<AlgebraicRoot>; 2]),
    All,
}
fn roots(polynomial: SectionDeterminant) -> Result<Roots, FieldError> {
    let mut values = [polynomial.quadratic, polynomial.linear, polynomial.constant];
    if values.iter().any(|v| v.unsigned_abs() > MAX_COEFFICIENT) {
        return Err(FieldError::Arithmetic);
    }
    let mut divisor = 0;
    for value in values {
        let mut b = value.unsigned_abs();
        while b != 0 {
            (divisor, b) = (b, divisor % b);
        }
    }
    if divisor == 0 {
        return Ok(Roots::All);
    }
    for value in &mut values {
        *value /= divisor as i64;
    }
    let [mut a, mut b, mut c] = values;
    let rational = |n, d| Rational::new(n, d).map(|value| AlgebraicRoot(Root::Rational(value)));
    if a == 0 {
        return Ok(Roots::Finite(if b == 0 {
            [None, None]
        } else {
            [Some(rational(-i128::from(c), i128::from(b))?), None]
        }));
    }
    if a < 0 {
        (a, b, c) = (-a, -b, -c);
    }
    let discriminant = i128::from(b) * i128::from(b) - 4 * i128::from(a) * i128::from(c);
    if discriminant < 0 {
        return Ok(Roots::Finite([None, None]));
    }
    let discriminant = u64::try_from(discriminant).map_err(|_| FieldError::Arithmetic)?;
    let square_root = discriminant.isqrt();
    if square_root * square_root == discriminant {
        let lower = rational(-i128::from(b) - i128::from(square_root), 2 * i128::from(a))?;
        let upper = if discriminant == 0 {
            None
        } else {
            Some(rational(
                -i128::from(b) + i128::from(square_root),
                2 * i128::from(a),
            )?)
        };
        Ok(Roots::Finite([Some(lower), upper]))
    } else {
        Ok(Roots::Finite([false, true].map(|upper| {
            Some(AlgebraicRoot(Root::Surd {
                a: a as i32,
                b: b as i32,
                c: c as i32,
                upper,
            }))
        })))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Linear {
    constant: i32,
    slope: i32,
}
impl Linear {
    fn sign(self, at: AlgebraicRoot) -> i8 {
        at.sign_linear(self.constant, self.slope)
    }
    fn scaled(self, at: Rational) -> i128 {
        i128::from(self.constant) * i128::from(at.denominator)
            + i128::from(self.slope) * i128::from(at.numerator)
    }
    fn approximate(self, at: AlgebraicRoot) -> f64 {
        at.approximate_linear(self.constant, self.slope)
    }
    fn subtract(self, other: Self) -> Self {
        Self {
            constant: self.constant - other.constant,
            slope: self.slope - other.slope,
        }
    }
}

/// A zero-set point with both derivatives transverse to `axis` exactly zero.
/// Its construction has proved that every coordinate is in the closed unit cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProjectionPoint {
    axis: u8,
    parameter: AlgebraicRoot,
    numerators: [Linear; 2],
    denominator: Linear,
    normal_sign: i8,
    boundary: u8,
}
impl ProjectionPoint {
    pub fn axis(self) -> usize {
        usize::from(self.axis)
    }
    pub fn parameter(self) -> AlgebraicRoot {
        self.parameter
    }
    /// Sign of the remaining derivative in this cell's trilinear extension.
    /// Zero marks a singular point there; nonzero marks a regular projected saddle.
    /// Boundary points still need the incident cells' connections checked.
    pub fn normal_sign(self) -> i8 {
        self.normal_sign
    }
    /// Bit 2*a marks coordinate a=0; bit 2*a+1 marks coordinate a=1.
    pub fn boundary(self) -> u8 {
        self.boundary
    }
    pub fn rational_point(self) -> Option<[Rational; 3]> {
        let t = self.parameter.rational()?;
        let mut point = [t; 3];
        let denominator = self.denominator.scaled(t);
        for (axis, numerator) in transverse_axes(self.axis())
            .into_iter()
            .zip(self.numerators)
        {
            point[axis] = Rational::new(numerator.scaled(t), denominator).ok()?;
        }
        Some(point)
    }
    /// Returns no coordinate if rounding cannot represent the proved unit-cell
    /// point. Exact boundary contacts snap only after exact equality was proved.
    pub fn approximate_point(self) -> Option<[f64; 3]> {
        if let Some(point) = self.rational_point() {
            return Some(point.map(Rational::as_f64));
        }
        let t = self.parameter.approximate();
        let denominator = self.denominator.approximate(self.parameter);
        if denominator == 0. {
            return None;
        }
        let mut point = [t; 3];
        for (axis, numerator) in transverse_axes(self.axis())
            .into_iter()
            .zip(self.numerators)
        {
            point[axis] = numerator.approximate(self.parameter) / denominator;
        }
        for (axis, value) in point.iter_mut().enumerate() {
            if self.boundary & (1 << (2 * axis)) != 0 {
                *value = 0.;
            } else if self.boundary & (1 << (2 * axis + 1)) != 0 {
                *value = 1.;
            }
        }
        point
            .into_iter()
            .all(|v| (0. ..=1.).contains(&v))
            .then_some(point)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProjectionAnalysis {
    Finite {
        points: [Option<ProjectionPoint>; 2],
        /// The complete bilinear section is zero, rather than one isolated point.
        whole_sections: [Option<AlgebraicRoot>; 2],
    },
    /// The section determinant vanishes identically. This can represent a
    /// singular family, a regular plane or no zero set; it is not an empty result.
    Degenerate,
}

/// Contours immediately inside one cell. These connections do not replace the
/// exact face's geometry, including its shared crossings, vertices or zero edges.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LimitContours {
    Empty,
    Arc([u8; 2]),
    Split {
        arcs: [[u8; 2]; 2],
        joined_corners: [u8; 2],
    },
    /// A zero corner/edge or saddle persists into the cell and needs explicit
    /// singular geometry. It must not be discarded as an empty contour.
    Critical,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FaceInteriorLimit {
    /// Cyclic order 00,10,11,01 on ascending tangential axes.
    pub corner_signs: [i8; 4],
    pub determinant_sign: i8,
    pub contours: LimitContours,
}
fn transverse_axes(axis: usize) -> [usize; 2] {
    [usize::from(axis == 0), if axis == 2 { 1 } else { 2 }]
}
fn boundary(axis: usize, lower: i8, upper: i8) -> u8 {
    (u8::from(lower == 0) << (2 * axis)) | (u8::from(upper == 0) << (2 * axis + 1))
}
impl Trilinear {
    /// Exact one-sided limit of parallel sections approaching a face from the
    /// cell interior. Leading integer coefficients replace a finite epsilon.
    pub fn face_interior_limit(
        self,
        axis: usize,
        upper: bool,
    ) -> Result<FaceInteriorLimit, FieldError> {
        let face = self.face(axis, upper).ok_or(FieldError::Bounds)?.values;
        let opposite = self.face(axis, !upper).ok_or(FieldError::Bounds)?.values;
        let corner_signs = std::array::from_fn(|i| {
            if face[i] == 0 {
                opposite[i].signum() as i8
            } else {
                face[i].signum() as i8
            }
        });
        let SectionDeterminant {
            quadratic: a,
            linear: b,
            constant: c,
        } = self.section_determinant(axis).ok_or(FieldError::Bounds)?;
        let coefficients = if upper {
            [a + b + c, -(2 * a + b), a]
        } else {
            [c, b, a]
        };
        let determinant_sign = coefficients
            .into_iter()
            .find(|v| *v != 0)
            .map_or(0, |v| v.signum() as i8);
        let mut crossing = [0; 4];
        let mut count = 0;
        for i in 0..4 {
            if corner_signs[i] != corner_signs[(i + 1) % 4] {
                crossing[count] = i as u8;
                count += 1;
            }
        }
        let contours = if corner_signs.contains(&0) {
            LimitContours::Critical
        } else {
            match (count, determinant_sign) {
                (0, _) => LimitContours::Empty,
                (2, _) => LimitContours::Arc([crossing[0], crossing[1]]),
                (4, 1) => LimitContours::Split {
                    arcs: [[0, 1], [2, 3]],
                    joined_corners: [0, 2],
                },
                (4, -1) => LimitContours::Split {
                    arcs: [[0, 3], [1, 2]],
                    joined_corners: [1, 3],
                },
                (4, 0) => LimitContours::Critical,
                _ => return Err(FieldError::Arithmetic),
            }
        };
        Ok(FaceInteriorLimit {
            corner_signs,
            determinant_sign,
            contours,
        })
    }
    /// Worker geometry analysis, with constant storage and no case table. The
    /// zero set and its two transverse derivatives give a section quadratic.
    /// Its roots still require bounds, denominator and degeneracy checks.
    pub fn projection_critical(self, axis: usize) -> Result<ProjectionAnalysis, FieldError> {
        let polynomial = self.section_determinant(axis).ok_or(FieldError::Bounds)?;
        let Roots::Finite(roots) = roots(polynomial)? else {
            return Ok(ProjectionAnalysis::Degenerate);
        };
        let lo = self
            .face(axis, false)
            .ok_or(FieldError::Bounds)?
            .values
            .map(i32::from);
        let hi = self
            .face(axis, true)
            .ok_or(FieldError::Bounds)?
            .values
            .map(i32::from);
        let linear = |f: fn([i32; 4]) -> i32| Linear {
            constant: f(lo),
            slope: f(hi) - f(lo),
        };
        let numerators = [linear(|v| v[0] - v[3]), linear(|v| v[0] - v[1])];
        let denominator = linear(|v| v[0] - v[1] + v[2] - v[3]);
        let origin = linear(|v| v[0]);
        let mut points = [None, None];
        let mut whole_sections = [None, None];
        for (index, parameter) in roots.into_iter().enumerate() {
            let Some(parameter) = parameter else { continue };
            let lower = parameter.sign_linear(0, 1);
            let upper = parameter.sign_linear(1, -1);
            if lower < 0 || upper < 0 {
                continue;
            }
            let sign = denominator.sign(parameter);
            if sign == 0 {
                if origin.sign(parameter) == 0 && numerators.iter().all(|n| n.sign(parameter) == 0)
                {
                    whole_sections[index] = Some(parameter);
                }
                continue;
            }
            let mut mask = boundary(axis, lower, upper);
            let mut inside = true;
            for (a, numerator) in transverse_axes(axis).into_iter().zip(numerators) {
                let lower = numerator.sign(parameter) * sign;
                let upper = denominator.subtract(numerator).sign(parameter) * sign;
                if lower < 0 || upper < 0 {
                    inside = false;
                    break;
                }
                mask |= boundary(a, lower, upper);
            }
            if !inside {
                continue;
            }
            points[index] = Some(ProjectionPoint {
                axis: axis as u8,
                parameter,
                numerators,
                denominator,
                // Bounded corner values make these derivative coefficients fit i32.
                normal_sign: parameter
                    .sign_linear(polynomial.linear as i32, (2 * polynomial.quadratic) as i32)
                    * sign,
                boundary: mask,
            });
        }
        Ok(ProjectionAnalysis::Finite {
            points,
            whole_sections,
        })
    }
}

#[cfg(test)]
mod tests;
