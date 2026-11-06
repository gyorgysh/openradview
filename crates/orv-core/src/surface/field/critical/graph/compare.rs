// SPDX-License-Identifier: GPL-3.0-or-later
//! Exact ordering across incident cells, whose quadratic fields need not agree.
use super::*;
mod integer;
mod orientation;
use integer::Integer;
pub(super) use orientation::orientation;

impl ExactCoordinate {
    /// Exact numerical ordering, including unrelated quadratic fields from
    /// neighboring cells. No approximate coordinate participates in the result.
    pub fn compare(self, other: Self) -> Result<Ordering, FieldError> {
        let left = self.rational_value();
        let right = other.rational_value();
        let d = self.radicand(left.is_some());
        let e = other.radicand(right.is_some());
        let common = common_radicand(d, e);
        let compatible = |v: u64| v == 0 || (v / common).isqrt().pow(2) == v / common;
        if compatible(d) && compatible(e) {
            return self.compare_same_field(other);
        }
        let (p, q, r) = self.rationalized(left, d)?;
        let (s, t, u) = other.rationalized(right, e)?;
        let a = Integer::new(p)
            .product(Integer::new(u))?
            .difference(Integer::new(s).product(Integer::new(r))?)?;
        let b = Integer::new(q).product(Integer::new(u))?;
        let c = Integer::new(t).product(Integer::new(r))?.negative();
        Ok(two_radical_sign(a, b, d, c, e)?.cmp(&0))
    }

    // (P + Q sqrt(d))/R, with R positive. Irreducibility makes a nonzero
    // denominator's conjugate nonzero too, so rationalization cannot lose it.
    fn rationalized(
        self,
        rational: Option<Rational>,
        d: u64,
    ) -> Result<(i128, i128, i128), FieldError> {
        if let Some(value) = rational {
            return Ok((
                i128::from(value.numerator),
                0,
                i128::from(value.denominator),
            ));
        }
        let (n, denominator) = self.forms(rational, d)?;
        let subtract = |a: i128, b: i128| a.checked_sub(b).ok_or(FieldError::Arithmetic);
        let times_d = |a: i128| product(a, i128::from(d));
        let p = subtract(
            product(n.0, denominator.0)?,
            times_d(product(n.1, denominator.1)?)?,
        )?;
        let q = subtract(product(n.1, denominator.0)?, product(n.0, denominator.1)?)?;
        let r = subtract(
            product(denominator.0, denominator.0)?,
            times_d(product(denominator.1, denominator.1)?)?,
        )?;
        if r == 0 {
            return Err(FieldError::Arithmetic);
        }
        if r < 0 {
            Ok((
                p.checked_neg().ok_or(FieldError::Arithmetic)?,
                q.checked_neg().ok_or(FieldError::Arithmetic)?,
                r.checked_neg().ok_or(FieldError::Arithmetic)?,
            ))
        } else {
            Ok((p, q, r))
        }
    }
}

fn radical_sign(p: Integer, q: Integer, d: u64) -> Result<i8, FieldError> {
    if q.sign() == 0 || d == 0 {
        return Ok(p.sign());
    }
    if p.sign() == 0 || p.sign() == q.sign() {
        return Ok(q.sign());
    }
    Ok(
        match p.product(p)?.compare_magnitude(q.product(q)?.times(d)?) {
            Ordering::Greater => p.sign(),
            Ordering::Less => q.sign(),
            Ordering::Equal => 0,
        },
    )
}

// X=A+B sqrt(d). Only when X and C sqrt(e) have opposite signs may their
// magnitudes be squared without changing the comparison. The squared difference
// is (A²+B²d-C²e) + 2AB sqrt(d), reducing it to one radical.
fn two_radical_sign(a: Integer, b: Integer, d: u64, c: Integer, e: u64) -> Result<i8, FieldError> {
    let x = radical_sign(a, b, d)?;
    let y = if e == 0 { 0 } else { c.sign() };
    if x == 0 {
        return Ok(y);
    }
    if y == 0 || x == y {
        return Ok(x);
    }
    let p = a
        .product(a)?
        .sum(b.product(b)?.times(d)?)?
        .difference(c.product(c)?.times(e)?)?;
    let q = a.product(b)?.times(2)?;
    Ok(match radical_sign(p, q, d)? {
        1 => x,
        -1 => y,
        _ => 0,
    })
}

// Exact sign of A+B*u+C*v+D*u*v, even when the two coordinates belong to
// unrelated fields. With positive denominators its numerator is
// (L+M*sqrt(d))+(N+O*sqrt(d))*sqrt(e). Squaring opposing terms leaves one radical.
pub(super) fn bilinear_sign(
    coefficients: [i32; 4],
    point: [ExactCoordinate; 2],
) -> Result<i8, FieldError> {
    if coefficients
        .into_iter()
        .any(|v| v.unsigned_abs() > SOURCE_FORM)
    {
        return Err(FieldError::Arithmetic);
    }
    bounded_bilinear_sign(coefficients, point)
}

/// Derivative cross products need coefficients up to1040400, independently of
/// coordinate forms. The wider polynomial fits the1024-bit predicate.
pub(super) fn bounded_bilinear_sign(
    coefficients: [i32; 4],
    point: [ExactCoordinate; 2],
) -> Result<i8, FieldError> {
    if coefficients
        .into_iter()
        .any(|v| u64::from(v.unsigned_abs()) > MAX_COEFFICIENT)
    {
        return Err(FieldError::Arithmetic);
    }
    let [left, right] = point;
    let lr = left.rational_value();
    let rr = right.rational_value();
    let d = left.radicand(lr.is_some());
    let e = right.radicand(rr.is_some());
    let (p, q, r) = left.rationalized(lr, d)?;
    let (s, t, u) = right.rationalized(rr, e)?;
    let [a, b, c, f] = coefficients.map(|v| Integer::new(i128::from(v)));
    let [p, q, r, s, t, u] = [p, q, r, s, t, u].map(Integer::new);
    let l = a
        .product(r)?
        .product(u)?
        .sum(b.product(p)?.product(u)?)?
        .sum(c.product(s)?.product(r)?)?
        .sum(f.product(p)?.product(s)?)?;
    let m = b.product(q)?.product(u)?.sum(f.product(q)?.product(s)?)?;
    let n = c.product(t)?.product(r)?.sum(f.product(p)?.product(t)?)?;
    let o = f.product(q)?.product(t)?;
    let x = radical_sign(l, m, d)?;
    let y = if e == 0 { 0 } else { radical_sign(n, o, d)? };
    if x == 0 {
        return Ok(y);
    }
    if y == 0 || x == y {
        return Ok(x);
    }
    let p = l
        .product(l)?
        .sum(m.product(m)?.times(d)?)?
        .difference(n.product(n)?.sum(o.product(o)?.times(d)?)?.times(e)?)?;
    let q = l
        .product(m)?
        .difference(n.product(o)?.times(e)?)?
        .times(2)?;
    Ok(match radical_sign(p, q, d)? {
        1 => x,
        -1 => y,
        _ => 0,
    })
}

#[cfg(test)]
mod tests;
