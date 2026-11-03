// SPDX-License-Identifier: GPL-3.0-or-later
//! Prototype thin curved reformat of a scalar volume along a manual polyline.
//!
//! Convention: column `i` lies at arc length `i * step_mm` along the polyline
//! from its first point. Row `j` is offset by `(j - (rows - 1) / 2) * step_mm`
//! along the reference direction projected perpendicular to the local tangent.
//! The result has no planar affine, so physical measurements stay unavailable;
//! every pixel's source position is exact through `source_position`.
use crate::{study::Frame, volume::Volume};
use sha2::{Digest, Sha256};
use std::sync::Arc;

pub const MAX_POINTS: usize = 256;
pub const MAX_SIDE: u32 = 2048;
pub const MAX_SAMPLES: u64 = 4 * 1024 * 1024;
/// Tangents within this angle of the reference direction are refused.
const MIN_ANGLE_DEGREES: f64 = 5.;

type V3 = [f64; 3];
fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn mul(a: V3, s: f64) -> V3 {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn norm(a: V3) -> f64 {
    dot(a, a).sqrt()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CurvedError {
    Points,
    Reference,
    Alignment,
    Limit,
    Color,
    Cancelled,
}
impl std::fmt::Display for CurvedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Points => "A curve needs 2–256 finite points with nonzero total length.",
            Self::Reference => "The reference direction must be a finite nonzero vector.",
            Self::Alignment => {
                "The curve runs too close to the reference direction to define rows."
            }
            Self::Limit => "The curved image exceeds the supported size.",
            Self::Color => "Curved reformats support scalar volumes only.",
            Self::Cancelled => "Curved reformat cancelled.",
        })
    }
}

/// A validated, bounded curve specification.
#[derive(Clone, Debug, PartialEq)]
pub struct Curve {
    points: Vec<V3>,
    /// Cumulative arc length at each point.
    lengths: Vec<f64>,
    reference: V3,
    step_mm: f64,
    rows: u32,
}
impl Curve {
    /// `reference` sets the row direction; `half_width_mm` bounds the rows on
    /// each side of the curve; `step_mm` is the spacing along and across it.
    pub fn new(
        points: &[V3],
        reference: V3,
        half_width_mm: f64,
        step_mm: f64,
    ) -> Result<Self, CurvedError> {
        if points.len() < 2 || points.len() > MAX_POINTS {
            return Err(CurvedError::Points);
        }
        if points.iter().flatten().any(|v| !v.is_finite()) {
            return Err(CurvedError::Points);
        }
        // Repeated points have no tangent; drop them rather than guess one.
        let mut kept: Vec<V3> = Vec::with_capacity(points.len());
        for point in points {
            if kept
                .last()
                .is_none_or(|last| norm(sub(*point, *last)) > 1e-9)
            {
                kept.push(*point);
            }
        }
        if kept.len() < 2 {
            return Err(CurvedError::Points);
        }
        let length = norm(reference);
        if !length.is_finite() || length <= 0. {
            return Err(CurvedError::Reference);
        }
        let reference = mul(reference, 1. / length);
        if !step_mm.is_finite() || step_mm <= 0. || !half_width_mm.is_finite() || half_width_mm < 0.
        {
            return Err(CurvedError::Limit);
        }
        let mut lengths = vec![0.];
        for pair in kept.windows(2) {
            let segment = sub(pair[1], pair[0]);
            let tangent = mul(segment, 1. / norm(segment));
            if dot(tangent, reference).abs() > MIN_ANGLE_DEGREES.to_radians().cos() {
                return Err(CurvedError::Alignment);
            }
            lengths.push(lengths.last().copied().unwrap_or(0.) + norm(segment));
        }
        let rows = (2. * half_width_mm / step_mm).floor() + 1.;
        let columns = (lengths.last().copied().unwrap_or(0.) / step_mm).floor() + 1.;
        if rows > f64::from(MAX_SIDE)
            || columns > f64::from(MAX_SIDE)
            || rows * columns > MAX_SAMPLES as f64
        {
            return Err(CurvedError::Limit);
        }
        Ok(Self {
            points: kept,
            lengths,
            reference,
            step_mm,
            rows: rows as u32,
        })
    }
    pub fn length_mm(&self) -> f64 {
        self.lengths.last().copied().unwrap_or(0.)
    }
    pub fn size(&self) -> [u32; 2] {
        [
            (self.length_mm() / self.step_mm).floor() as u32 + 1,
            self.rows,
        ]
    }
    /// Exact source-space position of a curved-image pixel centre.
    pub fn source_position(&self, column: u32, row: u32) -> Option<V3> {
        let [columns, rows] = self.size();
        if column >= columns || row >= rows {
            return None;
        }
        let s = f64::from(column) * self.step_mm;
        // The last segment containing s; vertices belong to the later segment.
        let segment = self
            .lengths
            .windows(2)
            .rposition(|pair| pair[0] <= s)?
            .min(self.points.len() - 2);
        let (a, b) = (self.points[segment], self.points[segment + 1]);
        let direction = sub(b, a);
        let length = norm(direction);
        let tangent = mul(direction, 1. / length);
        let centre = add(a, mul(tangent, s - self.lengths[segment]));
        let across = sub(self.reference, mul(tangent, dot(self.reference, tangent)));
        let across = mul(across, 1. / norm(across));
        let offset = (f64::from(row) - f64::from(rows - 1) / 2.) * self.step_mm;
        Some(add(centre, mul(across, offset)))
    }
    /// Identity of the curve and sampling, independent of the source volume.
    pub fn digest(&self) -> [u8; 32] {
        let mut hash = Sha256::new();
        hash.update(b"OpenRadView curved reformat v1\0");
        hash.update((self.points.len() as u32).to_le_bytes());
        for value in self.points.iter().flatten().chain(&self.reference) {
            hash.update(value.to_le_bytes());
        }
        hash.update(self.step_mm.to_le_bytes());
        hash.update(self.rows.to_le_bytes());
        hash.finalize().into()
    }
}

/// A curved image: values on the curve grid, missing outside valid source data.
#[derive(Clone, Debug)]
pub struct CurvedImage {
    pub curve: Curve,
    /// Scalar frame without geometry: it has no single planar affine.
    pub frame: Arc<Frame>,
}

pub fn reformat(
    volume: &Volume,
    curve: &Curve,
    cancelled: &impl Fn() -> bool,
) -> Result<CurvedImage, CurvedError> {
    if volume.is_color() {
        return Err(CurvedError::Color);
    }
    let [columns, rows] = curve.size();
    let mut pixels = Vec::new();
    pixels
        .try_reserve_exact(columns as usize * rows as usize)
        .map_err(|_| CurvedError::Limit)?;
    for row in 0..rows {
        if cancelled() {
            return Err(CurvedError::Cancelled);
        }
        for column in 0..columns {
            let value = curve
                .source_position(column, row)
                .and_then(|world| volume.sample(world));
            pixels.push(value.unwrap_or(f32::NAN));
        }
    }
    let source = &volume.frames()[0];
    Ok(CurvedImage {
        curve: curve.clone(),
        frame: Arc::new(Frame {
            presentation: None,
            calibration: None,
            slab: None,
            scalar: source.scalar.as_ref().map(|data| data.resampled()),
            validity: None,
            color: None,
            nifti: source.nifti.clone(),
            rgba: None,
            width: columns,
            height: rows,
            pixels: pixels.into(),
            geometry: None,
            instance: 0,
            frame_index: 0,
            monochrome1: source.monochrome1,
            default_window: source.default_window,
        }),
    })
}
