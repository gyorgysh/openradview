// SPDX-License-Identifier: GPL-3.0-or-later
//! Deterministic user labels. No anatomical inference, persistence or network access.
use crate::{resources::MemoryBudget, study::Frame, volume::Volume};
use std::sync::Arc;
mod export;
mod physical;
mod regions;
mod sampling;
mod seeded;
mod statistics;
mod surface;
mod tiles;
pub use physical::{
    BrushAction, BrushShape, MAX_PHYSICAL_BRUSH_RADIUS_MM, MAX_STROKE_CANDIDATES,
    MAX_STROKE_POINTS, PhysicalBrush, PhysicalStroke, StrokeSummary,
};
pub use regions::{
    LabelScope, LabelSelection, RegionOperation, SelectionRequest, SelectionSummary,
};
pub use sampling::{LabelPlane, MAX_PLANE_PIXELS};
pub use seeded::{SeedClass, SeedPoint, SeededRegion, SeededRequest, SeededSummary};
pub use statistics::{LabelCount, LabelStatistics};
pub use surface::{FaceErase, FaceEraseSelection, FaceEraseSummary, MAX_ERASE_FACES};
pub use tiles::{
    LABEL_TILE_SIDE, LabelObjectId, LabelRevision, LabelStorageStatistics, LabelTile,
    ValueIter as LabelValueIter, Values as LabelValues,
};
#[cfg(test)]
mod sampling_tests;
#[cfg(test)]
mod tests;

pub const MAX_VOXELS: usize = 32 * 1024 * 1024;

