// SPDX-License-Identifier: GPL-3.0-or-later
//! Exploratory measurements only. Coordinates and spacing are physical, never screen pixels.
use crate::{study::Frame, vector::*};
use serde::{Deserialize, Serialize};
mod color;
pub use color::{
    ChannelStats, ColorRoiDomain, ColorRoiStats, color_roi_stats, color_roi_stats_cancellable,
};

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
    if cancelled() {
        return Err("Cancelled");
    }
    if frame.rgba.is_some() {
        return Err("ROI statistics need scalar images; color channels are not modality values");
    }
    if frame.width == 0
        || frame.height == 0
        || frame.width as usize * frame.height as usize != frame.pixels.len()
        || corners.iter().flatten().any(|v| !v.is_finite())
    {
        return Err("ROI coordinates or image are invalid");
    }
    let min: [f64; 2] = std::array::from_fn(|i| corners[0][i].min(corners[1][i]));
    let max: [f64; 2] = std::array::from_fn(|i| corners[0][i].max(corners[1][i]));
    if min[0] < 0.0
        || min[1] < 0.0
        || max[0] > (frame.width - 1) as f64
        || max[1] > (frame.height - 1) as f64
    {
        return Err("ROI must lie inside the image");
    }
    let mut stats = RoiStats {
        count: 0,
        min: f64::INFINITY,
        max: f64::NEG_INFINITY,
        mean: 0.0,
        standard_deviation: 0.0,
        area_mm2: None,
    };
    let mut m2 = 0.0;
    for y in min[1].ceil() as u32..=max[1].floor() as u32 {
        if cancelled() {
            return Err("Cancelled");
        }
        for x in min[0].ceil() as u32..=max[0].floor() as u32 {
            let value = frame.pixels[(y * frame.width + x) as usize] as f64;
            if !frame.has_sample((y * frame.width + x) as usize) {
                continue;
            }
            stats.count += 1;
            let delta = value - stats.mean;
            stats.mean += delta / stats.count as f64;
            m2 += delta * (value - stats.mean);
            stats.min = stats.min.min(value);
            stats.max = stats.max.max(value);
        }
    }
    if stats.count == 0 {
        return Err("The ROI contains no scalar samples");
    }
    stats.standard_deviation = (m2 / stats.count as f64).max(0.0).sqrt();
    stats.area_mm2 = frame
        .geometry
        .as_ref()
        .filter(|g| g.validate().is_ok())
        .map(|g| stats.count as f64 * g.spacing[0] * g.spacing[1]);
    Ok(stats)
}
