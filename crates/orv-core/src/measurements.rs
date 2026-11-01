// SPDX-License-Identifier: GPL-3.0-or-later
//! Exploratory measurements only. Coordinates and spacing are physical, never screen pixels.
use crate::{study::Frame, vector::*};
use serde::{Deserialize, Serialize};
mod color;
mod reduction;
mod shape;
pub use color::{
    ChannelStats, ColorRegionStats, ColorRoiDomain, ColorRoiStats, color_roi_shape_stats,
    color_roi_shape_stats_cancellable, color_roi_stats, color_roi_stats_cancellable,
};
pub use shape::{MAX_POLYGON_VERTICES, RoiShape};
#[cfg(test)]
mod shape_tests;

pub fn distance_mm(a: V3, b: V3) -> Result<f64, &'static str> {
    if !finite(a) || !finite(b) {
        return Err("Measurement coordinates must be finite");
    }
    let result = norm(sub(a, b));
    if !result.is_finite() {
        return Err("Measurement is outside the supported range");
    }
    Ok(result)
}
pub fn angle_degrees(a: V3, vertex: V3, b: V3) -> Result<f64, &'static str> {
    if !finite(a) || !finite(vertex) || !finite(b) {
        return Err("Measurement coordinates must be finite");
    }
    let (u, v) = (sub(a, vertex), sub(b, vertex));
    let scale = norm(u) * norm(v);
    if !scale.is_finite() || scale < 1e-12 {
        return Err("An angle needs two nonzero arms");
    }
    Ok((dot(u, v) / scale).clamp(-1.0, 1.0).acos().to_degrees())
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoiStats {
    pub count: usize,
    pub min: f64,
    pub max: f64,
    pub mean: f64,
    /// Population standard deviation, in modality units.
    pub standard_deviation: f64,
    /// Sum of areas of included finite pixels. None if spacing is not known/valid.
    pub area_mm2: Option<f64>,
}
/// Inclusive rectangle of pixel centres; ignores declared padding and resampled no-data.
pub fn roi_stats(frame: &Frame, corners: [[f64; 2]; 2]) -> Result<RoiStats, &'static str> {
    roi_stats_cancellable(frame, corners, || false)
}
pub fn roi_stats_cancellable(
    frame: &Frame,
    corners: [[f64; 2]; 2],
    cancelled: impl Fn() -> bool,
) -> Result<RoiStats, &'static str> {
    roi_shape_stats_cancellable(frame, RoiShape::Rectangle(corners), cancelled)
        .map(|stats| stats.samples)
}

#[derive(Debug, Clone, PartialEq)]
pub struct RegionStats {
    pub samples: RoiStats,
    /// Continuous region area, independent of sampled pixel count and missing data.
    pub geometric_area_mm2: Option<f64>,
}
pub fn roi_shape_stats(frame: &Frame, shape: RoiShape<'_>) -> Result<RegionStats, &'static str> {
    roi_shape_stats_cancellable(frame, shape, || false)
}
/// Worker-only scalar reduction over included pixel centres, without allocating a mask.
pub fn roi_shape_stats_cancellable(
    frame: &Frame,
    region: RoiShape<'_>,
    cancelled: impl Fn() -> bool,
) -> Result<RegionStats, &'static str> {
    if cancelled() {
        return Err("Cancelled");
    }
    if frame.slab.is_some() {
        return Err("Slab projections are not physical measurement planes");
    }
    if frame.rgba.is_some() {
        return Err("ROI statistics need scalar images; color channels are not modality values");
    }
    let count = (frame.width as usize)
        .checked_mul(frame.height as usize)
        .filter(|n| *n > 0)
        .ok_or("ROI coordinates or image are invalid")?;
    if frame.pixels.len() != count
        || frame
            .validity
            .as_ref()
            .is_some_and(|mask| mask.len() != count)
    {
        return Err("ROI coordinates or image are invalid");
    }
    let region = region.prepare([frame.width, frame.height], &cancelled)?;
    let mut moments = reduction::Moments::default();
    region.visit(frame.width, &cancelled, |index| {
        if frame.has_sample(index) {
            moments.add(f64::from(frame.pixels[index]));
        }
        Ok(())
    })?;
    if moments.count == 0 {
        return Err("The ROI contains no scalar samples");
    }
    Ok(RegionStats {
        samples: RoiStats {
            count: moments.count,
            min: moments.min,
            max: moments.max,
            mean: moments.mean,
            standard_deviation: moments.standard_deviation(),
            area_mm2: shape::area_mm2(frame, moments.count as f64),
        },
        geometric_area_mm2: shape::area_mm2(frame, region.area_pixels),
    })
}