pub struct Labels {
    source: Arc<Volume>,
    values: tiles::Store,
    object: LabelObjectId,
    revision: LabelRevision,
    parent: Option<LabelRevision>,
}
impl Labels {
    /// Header-only admission can call this before preparing or decoding a volume.
    pub fn check_dimensions(dimensions: [usize; 3]) -> Result<usize, &'static str> {
        let plane = dimensions[0]
            .checked_mul(dimensions[1])
            .filter(|n| *n > 0 && *n <= MAX_PLANE_PIXELS)
            .ok_or("Label editing is limited to four million pixels per slice")?;
        plane
            .checked_mul(dimensions[2])
            .filter(|n| *n > 0 && *n <= MAX_VOXELS)
            .ok_or("Label maps are limited to 32 million voxels")
    }
    pub fn empty(source: Arc<Volume>) -> Result<Self, &'static str> {
        Self::empty_with_budget(source, MemoryBudget::volumes())
    }
    fn empty_with_budget(
        source: Arc<Volume>,
        budget: Arc<MemoryBudget>,
    ) -> Result<Self, &'static str> {
        let values = tiles::Store::empty(source.dimensions(), LABEL_TILE_SIDE, budget.clone())?;
        let (object, revision) = LabelObjectId::new(&budget)?;
        Ok(Self {
            source,
            values,
            object,
            revision,
            parent: None,
        })
    }
    /// Worker-only private checkpoint. Unchanged pages retain their allocation
    /// charge once across all readers and history revisions.
    pub fn try_clone(&self, cancelled: impl Fn() -> bool) -> Result<Self, &'static str> {
        if cancelled() {
            return Err("Cancelled");
        }
        Ok(Self {
            source: self.source.clone(),
            values: self.values.try_clone(cancelled)?,
            object: self.object.clone(),
            revision: self.revision.clone(),
            parent: None,
        })
    }
    pub fn object_id(&self) -> &LabelObjectId {
        &self.object
    }
    pub fn revision(&self) -> &LabelRevision {
        &self.revision
    }
    /// Tile changes are applicable only when the consumer has this exact parent.
    /// A replacement object or a nonconsecutive history revision needs a fresh upload.
    pub fn parent_revision(&self) -> Option<&LabelRevision> {
        self.parent.as_ref()
    }
    pub fn changed_tiles(&self) -> impl Iterator<Item = LabelTile> + '_ {
        self.values.changed_tiles()
    }
    pub fn tiles(&self) -> impl Iterator<Item = LabelTile> + '_ {
        self.values.tiles()
    }
    /// Worker-only bounded tile read, x-fastest within the clipped source-cell bounds.
    /// The caller admits its output buffer; source validity is not substituted for IDs.
    pub fn copy_tile(&self, tile: LabelTile, output: &mut [u8]) -> Result<(), &'static str> {
        self.values.copy_tile(tile, output)
    }
    /// Conservative tile differences for forward edits or undo/redo. Consumers
    /// must still match their resident object/revision to `previous` before upload.
    pub fn changed_tiles_since<'a>(
        &'a self,
        previous: &'a Self,
    ) -> Result<impl Iterator<Item = LabelTile> + 'a, &'static str> {
        if self.object != previous.object || !Arc::ptr_eq(&self.source, &previous.source) {
            return Err("Label changes belong to another object or source");
        }
        self.values.changed_since(&previous.values)
    }
    pub fn storage_statistics(&self) -> LabelStorageStatistics {
        self.values.statistics()
    }
    fn begin_edit(&mut self) {
        self.values.reset_changes();
        self.parent = None;
    }
    fn set_value(&mut self, index: usize, value: u8) -> Result<(), &'static str> {
        self.set_values(index, &[value])
    }
    fn set_values(&mut self, index: usize, values: &[u8]) -> Result<(), &'static str> {
        if values.is_empty() {
            return Ok(());
        }
        let previous = self
            .values
            .run(index)
            .and_then(|run| run.get(..values.len()))
            .ok_or("Label write crosses a tile row")?;
        if previous == values {
            return Ok(());
        }
        let revision = (!self.values.has_changes())
            .then(|| LabelRevision::next(self.values.budget()))
            .transpose()?;
        self.values.write_run(index, values)?;
        if let Some(revision) = revision {
            self.parent = Some(self.revision.clone());
            self.revision = revision;
        }
        Ok(())
    }
    pub fn source(&self) -> &Volume {
        &self.source
    }
    pub fn source_arc(&self) -> Arc<Volume> {
        self.source.clone()
    }
    pub fn values(&self) -> LabelValues<'_> {
        // The layout has already been admitted against the exact source dimensions.
        self.values.view()
    }
    /// Stored categorical ID, independently of source sample validity.
    pub fn value(&self, voxel: [usize; 3]) -> Option<u8> {
        let [w, h, d] = self.source.dimensions();
        let [x, y, z] = voxel;
        if x >= w || y >= h || z >= d {
            return None;
        }
        self.values.get((z * h + y) * w + x).copied()
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
            let mut i = 0;
            while i < frame.pixels.len() {
                if cancel() {
                    return Err("Cancelled");
                }
                let count = result
                    .values
                    .run(offset + i)
                    .ok_or("Label tile is inconsistent")?
                    .len()
                    .min(frame.pixels.len() - i);
                if count == 0 {
                    return Err("Label tile is inconsistent");
                }
                let mut values = [0u8; 64];
                for (target, value) in values[..count].iter_mut().zip(&frame.pixels[i..i + count]) {
                    if !value.is_finite() || *value < 0. || *value > 255. || value.fract() != 0. {
                        return Err("Label maps need integer values from 0 to 255");
                    }
                    *target = *value as u8;
                }
                result.set_values(offset + i, &values[..count])?;
                i += count;
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
        // The acquired geometry is authoritative. Accepted regular stacks can
        // retain decimal-position residuals relative to their fitted affine.
        (frame.width == reference.width
            && frame.height == reference.height
            && frame.geometry == reference.geometry)
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
        self.begin_edit();
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
                            self.set_value((z * h + row) * w + col, label)?;
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
        self.begin_edit();
        let source = self.source.clone();
        for (z, frame) in source.frames().iter().enumerate() {
            if slice.is_some_and(|s| s != z) {
                continue;
            }
            // Regular scalar volumes already reject nonfinite payloads. Without
            // a supplied validity mask every source sample is present.
            let mut pixel = 0;
            while pixel < frame.pixels.len() {
                if cancel() {
                    return Err("Cancelled");
                }
                let index = z * frame.pixels.len() + pixel;
                let current = self.values.run(index).ok_or("Label tile is inconsistent")?;
                let count = current.len().min(frame.pixels.len() - pixel);
                if count == 0 {
                    return Err("Label tile is inconsistent");
                }
                let mut values = [0u8; 64];
                for local in 0..count {
                    let old = current[local];
                    let value = frame.pixels[pixel + local];
                    values[local] = if old != 0 && old != label {
                        old
                    } else if (frame.validity.is_none() || frame.has_sample(pixel + local))
                        && value >= range[0]
                        && value <= range[1]
                    {
                        label
                    } else {
                        0
                    };
                }
                self.set_values(index, &values[..count])?;
                pixel += count;
            }
        }
        Ok(())
    }
    pub fn plane_rgba(&self, z: usize) -> Result<Vec<u8>, &'static str> {
        let [w, h, d] = self.source.dimensions();
        if z >= d {
            return Err("Label slice is outside the source");
        }
        let values = self
            .values
            .range(z * w * h..(z + 1) * w * h)
            .ok_or("Label slice is outside the source")?;
        Ok(values
            .iter()
            .flat_map(|label| {
                // Neutral shades carry no anatomical meaning; IDs remain distinct in the exported map.
                let gray = 160 + (u16::from(*label) * 37 % 96) as u8;
                [gray, gray, gray, if *label == 0 { 0 } else { 255 }]
            })
            .collect())
    }
}
