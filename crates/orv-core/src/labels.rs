// SPDX-License-Identifier: GPL-3.0-or-later
//! Deterministic user labels. No anatomical inference, persistence or network access.
use crate::{study::Frame, volume::Volume};
use std::sync::Arc;
mod export;
#[cfg(test)]
mod tests;

pub const MAX_VOXELS: usize = 32 * 1024 * 1024;

#[derive(Clone)]
pub struct Labels {
    source: Arc<Volume>,
    values: Vec<u8>,
}
impl Labels {
    pub fn empty(source: Arc<Volume>) -> Result<Self, &'static str> {
        let count = source
            .dimensions()
            .iter()
            .try_fold(1usize, |a, b| a.checked_mul(*b))
            .filter(|n| *n <= MAX_VOXELS)
            .ok_or("Label maps are limited to 32 million voxels")?;
        Ok(Self {
            source,
            values: vec![0; count],
        })
    }
    pub fn source(&self) -> &Volume {
        &self.source
    }
    pub fn values(&self) -> &[u8] {
        &self.values
    }
    /// Numeric grid equality is necessary, not proof that two files describe the same anatomy.
    pub fn import(
        source: Arc<Volume>,
        labels: &Volume,
        cancel: impl Fn() -> bool,
    ) -> Result<Self, &'static str> {
        let mut result = Self::empty(source)?;
        let a = result.source.frames()[0]
            .geometry
            .as_ref()
            .ok_or("Missing source geometry")?;
        let b = labels.frames()[0]
            .geometry
            .as_ref()
            .ok_or("Missing label geometry")?;
        if result.source.dimensions() != labels.dimensions()
            || labels.is_color()
            || b.nifti_transform.map(|t| t.code) != Some(a.nifti_transform.map_or(1, |t| t.code))
            || [[0., 0., 0.], [1., 0., 0.], [0., 1., 0.], [0., 0., 1.]]
                .into_iter()
                .any(|p| {
                    result
                        .source
                        .voxel_to_world(p)
                        .into_iter()
                        .zip(labels.voxel_to_world(p))
                        .any(|(a, b)| (a - b).abs() > 1e-5)
                })
        {
            return Err(
                "Label dimensions, reference and affine must match; registration is not performed",
            );
        }
        for (z, frame) in labels.frames().iter().enumerate() {
            if cancel() {
                return Err("Cancelled");
            }
            let offset = z * frame.pixels.len();
            for (i, value) in frame.pixels.iter().enumerate() {
                if !value.is_finite() || *value < 0. || *value > 255. || value.fract() != 0. {
                    return Err("Label maps need integer values from 0 to 255");
                }
                result.values[offset + i] = *value as u8;
            }
        }
        Ok(result)
    }
    pub fn slice_index(&self, frame: &Frame) -> Option<usize> {
        let g = frame.geometry.as_ref()?;
        let voxel = self.source.world_to_voxel(g.position);
        let z = voxel[2].round();
        let index = z as usize;
        let reference = self.source.frames().get(index)?;
        (frame.width == reference.width
            && frame.height == reference.height
            && frame.geometry == reference.geometry
            && (voxel[2] - z).abs() < 1e-5)
            .then_some(index)
    }
    /// Mutations may be partial on cancellation/error: edit a private checkpoint and discard it on failure.
    /// Circular source-grid brush. Segments are filled at one-pixel intervals, including fast drags.
    pub fn paint(
        &mut self,
        z: usize,
        points: &[[f64; 2]],
        radius: u32,
        label: u8,
        cancel: impl Fn() -> bool,
    ) -> Result<(), &'static str> {
        let [w, h, d] = self.source.dimensions();
        if z >= d
            || !(1..=64).contains(&radius)
            || points.is_empty()
            || points.len() > 8192
            || points.iter().flatten().any(|p| !p.is_finite())
            || points
                .iter()
                .any(|p| p[0] < 0. || p[1] < 0. || p[0] >= w as f64 || p[1] >= h as f64)
        {
            return Err("Invalid label brush stroke");
        }
        let mut total = 0usize;
        let mut previous = points[0];
        for &point in points {
            let steps = (point[0] - previous[0])
                .hypot(point[1] - previous[1])
                .ceil()
                .max(1.) as usize;
            total = total
                .checked_add(steps)
                .filter(|n| *n <= 262144)
                .ok_or("Brush stroke is too long")?;
            if total.saturating_mul((radius as usize * 2 + 1).pow(2)) > 64 * 1024 * 1024 {
                return Err("Brush stroke is too large; use shorter strokes or a smaller radius");
            }
            for step in 0..=steps {
                if cancel() {
                    return Err("Cancelled");
                }
                let t = step as f64 / steps as f64;
                let x = previous[0] + (point[0] - previous[0]) * t;
                let y = previous[1] + (point[1] - previous[1]) * t;
                let r = f64::from(radius);
                for row in ((y - r).ceil().max(0.) as usize)
                    ..=((y + r).floor().min((h - 1) as f64) as usize)
                {
                    for col in ((x - r).ceil().max(0.) as usize)
                        ..=((x + r).floor().min((w - 1) as f64) as usize)
                    {
                        if (col as f64 - x).powi(2) + (row as f64 - y).powi(2) <= r * r {
                            self.values[(z * h + row) * w + col] = label;
                        }
                    }
                }
            }
            previous = point;
        }
        Ok(())
    }
    /// Replace only the selected label in this slice/all slices; other labels are preserved.
    pub fn threshold(
        &mut self,
        slice: Option<usize>,
        range: [f32; 2],
        label: u8,
        cancel: impl Fn() -> bool,
    ) -> Result<(), &'static str> {
        if self.source.is_color()
            || label == 0
            || range.iter().any(|v| !v.is_finite())
            || range[0] > range[1]
            || slice.is_some_and(|z| z >= self.source.frames().len())
        {
            return Err("Threshold needs scalar data, a nonzero label and a valid range");
        }
        for (z, frame) in self.source.frames().iter().enumerate() {
            if slice.is_some_and(|s| s != z) {
                continue;
            }
            for (i, row) in frame.pixels.chunks(frame.width as usize).enumerate() {
                if cancel() {
                    return Err("Cancelled");
                }
                for (x, value) in row.iter().enumerate() {
                    let slot =
                        &mut self.values[z * frame.pixels.len() + i * frame.width as usize + x];
                    if *slot == 0 || *slot == label {
                        *slot = if frame.has_sample(i * frame.width as usize + x)
                            && *value >= range[0]
                            && *value <= range[1]
                        {
                            label
                        } else {
                            0
                        };
                    }
                }
            }
        }
        Ok(())
    }
    pub fn plane_rgba(&self, z: usize) -> Result<Vec<u8>, &'static str> {
        let [w, h, d] = self.source.dimensions();
        if z >= d {
            return Err("Label slice is outside the source");
        }
        Ok(self.values[z * w * h..(z + 1) * w * h]
            .iter()
            .flat_map(|label| {
                // Neutral shades carry no anatomical meaning; IDs remain distinct in the exported map.
                let gray = 160 + (u16::from(*label) * 37 % 96) as u8;
                [gray, gray, gray, if *label == 0 { 0 } else { 255 }]
            })
            .collect())
    }
}
