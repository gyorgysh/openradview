// SPDX-License-Identifier: GPL-3.0-or-later
//! Categorical labels on a thin plane; scalar interpolation never blends IDs.
use super::Labels;
use crate::{
    resources::{MemoryBudget, Reservation},
    study::Frame,
    vector::*,
};

pub const MAX_PLANE_PIXELS: usize = 4 * 1024 * 1024;

pub struct LabelPlane {
    dimensions: [usize; 2],
    ids: Vec<u8>,
    eligible: Vec<u8>,
    _reservation: Reservation,
}
impl LabelPlane {
    pub fn dimensions(&self) -> [usize; 2] {
        self.dimensions
    }
    /// Preserves the nearest stored ID even when that source sample is absent.
    /// Outside the grid, the ID and eligibility are both zero.
    pub fn ids(&self) -> &[u8] {
        &self.ids
    }
    /// One only if the nearest source voxel and completed output sample exist.
    pub fn eligibility(&self) -> &[u8] {
        &self.eligible
    }
}
impl Labels {
    /// Worker-only. The caller must bind the completed image to this exact source
    /// and reformat generation; matching declared geometry alone is not identity.
    /// Nearest cells cover [-0.5, dimension-0.5), with half ties toward the larger
    /// index. Source order follows the volume affine, including negative steps.
    pub fn sample_plane(
        &self,
        image: &Frame,
        cancelled: impl Fn() -> bool,
    ) -> Result<LabelPlane, &'static str> {
        if cancelled() {
            return Err("Cancelled");
        }
        if image.slab.is_some() {
            return Err("Label overlays require a thin plane, not a slab projection");
        }
        let dimensions = [image.width as usize, image.height as usize];
        let count = dimensions[0]
            .checked_mul(dimensions[1])
            .filter(|n| *n > 0 && *n <= MAX_PLANE_PIXELS)
            .ok_or("Label planes are limited to four million pixels")?;
        if !image.valid_dimensions() {
            return Err("The completed image has invalid sample dimensions");
        }
        let geometry = image
            .geometry
            .as_ref()
            .ok_or("Label overlays need plane geometry")?;
        geometry
            .validate()
            .map_err(|_| "Label overlays need valid plane geometry")?;
        let reference = self
            .source
            .frames()
            .first()
            .and_then(|frame| frame.geometry.as_ref())
            .ok_or("Labels need source geometry")?;
        if geometry.frame_of_reference != reference.frame_of_reference
            || geometry.nifti_transform != reference.nifti_transform
        {
            return Err("Label plane and source references differ");
        }
        let reservation = MemoryBudget::volumes()
            .reserve(count as u64 * 2)
            .ok_or("Not enough memory for a label plane")?;
        let mut ids = Vec::new();
        let mut eligible = Vec::new();
        ids.try_reserve_exact(count)
            .map_err(|_| "Not enough memory for label IDs")?;
        eligible
            .try_reserve_exact(count)
            .map_err(|_| "Not enough memory for label eligibility")?;
        let source_size = self.source.dimensions();
        let (column, row) = geometry.directions();
        let column = mul(column, geometry.spacing[1]);
        let row = mul(row, geometry.spacing[0]);
        for index in 0..count {
            if index.is_multiple_of(1024) && cancelled() {
                return Err("Cancelled");
            }
            let x = index % dimensions[0];
            let y = index / dimensions[0];
            let world = add(
                geometry.position,
                add(mul(column, x as f64), mul(row, y as f64)),
            );
            let voxel = self.source.world_to_voxel(world);
            if !finite(world) || !finite(voxel) {
                return Err("Label plane coordinates exceed the supported range");
            }
            if voxel
                .iter()
                .zip(source_size)
                .any(|(v, size)| *v < -0.5 || *v >= size as f64 - 0.5)
            {
                ids.push(0);
                eligible.push(0);
                continue;
            }
            let [x, y, z] = voxel.map(|v| (v + 0.5).floor() as usize);
            let pixel = y * source_size[0] + x;
            ids.push(self.values[z * source_size[0] * source_size[1] + pixel]);
            eligible.push(u8::from(
                image.has_sample(index) && self.source.frames()[z].has_sample(pixel),
            ));
        }
        if cancelled() {
            return Err("Cancelled");
        }
        Ok(LabelPlane {
            dimensions,
            ids,
            eligible,
            _reservation: reservation,
        })
    }
}
