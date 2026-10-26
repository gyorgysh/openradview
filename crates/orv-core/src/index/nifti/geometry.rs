// SPDX-License-Identifier: GPL-3.0-or-later
use super::fields::Fields;
use crate::study::Geometry;
pub(super) fn geometry(f: &Fields<'_>, id: u64) -> Option<(Option<Geometry>, [f64; 3])> {
    if f.u16(254) > 5 || f.u16(252) > 5 {
        return None;
    }
    let unit = match f.units() & 7 {
        1 => 1000.,
        2 => 1.,
        3 => 0.001,
        _ => return None,
    };
    let mut matrix = [[0.; 4]; 3];
    if (1..=5).contains(&f.u16(254)) {
        for (row, values) in matrix.iter_mut().enumerate() {
            for (col, value) in values.iter_mut().enumerate() {
                *value = f.f32(280 + row * 16 + col * 4);
            }
        }
    } else if (1..=5).contains(&f.u16(252)) {
        let (mut b, mut c, mut d) = (f.f32(256), f.f32(260), f.f32(264));
        let sum = b * b + c * c + d * d;
        if !sum.is_finite() {
            return None;
        }
        let a = if 1. - sum < 1e-7 {
            let n = sum.sqrt();
            b /= n;
            c /= n;
            d /= n;
            0.
        } else {
            (1. - sum).sqrt()
        };
        let steps = [f.f32(80), f.f32(84), f.f32(88)];
        if steps.iter().any(|v| !v.is_finite() || *v <= 0.) {
            return None;
        }
        if !f.f32(76).is_finite() {
            return None;
        }
        let qfac = if f.f32(76) < 0. { -1. } else { 1. };
        let rotation = [
            [
                a * a + b * b - c * c - d * d,
                2. * b * c - 2. * a * d,
                2. * b * d + 2. * a * c,
            ],
            [
                2. * b * c + 2. * a * d,
                a * a + c * c - b * b - d * d,
                2. * c * d - 2. * a * b,
            ],
            [
                2. * b * d - 2. * a * c,
                2. * c * d + 2. * a * b,
                a * a + d * d - c * c - b * b,
            ],
        ];
        for row in 0..3 {
            for (col, step) in steps.iter().enumerate() {
                matrix[row][col] = rotation[row][col] * step * if col == 2 { qfac } else { 1. };
            }
            matrix[row][3] = f.f32(268 + row * 4);
        }
    } else {
        return None;
    }
    if matrix.iter().flatten().any(|v| !v.is_finite()) {
        return None;
    }
    // NIfTI RAS -> the engine's LPS millimetres; do not infer orientation from pixdim alone.
    for (row, values) in matrix.iter_mut().enumerate() {
        for value in values {
            *value *= unit * if row < 2 { -1. } else { 1. };
        }
    }
    let steps: [[f64; 3]; 3] =
        std::array::from_fn(|col| std::array::from_fn(|row| matrix[row][col]));
    let norm = |v: [f64; 3]| v.iter().map(|x| x * x).sum::<f64>().sqrt();
    let (dx, dy) = (norm(steps[0]), norm(steps[1]));
    if dx <= 0. || dy <= 0. || norm(steps[2]) <= 0. {
        return None;
    }
    let geometry = Geometry {
        nifti_transform: Some(crate::study::NiftiTransform {
            method: if f.u16(254) > 0 {
                crate::study::NiftiMethod::Sform
            } else {
                crate::study::NiftiMethod::Qform
            },
            code: if f.u16(254) > 0 {
                f.u16(254)
            } else {
                f.u16(252)
            },
        }),
        frame_of_reference: Some(format!("nifti-local-{id}")),
        position: std::array::from_fn(|row| matrix[row][3]),
        orientation: std::array::from_fn(|i| {
            if i < 3 {
                steps[0][i] / dx
            } else {
                steps[1][i - 3] / dy
            }
        }),
        spacing: [dy, dx],
    };
    geometry.validate().ok()?;
    let normal = geometry.normal();
    if normal
        .iter()
        .zip(steps[2])
        .map(|(a, b)| a * b)
        .sum::<f64>()
        .abs()
        < 1e-5
    {
        return None;
    }
    Some((Some(geometry), steps[2]))
}
