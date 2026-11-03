// SPDX-License-Identifier: GPL-3.0-or-later
//! Resample one source image onto another image's pixel grid for local display
//! composition. Both must declare the same frame of reference and lie on
//! parallel planes within an explicit offset. That is a stated spatial
//! relationship, not registration: nothing aligns differently positioned data.
//! Values stay separate from the target's samples and are never combined.
use crate::study::Frame;

/// Composition is bounded like other display layers.
pub const MAX_PIXELS: u64 = 4 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FusionError {
    Color,
    Geometry,
    Reference,
    Orientation,
    Offset,
    Limit,
    Cancelled,
}
impl std::fmt::Display for FusionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Color => "Fusion supports scalar images only.",
            Self::Geometry => {
                "Fusion needs valid position, orientation and spacing for both images."
            }
            Self::Reference => "Fusion needs both images to declare the same frame of reference.",
            Self::Orientation => "Fusion needs parallel image planes.",
            Self::Offset => "The two image planes are farther apart than the allowed offset.",
            Self::Limit => "The fused picture exceeds the supported size.",
            Self::Cancelled => "Fusion update cancelled.",
        })
    }
}

/// Source values on the target grid; `valid` is 0 where no source sample exists.
#[derive(Clone, Debug, PartialEq)]
pub struct FusedPlane {
    pub width: u32,
    pub height: u32,
    pub values: Vec<f32>,
    pub valid: Vec<u8>,
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// `offset_mm` bounds the distance between the two parallel planes.
pub fn resample_onto(
    target: &Frame,
    source: &Frame,
    offset_mm: f64,
    cancelled: &impl Fn() -> bool,
) -> Result<FusedPlane, FusionError> {
    if target.rgba.is_some() || source.rgba.is_some() {
        return Err(FusionError::Color);
    }
    let count = u64::from(target.width) * u64::from(target.height);
    if count == 0 || count > MAX_PIXELS {
        return Err(FusionError::Limit);
    }
    let (Some(t), Some(s)) = (&target.geometry, &source.geometry) else {
        return Err(FusionError::Geometry);
    };
    if t.validate().is_err() || s.validate().is_err() || !offset_mm.is_finite() || offset_mm < 0. {
        return Err(FusionError::Geometry);
    }
    // NIfTI scanner, aligned and template spaces stay distinct as well.
    match (&t.frame_of_reference, &s.frame_of_reference) {
        (Some(a), Some(b)) if a == b && t.nifti_transform == s.nifti_transform => {}
        _ => return Err(FusionError::Reference),
    }
    let (tu, tv) = t.directions();
    let (su, sv) = s.directions();
    let normal = cross(su, sv);
    if dot(cross(tu, tv), normal).abs() < 1. - 1e-6 {
        return Err(FusionError::Orientation);
    }
    // Pixel (x, y) of the target maps affinely to source pixel coordinates.
    let to_source = |world: [f64; 3]| {
        let delta = [
            world[0] - s.position[0],
            world[1] - s.position[1],
            world[2] - s.position[2],
        ];
        (
            [dot(delta, su) / s.spacing[1], dot(delta, sv) / s.spacing[0]],
            dot(delta, normal),
        )
    };
    let world = |pixel| t.pixel_to_world(pixel).map_err(|_| FusionError::Geometry);
    let (origin, distance) = to_source(world([0., 0.])?);
    if distance.abs() > offset_mm {
        return Err(FusionError::Offset);
    }
    let (x_step, _) = to_source(world([1., 0.])?);
    let (y_step, _) = to_source(world([0., 1.])?);
    let dx = [x_step[0] - origin[0], x_step[1] - origin[1]];
    let dy = [y_step[0] - origin[0], y_step[1] - origin[1]];
    let (width, height) = (source.width as usize, source.height as usize);
    let sample = |x: usize, y: usize| {
        let index = y * width + x;
        source
            .has_sample(index)
            .then(|| source.pixels.get(index).copied())
            .flatten()
            .filter(|value| value.is_finite())
    };
    let mut values = Vec::new();
    let mut valid = Vec::new();
    values
        .try_reserve_exact(count as usize)
        .map_err(|_| FusionError::Limit)?;
    valid
        .try_reserve_exact(count as usize)
        .map_err(|_| FusionError::Limit)?;
    for row in 0..target.height {
        if cancelled() {
            return Err(FusionError::Cancelled);
        }
        for column in 0..target.width {
            let (x, y) = (f64::from(column), f64::from(row));
            let p = [
                origin[0] + dx[0] * x + dy[0] * y,
                origin[1] + dx[1] * x + dy[1] * y,
            ];
            let value = bilinear(p, width, height, sample);
            valid.push(u8::from(value.is_some()));
            values.push(value.unwrap_or(0.));
        }
    }
    Ok(FusedPlane {
        width: target.width,
        height: target.height,
        values,
        valid,
    })
}

/// Pixel-centre bilinear sampling; any missing neighbour leaves it missing.
fn bilinear(
    p: [f64; 2],
    width: usize,
    height: usize,
    sample: impl Fn(usize, usize) -> Option<f32>,
) -> Option<f32> {
    const EDGE: f64 = 1e-6;
    let axis = |v: f64, size: usize| -> Option<(usize, f64)> {
        let last = (size - 1) as f64;
        if !(-EDGE..=last + EDGE).contains(&v) {
            return None;
        }
        let v = v.clamp(0., last);
        let base = (v.floor() as usize).min(size.saturating_sub(2));
        Some((base, v - base as f64))
    };
    let (x0, fx) = axis(p[0], width)?;
    let (y0, fy) = axis(p[1], height)?;
    let x1 = (x0 + 1).min(width - 1);
    let y1 = (y0 + 1).min(height - 1);
    let mut total = 0.;
    for (x, y, weight) in [
        (x0, y0, (1. - fx) * (1. - fy)),
        (x1, y0, fx * (1. - fy)),
        (x0, y1, (1. - fx) * fy),
        (x1, y1, fx * fy),
    ] {
        if weight > 0. {
            total += weight * f64::from(sample(x, y)?);
        }
    }
    let total = total as f32;
    total.is_finite().then_some(total)
}
