// SPDX-License-Identifier: GPL-3.0-or-later
//! Local image descriptions. Values and declared geometry do not establish medical meaning.
mod histogram;
use crate::study::{Frame, Geometry};
pub use histogram::{ChannelHistogram, Histogram, histogram};

/// LPS directions outside the left, right, top and bottom image edges.
/// Small (<0.0001) direction components are suppressed as rounding noise.
pub fn edge_directions(geometry: &Geometry) -> Option<[String; 4]> {
    geometry.validate().ok()?;
    if geometry
        .nifti_transform
        .as_ref()
        .is_some_and(|n| n.code == 0)
    {
        return None;
    }
    let (column, row) = geometry.directions();
    Some([
        direction(column.map(|v| -v)),
        direction(column),
        direction(row.map(|v| -v)),
        direction(row),
    ])
}
fn direction(vector: [f64; 3]) -> String {
    let mut axes = [0, 1, 2];
    axes.sort_by(|&a, &b| vector[b].abs().total_cmp(&vector[a].abs()));
    axes.into_iter()
        .filter(|&axis| vector[axis].abs() > 1e-4)
        .map(|axis| match (axis, vector[axis].is_sign_positive()) {
            (0, true) => 'L',
            (0, false) => 'R',
            (1, true) => 'P',
            (1, false) => 'A',
            (2, true) => 'S',
            _ => 'I',
        })
        .collect()
}

pub fn coordinate_reference(geometry: &Geometry) -> &'static str {
    match geometry.nifti_transform.as_ref().map(|n| n.code) {
        None | Some(1) => "Scanner LPS · mm",
        Some(2) => "Aligned LPS · mm",
        Some(3) => "Talairach LPS · mm",
        Some(4) => "MNI152 LPS · mm",
        Some(5) => "Template LPS · mm",
        Some(0) => "Unknown NIfTI reference",
        Some(_) => "Other NIfTI reference · LPS mm",
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Sample {
    Scalar(f32),
    RgbCodes { values: [u16; 3], bits: u16 },
    SrgbDisplay([u8; 3]),
}
/// Nearest source pixel centre, without interpolation or display window/inversion.
pub fn sample(frame: &Frame, pixel: [f64; 2]) -> Option<Sample> {
    if pixel.iter().any(|v| !v.is_finite()) || frame.width == 0 || frame.height == 0 {
        return None;
    }
    let [x, y] = pixel.map(f64::round);
    if x < 0. || y < 0. || x >= f64::from(frame.width) || y >= f64::from(frame.height) {
        return None;
    }
    let index = (y as usize)
        .checked_mul(frame.width as usize)?
        .checked_add(x as usize)?;
    if !frame.has_sample(index) {
        return None;
    }
    if frame.color.is_some() && frame.rgba.is_none() {
        return None;
    }
    if let Some(color) = &frame.color {
        let channels = color
            .rgb
            .get(index.checked_mul(3)?..index.checked_mul(3)?.checked_add(3)?)?;
        if !(1..=16).contains(&color.info.bits)
            || channels.iter().any(|v| *v > color.info.maximum())
        {
            return None;
        }
        Some(Sample::RgbCodes {
            values: channels.try_into().ok()?,
            bits: color.info.bits,
        })
    } else if let Some(rgba) = &frame.rgba {
        let channels = rgba.get(index.checked_mul(4)?..index.checked_mul(4)?.checked_add(3)?)?;
        Some(Sample::SrgbDisplay(channels.try_into().ok()?))
    } else {
        Some(Sample::Scalar(*frame.pixels.get(index)?))
    }
}

#[cfg(test)]
mod tests;
