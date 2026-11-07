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
mod reformat;
pub use reformat::ScalarReformatBuffer;

#[cfg(test)]
#[path = "volume_tests.rs"]
mod tests;

/// A bounded physical sampling grid prepared without allocating image pixels.
/// Both CPU and GPU reslicing use these exact dimensions and metadata.
#[derive(Clone, Debug)]
pub struct ReformatGrid {
    source: Arc<()>,
    geometry: Geometry,
    dimensions: [u32; 2],
    slab: Option<crate::slab::Slab>,
}
impl ReformatGrid {
    pub fn belongs_to(&self, volume: &Volume) -> bool {
        Arc::ptr_eq(&self.source, &volume.identity)
    }
    pub fn geometry(&self) -> &Geometry {
        &self.geometry
    }
    pub fn dimensions(&self) -> [u32; 2] {
        self.dimensions
    }
    pub fn slab(&self) -> Option<crate::slab::Slab> {
        self.slab
    }
}

#[derive(Debug)]
pub struct Volume {
    identity: Arc<()>,
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
            if frame.slab.is_some()
                || frame.width != first.width
                || frame.height != first.height
                || !frame.valid_dimensions()
            {
                return Err(GeometryError::Dimensions);
            }
            if frame.rgba.is_some() != first.rgba.is_some()
                || frame.color.as_ref().map(|d| &d.info) != first.color.as_ref().map(|d| &d.info)
                || frame.nifti != first.nifti
                || frame.monochrome1 != first.monochrome1
                || !match (&first.scalar, &frame.scalar) {
                    (Some(a), Some(b)) => a.compatible(b),
                    (None, None) => true,
                    _ => false,
                }
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
        let geometry = crate::spatial::RegularGeometry::prepare(
            frames.len(),
            |index| {
                frames
                    .get(index)
                    .map(|frame| crate::navigation::SpatialFrame {
                        size: [frame.width, frame.height],
                        geometry: frame.geometry.clone(),
                    })
            },
            &cancelled,
        )?;
        let normal = reference.normal();
        let source_frames = frames.clone();
        frames.sort_by(|a, b| {
            let position = |f: &Frame| {
                f.geometry
                    .as_ref()
                    .map(|g| dot(g.position, normal))
                    .unwrap_or(0.)
            };
            position(a).total_cmp(&position(b))
        });
        let crate::spatial::RegularGeometry {
            dimensions,
            origin,
            basis,
            inverse,
            step,
        } = geometry;
        let has_missing_scalar = frames.iter().any(|f| f.validity.is_some());
        let mut volume = Self {
            identity: Arc::new(()),
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
    /// Transform a displacement without adding/subtracting the world origin.
    pub fn voxel_vector_to_world(&self, vector: V3) -> V3 {
        add(
            mul(self.basis[0], vector[0]),
            add(mul(self.basis[1], vector[1]), mul(self.basis[2], vector[2])),
        )
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
    /// Physical volume of one source-grid cell, including any regular shear.
    pub fn voxel_volume_mm3(&self) -> f64 {
        dot(self.basis[0], cross(self.basis[1], self.basis[2])).abs()
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
        let grid = self.reformat_grid(plane, point, max_side)?;
        self.resample_grid(&grid, cancel)
    }
    pub fn reformat_grid(
        &self,
        plane: Plane,
        point: V3,
        max_side: u32,
    ) -> Result<ReformatGrid, GeometryError> {
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
        self.sampling_grid(position, u, v, extent, (max_side, None))
    }
    pub(crate) fn sampling_grid(
        &self,
        position: V3,
        u: V3,
        v: V3,
        extent: [f64; 2],
        request: (u32, Option<crate::slab::Slab>),
    ) -> Result<ReformatGrid, GeometryError> {
        let (max_side, slab) = request;
        if slab.is_some_and(|s| s.validate().is_err())
            || !(2..=2048).contains(&max_side)
            || extent.iter().any(|e| !e.is_finite() || *e <= 0.0)
        {
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
        if self.is_color() && slab.is_some() {
            return Err(GeometryError::Dimensions);
        }
        Ok(ReformatGrid {
            source: self.identity.clone(),
            geometry,
            dimensions,
            slab,
        })
    }
    /// Sample a prepared grid on a worker. Grids belong to one exact volume;
    /// matching geometry alone does not authorize sampling another source.
    pub fn resample_grid(
        &self,
        grid: &ReformatGrid,
        cancel: impl Fn() -> bool,
    ) -> Result<Arc<Frame>, GeometryError> {
        if !grid.belongs_to(self) {
            return Err(GeometryError::Reference);
        }
        let ReformatGrid {
            geometry,
            dimensions,
            slab,
            ..
        } = grid;
        let geometry = geometry.clone();
        let dimensions = *dimensions;
        let slab = *slab;
        let position = geometry.position;
        let u = [
            geometry.orientation[0],
            geometry.orientation[1],
            geometry.orientation[2],
        ];
        let v = [
            geometry.orientation[3],
            geometry.orientation[4],
            geometry.orientation[5],
        ];
        let spacing = [geometry.spacing[1], geometry.spacing[0]];
        if self.is_color() {
            if slab.is_some() {
                return Err(GeometryError::Dimensions);
            }
            return self.resample_color(geometry, dimensions, cancel);
        }
        let mut output = self.scalar_reformat_buffer(grid)?;
        let pixels = output.pixels_mut();
        let origin = self.world_to_voxel(position);
        let du = self.world_vector_to_voxel(mul(u, spacing[0]));
        let dv = self.world_vector_to_voxel(mul(v, spacing[1]));
        let normal = self.world_vector_to_voxel(cross(u, v));
        // Grid validation caps slabs at 256 intervals. The displacement is
        // identical for every pixel and keeps the reference arithmetic order.
        let mut offsets = [[0.; 3]; 257];
        if let Some(slab) = slab {
            for (i, offset) in offsets
                .iter_mut()
                .take(usize::from(slab.intervals) + 1)
                .enumerate()
            {
                let distance = slab.thickness_mm * (i as f64 / f64::from(slab.intervals) - 0.5);
                *offset = mul(normal, distance);
            }
        }
        for y in 0..dimensions[1] {
            if cancel() {
                return Err(GeometryError::Cancelled);
            }
            let row = add(origin, mul(dv, y as f64));
            for x in 0..dimensions[0] {
                let center = add(row, mul(du, x as f64));
                let value = if let Some(slab) = slab {
                    let mut extreme: Option<f32> = None;
                    // At most 257 finite f32 samples: an f64 sum cannot overflow
                    // and stays well inside f32 output precision.
                    let (mut sum, mut count) = (0f64, 0u32);
                    for (i, offset) in offsets
                        .iter()
                        .take(usize::from(slab.intervals) + 1)
                        .enumerate()
                    {
                        if i % 16 == 0 && cancel() {
                            return Err(GeometryError::Cancelled);
                        }
                        if let Some(value) = self.sample_voxel(add(center, *offset)) {
                            match slab.projection {
                                crate::slab::SlabProjection::Maximum => {
                                    extreme = Some(extreme.map_or(value, |old| old.max(value)));
                                }
                                crate::slab::SlabProjection::Minimum => {
                                    extreme = Some(extreme.map_or(value, |old| old.min(value)));
                                }
                                crate::slab::SlabProjection::Mean => {
                                    sum += f64::from(value);
                                    count += 1;
                                }
                            }
                        }
                    }
                    // An all-missing ray stays missing for every projection.
                    match slab.projection {
                        crate::slab::SlabProjection::Mean => {
                            (count > 0).then(|| (sum / f64::from(count)) as f32)
                        }
                        _ => extreme,
                    }
                } else {
                    self.sample_voxel(center)
                };
                pixels[y as usize * dimensions[0] as usize + x as usize] =
                    value.unwrap_or(f32::NAN);
            }
        }
        Ok(output.finish())
    }
}
