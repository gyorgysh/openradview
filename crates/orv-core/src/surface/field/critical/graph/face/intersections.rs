// SPDX-License-Identifier: GPL-3.0-or-later
//! Exact intersections and common components of two clipped bilinear zero sets.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntersectionContact {
    Transverse,
    Tangent,
    Singular,
}
#[derive(Clone, Copy, Debug)]
pub struct IntersectionPoint {
    coordinates: [ExactCoordinate; 2],
    contact: IntersectionContact,
}
impl IntersectionPoint {
    pub fn coordinates(&self) -> &[ExactCoordinate; 2] {
        &self.coordinates
    }
    pub fn contact(&self) -> IntersectionContact {
        self.contact
    }
}
/// Common zero locus clipped to the closed unit square. A curve can meet the
/// square only at a corner; this type makes no dimension/manifold assumption.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommonZero {
    Whole,
    Axis {
        axis: u8,
        position: Rational,
    },
    /// Primitive corner values, with the first nonzero value positive.
    Curve(Bilinear),
}
pub struct BilinearIntersections {
    points: [Option<IntersectionPoint>; 4],
    count: usize,
    common: [Option<CommonZero>; 2],
    common_count: usize,
}
impl BilinearIntersections {
    /// Finite crossings/tangencies and joins on shared components. Domain
    /// endpoints of shared components remain the component's own constraints.
    pub fn points(&self) -> impl Iterator<Item = &IntersectionPoint> {
        self.points.iter().flatten()
    }
    pub fn common(&self) -> impl Iterator<Item = &CommonZero> {
        self.common.iter().flatten()
    }
    fn add_common(&mut self, component: CommonZero) -> Result<(), FieldError> {
        if self.common().any(|c| *c == component) {
            return Ok(());
        }
        *self
            .common
            .get_mut(self.common_count)
            .ok_or(FieldError::Capacity)? = Some(component);
        self.common_count += 1;
        Ok(())
    }
    fn add_point(
        &mut self,
        point: [ExactCoordinate; 2],
        fields: [Bilinear; 2],
        tangent: bool,
        cancel: &impl Fn() -> bool,
    ) -> Result<(), FieldError> {
        check_cancel(cancel)?;
        let mut singular = false;
        for field in fields {
            if field.sign_at(point)? != 0 {
                return Err(FieldError::Arithmetic);
            }
            let [_, b, c, d] = coefficients(field);
            singular |= bilinear_sign([b, 0, d, 0], point)? == 0
                && bilinear_sign([c, d, 0, 0], point)? == 0;
        }
        let contact = if singular {
            IntersectionContact::Singular
        } else if tangent {
            IntersectionContact::Tangent
        } else {
            IntersectionContact::Transverse
        };
        for prior in self.points.iter_mut().flatten() {
            check_cancel(cancel)?;
            if same_point(prior.coordinates, point)? {
                for (old, new) in prior.coordinates.iter_mut().zip(point) {
                    if representation(new) < representation(*old) {
                        *old = new;
                    }
                }
                if prior.contact != contact {
                    return Err(FieldError::Arithmetic);
                }
                return Ok(());
            }
        }
        let coordinates = point.map(|v| v.rational_value().map_or(Ok(v), rational_coordinate));
        let coordinates = [coordinates[0]?, coordinates[1]?];
        *self
            .points
            .get_mut(self.count)
            .ok_or(FieldError::Capacity)? = Some(IntersectionPoint {
            coordinates,
            contact,
        });
        self.count += 1;
        Ok(())
    }
    fn canonicalize(&mut self, cancel: &impl Fn() -> bool) -> Result<(), FieldError> {
        for i in 1..self.count {
            let mut j = i;
            while j > 0 {
                check_cancel(cancel)?;
                let a = self.points[j - 1]
                    .ok_or(FieldError::Arithmetic)?
                    .coordinates;
                let b = self.points[j].ok_or(FieldError::Arithmetic)?.coordinates;
                let first = a[0].compare(b[0])?;
                if first == Ordering::Less
                    || (first == Ordering::Equal && a[1].compare(b[1])? != Ordering::Greater)
                {
                    break;
                }
                self.points.swap(j - 1, j);
                j -= 1;
            }
        }
        // At most one common factor per axis. Both-factor cases use axis0 first.
        if let [
            Some(CommonZero::Axis { axis: 1, .. }),
            Some(CommonZero::Axis { axis: 0, .. }),
        ] = self.common
        {
            self.common.swap(0, 1);
        }
        Ok(())
    }
}
fn primitive(field: Bilinear) -> Bilinear {
    let mut gcd = 0u16;
    for value in field.values {
        let mut b = value.unsigned_abs();
        while b != 0 {
            (gcd, b) = (b, gcd % b);
        }
    }
    if gcd == 0 {
        return field;
    }
    let sign = field
        .values
        .into_iter()
        .find(|v| *v != 0)
        .map_or(1, i16::signum);
    Bilinear {
        values: field.values.map(|v| v / (gcd as i16) * sign),
    }
}
fn axis(axis: u8, n: i32, d: i32) -> Result<Option<CommonZero>, FieldError> {
    let position = Rational::new(-i128::from(n), i128::from(d))?;
    Ok((position.numerator >= 0
        && i128::from(position.numerator) <= i128::from(position.denominator))
    .then_some(CommonZero::Axis { axis, position }))
}
fn components(field: Bilinear) -> Result<[Option<CommonZero>; 2], FieldError> {
    let field = primitive(field);
    let [a, b, c, d] = coefficients(field);
    Ok(if [a, b, c, d] == [0; 4] {
        [Some(CommonZero::Whole), None]
    } else if d != 0 && i64::from(a) * i64::from(d) == i64::from(b) * i64::from(c) {
        [axis(0, c, d)?, axis(1, b, d)?]
    } else if d == 0 && c == 0 && b != 0 {
        [axis(0, a, b)?, None]
    } else if d == 0 && b == 0 && c != 0 {
        [axis(1, a, c)?, None]
    } else if (b == 0 && c == 0 && d == 0)
        || field.values.iter().all(|v| *v > 0)
        || field.values.iter().all(|v| *v < 0)
    {
        [None, None]
    } else {
        [Some(CommonZero::Curve(field)), None]
    })
}
fn axis_curve(
    axis: u8,
    position: Rational,
    field: Bilinear,
) -> Result<Option<[ExactCoordinate; 2]>, FieldError> {
    let [a, b, c, d] = coefficients(field);
    let parameter = AlgebraicRoot(Root::Rational(position));
    let (n, den) = if axis == 0 {
        ([a, b], [c, d])
    } else {
        ([a, c], [b, d])
    };
    let denominator = Linear {
        constant: den[0],
        slope: den[1],
    };
    if denominator.sign(parameter) == 0 {
        return Ok(None);
    }
    let Some(other) = ExactCoordinate::unit(
        parameter,
        Linear {
            constant: -n[0],
            slope: -n[1],
        },
        denominator,
    )?
    else {
        return Ok(None);
    };
    let fixed = rational_coordinate(position)?;
    Ok(Some(if axis == 0 {
        [fixed, other]
    } else {
        [other, fixed]
    }))
}
fn curve_pair(
    a: Bilinear,
    b: Bilinear,
    result: &mut BilinearIntersections,
    fields: [Bilinear; 2],
    cancel: &impl Fn() -> bool,
) -> Result<(), FieldError> {
    let (a, b) = if a.values <= b.values { (a, b) } else { (b, a) };
    if a == b {
        return result.add_common(CommonZero::Curve(a));
    }
    let [a0, a1, a2, a3] = coefficients(a).map(i64::from);
    let [b0, b1, b2, b3] = coefficients(b).map(i64::from);
    // Eliminate v: (a0+a1u)(b2+b3u)-(b0+b1u)(a2+a3u).
    // Corner bounds255 give each coefficient magnitude<=1040400<2^20.
    let polynomial = SectionDeterminant {
        constant: a0 * b2 - b0 * a2,
        linear: a0 * b3 + a1 * b2 - b0 * a3 - b1 * a2,
        quadratic: a1 * b3 - b1 * a3,
    };
    let Roots::Finite(candidates) = roots(polynomial)? else {
        return Err(FieldError::Arithmetic);
    };
    for parameter in candidates.into_iter().flatten() {
        check_cancel(cancel)?;
        let Some(u) = ExactCoordinate::parameter(parameter)? else {
            continue;
        };
        let mut point = None;
        for field in [a, b] {
            let [a, b, c, d] = coefficients(field);
            let denominator = Linear {
                constant: c,
                slope: d,
            };
            if denominator.sign(parameter) == 0 {
                continue;
            }
            if let Some(v) = ExactCoordinate::unit(
                parameter,
                Linear {
                    constant: -a,
                    slope: -b,
                },
                denominator,
            )? {
                point = Some([u, v]);
            }
            break;
        }
        let Some(point) = point else { continue };
        // At a common regular zero R'(u)=Fu*Hv-Fv*Hu. Both irreducible
        // curves are regular; a repeated eliminant root is an exact tangency.
        let tangent = parameter.sign_linear(
            i32::try_from(polynomial.linear).map_err(|_| FieldError::Arithmetic)?,
            i32::try_from(2 * polynomial.quadratic).map_err(|_| FieldError::Arithmetic)?,
        ) == 0;
        result.add_point(point, fields, tangent, cancel)?;
    }
    Ok(())
}
impl Bilinear {
    /// Complete zero-set intersections in the closed unit square, preserving
    /// shared components and singular joins instead of inventing isolated roots.
    /// This does not construct a two-field region arrangement or source mesh.
    pub fn intersections(
        self,
        other: Self,
        cancel: impl Fn() -> bool,
    ) -> Result<BilinearIntersections, FieldError> {
        check_cancel(&cancel)?;
        let fields = [self, other];
        let [left, right] = [components(self)?, components(other)?];
        let mut result = BilinearIntersections {
            points: [None; 4],
            count: 0,
            common: [None; 2],
            common_count: 0,
        };
        for a in left.into_iter().flatten() {
            for b in right.into_iter().flatten() {
                check_cancel(&cancel)?;
                match (a, b) {
                    (CommonZero::Whole, c) | (c, CommonZero::Whole) => result.add_common(c)?,
                    (
                        CommonZero::Axis {
                            axis: a,
                            position: x,
                        },
                        CommonZero::Axis {
                            axis: b,
                            position: y,
                        },
                    ) => {
                        if a == b {
                            if x == y {
                                result.add_common(CommonZero::Axis {
                                    axis: a,
                                    position: x,
                                })?;
                            }
                        } else {
                            let point = if a == 0 {
                                [rational_coordinate(x)?, rational_coordinate(y)?]
                            } else {
                                [rational_coordinate(y)?, rational_coordinate(x)?]
                            };
                            result.add_point(point, fields, false, &cancel)?;
                        }
                    }
                    (CommonZero::Axis { axis, position }, CommonZero::Curve(curve))
                    | (CommonZero::Curve(curve), CommonZero::Axis { axis, position }) => {
                        if let Some(point) = axis_curve(axis, position, curve)? {
                            result.add_point(point, fields, false, &cancel)?;
                        }
                    }
                    (CommonZero::Curve(a), CommonZero::Curve(b)) => {
                        curve_pair(a, b, &mut result, fields, &cancel)?
                    }
                }
            }
        }
        if left.contains(&Some(CommonZero::Whole)) || right.contains(&Some(CommonZero::Whole)) {
            let common = result.common;
            if let [
                Some(CommonZero::Axis {
                    axis: 0,
                    position: u,
                }),
                Some(CommonZero::Axis {
                    axis: 1,
                    position: v,
                }),
            ] = common
            {
                result.add_point(
                    [rational_coordinate(u)?, rational_coordinate(v)?],
                    fields,
                    false,
                    &cancel,
                )?;
            }
        }
        result.canonicalize(&cancel)?;
        check_cancel(&cancel)?;
        Ok(result)
    }
}

#[cfg(test)]
mod tests;
