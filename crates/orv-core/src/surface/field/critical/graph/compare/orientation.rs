// SPDX-License-Identifier: GPL-3.0-or-later
//! Exact planar turns, including six unrelated quadratic coordinate fields.
use super::{integer::FixedInteger, *};
use crate::{resources::MemoryBudget, surface::SurfaceError};
use std::sync::Arc;

#[derive(Clone, Copy)]
struct Form {
    p: i128,
    q: i128,
    r: i128,
    field: Option<usize>,
}
fn checked(cancel: &impl Fn() -> bool) -> Result<(), SurfaceError> {
    if cancel() {
        Err(SurfaceError::Cancelled)
    } else {
        Ok(())
    }
}
fn arithmetic(_: FieldError) -> SurfaceError {
    SurfaceError::Geometry
}

pub(crate) fn orientation(
    points: [[ExactCoordinate; 2]; 3],
    budget: &Arc<MemoryBudget>,
    cancel: &impl Fn() -> bool,
) -> Result<i8, SurfaceError> {
    checked(cancel)?;
    let mut fields = [0u64; 6];
    let mut count = 0;
    let mut forms = [Form {
        p: 0,
        q: 0,
        r: 1,
        field: None,
    }; 6];
    for (output, coordinate) in forms.iter_mut().zip(points.into_iter().flatten()) {
        checked(cancel)?;
        let rational = coordinate.rational_value();
        let d = coordinate.radicand(rational.is_some());
        let (p, q, r) = coordinate.rationalized(rational, d).map_err(arithmetic)?;
        let field = if d == 0 {
            None
        } else {
            let index = fields[..count]
                .iter()
                .position(|v| *v == d)
                .unwrap_or(count);
            if index == count {
                fields[count] = d;
                count += 1;
            }
            Some(index)
        };
        *output = Form { p, q, r, field };
    }
    // Rationalized magnitudes are <2^89, and sqrt(d)<2^22. Clearing the six
    // positive denominators gives weighted coefficients <2^(541+22*k).
    // Eliminating one radical squares and sums at most2^k products: the bound
    // becomes2*b+k+2. These widths cover all k<=6, including dependent fields.
    match count {
        0 => {
            let mut coefficient = [FixedInteger::<16>::new(0)];
            coefficients(&forms, &fields, &mut coefficient, cancel)?;
            Ok(coefficient[0].sign())
        }
        1 => evaluate::<32>(&forms, &fields[..count], budget, cancel),
        2 => evaluate::<64>(&forms, &fields[..count], budget, cancel),
        3 => evaluate::<128>(&forms, &fields[..count], budget, cancel),
        4 => evaluate::<256>(&forms, &fields[..count], budget, cancel),
        5 => evaluate::<512>(&forms, &fields[..count], budget, cancel),
        _ => evaluate::<1024>(&forms, &fields[..count], budget, cancel),
    }
}
fn coefficients<const N: usize>(
    forms: &[Form; 6],
    fields: &[u64],
    output: &mut [FixedInteger<N>],
    cancel: &impl Fn() -> bool,
) -> Result<(), SurfaceError> {
    // det([x,y,1]) with all six positive coordinate denominators cleared.
    for (left, right, sign) in [
        (0, 3, 1),
        (2, 5, 1),
        (4, 1, 1),
        (1, 2, -1),
        (3, 4, -1),
        (5, 0, -1),
    ] {
        for choice in 0..4 {
            checked(cancel)?;
            let mut value = FixedInteger::<N>::new(sign);
            let mut mask = 0usize;
            for (i, form) in forms.iter().enumerate() {
                let radical = (i == left && choice & 1 != 0) || (i == right && choice & 2 != 0);
                let factor = if i == left || i == right {
                    if radical { form.q } else { form.p }
                } else {
                    form.r
                };
                value = value
                    .product(FixedInteger::new(factor))
                    .map_err(arithmetic)?;
                if radical {
                    let Some(field) = form.field else { continue };
                    let bit = 1usize << field;
                    if mask & bit != 0 {
                        value = value.times(fields[field]).map_err(arithmetic)?;
                    }
                    mask ^= bit;
                }
            }
            output[mask] = output[mask].sum(value).map_err(arithmetic)?;
        }
    }
    Ok(())
}
fn evaluate<const N: usize>(
    forms: &[Form; 6],
    fields: &[u64],
    budget: &Arc<MemoryBudget>,
    cancel: &impl Fn() -> bool,
) -> Result<i8, SurfaceError> {
    let count = 1usize << fields.len();
    let length = 2 * count - 1;
    let bytes = length * std::mem::size_of::<FixedInteger<N>>();
    let _reservation = budget.reserve(bytes as u64).ok_or(SurfaceError::Memory)?;
    let mut values = Vec::new();
    values
        .try_reserve_exact(length)
        .map_err(|_| SurfaceError::Memory)?;
    if values.capacity() != length {
        return Err(SurfaceError::Memory);
    }
    values.resize(length, FixedInteger::<N>::new(0));
    let (input, scratch) = values.split_at_mut(count);
    coefficients(forms, fields, input, cancel)?;
    sign(input, fields, scratch, cancel)
}
fn sign<const N: usize>(
    input: &[FixedInteger<N>],
    fields: &[u64],
    scratch: &mut [FixedInteger<N>],
    cancel: &impl Fn() -> bool,
) -> Result<i8, SurfaceError> {
    checked(cancel)?;
    if fields.is_empty() {
        return Ok(input[0].sign());
    }
    let half = input.len() / 2;
    let (a, b) = input.split_at(half);
    let (field, lower_fields) = fields.split_last().ok_or(SurfaceError::Geometry)?;
    let x = sign(a, lower_fields, scratch, cancel)?;
    let y = sign(b, lower_fields, scratch, cancel)?;
    if x == 0 {
        return Ok(y);
    }
    if y == 0 || x == y {
        return Ok(x);
    }
    let (norm, tail) = scratch.split_at_mut(half);
    norm.fill(FixedInteger::new(0));
    // Only opposite signs permit magnitude squaring. The new expression is
    // A²-d*B² in one fewer radical, evaluated in the same real embedding.
    for (terms, subtract) in [(a, false), (b, true)] {
        for (i, left) in terms.iter().enumerate() {
            if left.sign() == 0 {
                continue;
            }
            for (j, right) in terms.iter().enumerate() {
                checked(cancel)?;
                if right.sign() == 0 {
                    continue;
                }
                let mut value = left.product(*right).map_err(arithmetic)?;
                for (k, d) in lower_fields.iter().enumerate() {
                    if i & j & (1 << k) != 0 {
                        value = value.times(*d).map_err(arithmetic)?;
                    }
                }
                if subtract {
                    value = value.times(*field).map_err(arithmetic)?.negative();
                }
                norm[i ^ j] = norm[i ^ j].sum(value).map_err(arithmetic)?;
            }
        }
    }
    Ok(match sign(norm, lower_fields, tail, cancel)? {
        1 => x,
        -1 => y,
        _ => 0,
    })
}

#[cfg(test)]
mod tests;
