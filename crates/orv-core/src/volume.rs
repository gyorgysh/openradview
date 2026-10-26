// SPDX-License-Identifier: GPL-3.0-or-later
//! Immutable regular affine volumes, including regular tilted stacks. No UI or GPU dependencies.
use crate::{
    Plane,
    spatial::GeometryError,
    study::{Frame, Geometry},
    vector::*,
};
use std::sync::Arc;
mod color;

#[cfg(test)]
#[path = "volume_tests.rs"]
mod tests;

#[derive(Debug)]
pub struct Volume {
    frames: Vec<Arc<Frame>>,
    source_frames: Vec<Arc<Frame>>,
    dimensions: [usize; 3],
    origin: V3,
    basis: [V3; 3],
    inverse: [V3; 3],
    bounds: [V3; 2],
    step: f64,
    has_missing_scalar: bool,
    reservation: Option<crate::resources::Reservation>,
}
impl Volume {
    /// Worker-only admission for resident sources. Indexed inputs reserve before decoding.
    pub fn prepare(
        frames: Vec<Arc<Frame>>,
        cancelled: impl Fn() -> bool,
    ) -> Result<Arc<Self>, String> {
        crate::resources::refresh_memory_policy();
        let bytes = frames
            .iter()
            .try_fold(0u64, |sum, frame| sum.checked_add(frame.byte_len()))
            .ok_or("Volume size overflow")?;
        let budget = crate::resources::MemoryBudget::volumes();
        let reservation = budget.reserve(bytes).ok_or_else(|| {
            format!(
                "Active volumes exceed the current shared {} MiB memory allowance; use 2D browsing",
                budget.limit() / (1024 * 1024)
            )
        })?;
        let mut volume =
            Self::new_cancellable(frames, cancelled).map_err(|error| error.to_string())?;
        volume.retain_reservation(reservation);
        Ok(Arc::new(volume))
    }
    /// Validate before anatomical labels or physical resampling. Source pixels are shared, not copied.
    pub fn new(frames: Vec<Arc<Frame>>) -> Result<Self, GeometryError> {
        Self::new_cancellable(frames, || false)
    }
    pub fn new_cancellable(
        mut frames: Vec<Arc<Frame>>,
        cancelled: impl Fn() -> bool,
    ) -> Result<Self, GeometryError> {
        if cancelled() {
            return Err(GeometryError::Cancelled);
        }
        if frames.len() < 2 {
            return Err(GeometryError::TooFew);
        }
        let first = &frames[0];
        let reference = first.geometry.as_ref().ok_or(GeometryError::Missing)?;
        reference.validate()?;
        let uid = reference
            .frame_of_reference
            .as_deref()
            .filter(|s| !s.is_empty())
            .ok_or(GeometryError::Reference)?;
        if first.width < 2 || first.height < 2 {
            return Err(GeometryError::Dimensions);
        }
        let dimensions = [first.width as usize, first.height as usize, frames.len()];
        if frames
            .iter()
            .try_fold(0u64, |sum, frame| sum.checked_add(frame.byte_len()))
            .is_none_or(|bytes| bytes > crate::index::MAX_VOLUME_BYTES)
        {
            return Err(GeometryError::Limit);
        }
        if dimensions
            .iter()
            .try_fold(1usize, |a, b| a.checked_mul(*b))
            .ok_or(GeometryError::Limit)?
            > (crate::resources::MAX_VOLUME_BYTES / 4) as usize
        {
            return Err(GeometryError::Limit);
        }
        for frame in &frames {
            if cancelled() {
                return Err(GeometryError::Cancelled);
            }
            if frame.width != first.width
                || frame.height != first.height
                || !frame.valid_dimensions()
            {
                return Err(GeometryError::Dimensions);
            }
            if frame.rgba.is_some() != first.rgba.is_some()
                || frame.color.as_ref().map(|d| &d.info) != first.color.as_ref().map(|d| &d.info)
                || frame.nifti != first.nifti
                || frame.monochrome1 != first.monochrome1
            {
                return Err(GeometryError::Pixels);
            }
            for block in frame.pixels.chunks(16384) {
                if cancelled() {
                    return Err(GeometryError::Cancelled);
                }
                if block.iter().any(|value| !value.is_finite()) {
                    return Err(GeometryError::Pixels);
                }
            }
            let g = frame.geometry.as_ref().ok_or(GeometryError::Missing)?;
            g.validate()?;
            if g.frame_of_reference.as_deref() != Some(uid)
                || g.nifti_transform != reference.nifti_transform
            {
                return Err(GeometryError::Reference);
            }
            if g.orientation
                .iter()
                .zip(reference.orientation)
                .any(|(a, b)| (a - b).abs() > 1e-4)
            {
                return Err(GeometryError::Orientation);
            }
            if g.spacing
                .iter()
                .zip(reference.spacing)
                .any(|(a, b)| (a - b).abs() > 1e-5 * b.max(1.0))
            {
                return Err(GeometryError::Spacing);
            }
        }
        let normal = reference.normal();
        let source_frames = frames.clone();
        frames.sort_by(|a, b| {
            let position = |f: &Frame| {
                f.geometry
                    .as_ref()
                    .map(|g| dot(g.position, normal))
                    .unwrap_or(0.0)
            };
            position(a).total_cmp(&position(b))
        });
        let origin = frames[0]
            .geometry
            .as_ref()
            .ok_or(GeometryError::Missing)?
            .position;
        let last = frames[frames.len() - 1]
            .geometry
            .as_ref()
            .ok_or(GeometryError::Missing)?
            .position;
        let delta = mul(sub(last, origin), 1.0 / (frames.len() - 1) as f64);
        let dz = dot(delta, normal);
        if !dz.is_finite() || dz <= 1e-5 {
            return Err(GeometryError::Irregular);
        }
        for (i, frame) in frames.iter().enumerate() {
            let p = frame
                .geometry
                .as_ref()
                .ok_or(GeometryError::Missing)?
                .position;
            if norm(sub(p, add(origin, mul(delta, i as f64)))) > 0.01f64.max(dz * 0.01) {
                return Err(GeometryError::Irregular);
            }
        }
        let reference = frames[0].geometry.as_ref().ok_or(GeometryError::Missing)?;
        let (u, v) = reference.directions();
        let basis = [
            mul(u, reference.spacing[1]),
            mul(v, reference.spacing[0]),
            delta,
        ];
        let det = dot(basis[0], cross(basis[1], basis[2]));
        if !det.is_finite() || det.abs() < 1e-12 {
            return Err(GeometryError::Irregular);
        }
        let inverse = [
            mul(cross(basis[1], basis[2]), 1.0 / det),
            mul(cross(basis[2], basis[0]), 1.0 / det),
            mul(cross(basis[0], basis[1]), 1.0 / det),
        ];
        let step = reference.spacing[0].min(reference.spacing[1]).min(dz);
        let has_missing_scalar = frames.iter().any(|f| f.validity.is_some());
        let mut volume = Self {
            frames,
            source_frames,
            dimensions,
            origin,
            basis,
            inverse,
            bounds: [[f64::INFINITY; 3], [f64::NEG_INFINITY; 3]],
            step,
            has_missing_scalar,
            reservation: None,
        };
        for x in [0.0, (dimensions[0] - 1) as f64] {
            for y in [0.0, (dimensions[1] - 1) as f64] {
                for z in [0.0, (dimensions[2] - 1) as f64] {
                    let corner = volume.voxel_to_world([x, y, z]);
                    if !finite(corner) {
                        return Err(GeometryError::Limit);
                    }
                    for (axis, value) in corner.into_iter().enumerate() {
                        volume.bounds[0][axis] = volume.bounds[0][axis].min(value);
                        volume.bounds[1][axis] = volume.bounds[1][axis].max(value);
                    }
                }
            }
        }
        Ok(volume)
    }
    pub(crate) fn retain_reservation(&mut self, reservation: crate::resources::Reservation) {
        self.reservation = Some(reservation);
    }
    pub fn dimensions(&self) -> [usize; 3] {
        self.dimensions
    }
    pub fn has_missing_scalar(&self) -> bool {
        self.has_missing_scalar
    }
    /// Sorted affine slices, shared for cancellable GPU upload without flattening.
    pub fn frames(&self) -> &[Arc<Frame>] {
        &self.frames
    }
    /// Original source order for per-frame approval checks, before spatial sorting.
    pub fn source_frames(&self) -> &[Arc<Frame>] {
        &self.source_frames
    }
    pub fn world_vector_to_voxel(&self, vector: V3) -> V3 {
        self.inverse.map(|row| dot(row, vector))
    }
    pub fn corners(&self) -> [V3; 8] {
        std::array::from_fn(|i| {
            self.voxel_to_world(std::array::from_fn(|axis| {
                if i & (1 << axis) == 0 {
                    0.0
                } else {
                    (self.dimensions[axis] - 1) as f64
                }
            }))
        })
    }
    pub fn bounds(&self) -> [V3; 2] {
        self.bounds
    }
    pub fn step_mm(&self) -> f64 {
        self.step
    }
    pub fn center(&self) -> V3 {
        self.voxel_to_world(self.dimensions.map(|d| (d - 1) as f64 * 0.5))
    }
    pub fn voxel_to_world(&self, p: V3) -> V3 {
        add(
            self.origin,
            add(
                mul(self.basis[0], p[0]),
                add(mul(self.basis[1], p[1]), mul(self.basis[2], p[2])),
            ),
        )
    }
    pub fn world_to_voxel(&self, p: V3) -> V3 {
        let d = sub(p, self.origin);
        self.inverse.map(|row| dot(row, d))
    }
    pub fn clamp_world(&self, p: V3) -> Option<V3> {
        if !finite(p) {
            return None;
        }
        let mut v = self.world_to_voxel(p);
        for (axis, value) in v.iter_mut().enumerate() {
            *value = value.clamp(0.0, (self.dimensions[axis] - 1) as f64);
        }
        Some(self.voxel_to_world(v))
    }
    pub fn sample(&self, world: V3) -> Option<f32> {
        self.sample_voxel(self.world_to_voxel(world))
    }
    pub(crate) fn sample_voxel(&self, v: V3) -> Option<f32> {
        if self.is_color() {
            return None;
        }
        if !finite(v) {
            return None;
        }
        if v.iter()
            .enumerate()
            .any(|(i, p)| *p < -1e-6 || *p > (self.dimensions[i] - 1) as f64 + 1e-6)
        {
            return None;
        }
        let v: V3 = std::array::from_fn(|i| v[i].clamp(0.0, (self.dimensions[i] - 1) as f64));
        let lo = v.map(|p| p.floor() as usize);
        let hi: [usize; 3] = std::array::from_fn(|i| (lo[i] + 1).min(self.dimensions[i] - 1));
        let t: V3 = std::array::from_fn(|i| v[i] - lo[i] as f64);
        if !self.has_missing_scalar {
            let width = self.dimensions[0];
            let row0 = lo[1] * width;
            let row1 = hi[1] * width;
            let plane = |z: usize| {
                let data = &self.frames[z].pixels;
                let mix = |row: usize| {
                    data[row + lo[0]] as f64 * (1. - t[0]) + data[row + hi[0]] as f64 * t[0]
                };
                mix(row0) * (1. - t[1]) + mix(row1) * t[1]
            };
            return Some((plane(lo[2]) * (1. - t[2]) + plane(hi[2]) * t[2]) as f32);
        }
        let mut value = 0.0;
        for (z, wz) in [(lo[2], 1.0 - t[2]), (hi[2], t[2])] {
            for (y, wy) in [(lo[1], 1.0 - t[1]), (hi[1], t[1])] {
                for (x, wx) in [(lo[0], 1.0 - t[0]), (hi[0], t[0])] {
                    let weight = wx * wy * wz;
                    if weight == 0. {
                        continue;
                    }
                    let index = y * self.dimensions[0] + x;
                    if self.has_missing_scalar && !self.frames[z].has_sample(index) {
                        return None;
                    }
                    value += self.frames[z].pixels[index] as f64 * weight;
                }
            }
        }
        Some(value as f32)
    }
    pub fn reformat(
        &self,
        plane: Plane,
        point: V3,
        max_side: u32,
        cancel: impl Fn() -> bool,
    ) -> Result<Arc<Frame>, GeometryError> {
        if !(2..=2048).contains(&max_side) || !finite(point) {
            return Err(GeometryError::Limit);
        }
        let [min, max] = self.bounds;
        let (position, u, v, extent) = match plane {
            Plane::Axial => (
                [min[0], min[1], point[2]],
                [1., 0., 0.],
                [0., 1., 0.],
                [max[0] - min[0], max[1] - min[1]],
            ),
            Plane::Coronal => (
                [min[0], point[1], max[2]],
                [1., 0., 0.],
                [0., 0., -1.],
                [max[0] - min[0], max[2] - min[2]],
            ),
            Plane::Sagittal => (
                [point[0], min[1], max[2]],
                [0., 1., 0.],
                [0., 0., -1.],
                [max[1] - min[1], max[2] - min[2]],
            ),
            Plane::Acquired => return Err(GeometryError::Orientation),
        };
        self.resample(position, u, v, extent, max_side, cancel)
    }
    pub(crate) fn resample(
        &self,
        position: V3,
        u: V3,
        v: V3,
        extent: [f64; 2],
        max_side: u32,
        cancel: impl Fn() -> bool,
    ) -> Result<Arc<Frame>, GeometryError> {
        if !(2..=2048).contains(&max_side) || extent.iter().any(|e| !e.is_finite() || *e <= 0.0) {
            return Err(GeometryError::Limit);
        }
        let step = self
            .step
            .max(extent[0].max(extent[1]) / (max_side - 1) as f64);
        let dimensions = extent.map(|e| ((e / step).ceil() as u32 + 1).clamp(2, max_side));
        let spacing = [
            extent[0] / (dimensions[0] - 1) as f64,
            extent[1] / (dimensions[1] - 1) as f64,
        ];
        let geometry = Geometry {
            nifti_transform: self.frames[0]
                .geometry
                .as_ref()
                .and_then(|g| g.nifti_transform),
            frame_of_reference: self.frames[0]
                .geometry
                .as_ref()
                .and_then(|g| g.frame_of_reference.clone()),
            position,
            orientation: [u[0], u[1], u[2], v[0], v[1], v[2]],
            spacing: [spacing[1], spacing[0]],
        };
        geometry.validate()?;
        if self.is_color() {
            return self.resample_color(geometry, dimensions, cancel);
        }
        let mut pixels = Vec::with_capacity(dimensions[0] as usize * dimensions[1] as usize);
        let origin = self.world_to_voxel(position);
        let du = self.world_vector_to_voxel(mul(u, spacing[0]));
        let dv = self.world_vector_to_voxel(mul(v, spacing[1]));
        for y in 0..dimensions[1] {
            if cancel() {
                return Err(GeometryError::Cancelled);
            }
            let row = add(origin, mul(dv, y as f64));
            for x in 0..dimensions[0] {
                pixels.push(
                    self.sample_voxel(add(row, mul(du, x as f64)))
                        .unwrap_or(f32::NAN),
                );
            }
        }
        Ok(Arc::new(Frame {
            validity: None,
            color: None,
            nifti: self.frames.first().and_then(|frame| frame.nifti.clone()),
            rgba: None,
            width: dimensions[0],
            height: dimensions[1],
            pixels: pixels.into(),
            geometry: Some(geometry),
            instance: 0,
            frame_index: 0,
            monochrome1: self.frames[0].monochrome1,
            default_window: self.frames[0].default_window,
        }))
    }
}
