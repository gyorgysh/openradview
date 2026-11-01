// SPDX-License-Identifier: GPL-3.0-or-later
//! Allocation-free nearest-cell mapping shared by categorical overlays.
use crate::{study::Frame, vector::*, volume::Volume};
pub(crate) const MAX_PIXELS: usize = 4 * 1024 * 1024;
pub(crate) struct Cell {
    pub index: usize,
    pub z: usize,
    pub eligible: bool,
}
pub(crate) struct ThinPlane<'a> {
    source: &'a Volume,
    image: &'a Frame,
    dimensions: [usize; 2],
    source_size: [usize; 3],
    origin: [f64; 3],
    column: [f64; 3],
    row: [f64; 3],
}
impl<'a> ThinPlane<'a> {
    pub fn new(source: &'a Volume, image: &'a Frame) -> Result<Self, &'static str> {
        if image.slab.is_some() {
            return Err("Categorical overlays require a thin plane, not a slab projection");
        }
        let dimensions = [image.width as usize, image.height as usize];
        dimensions[0]
            .checked_mul(dimensions[1])
            .filter(|n| *n > 0 && *n <= MAX_PIXELS)
            .ok_or("Categorical planes are limited to four million pixels")?;
        if !image.valid_dimensions() {
            return Err("The completed image has invalid sample dimensions");
        }
        let geometry = image
            .geometry
            .as_ref()
            .ok_or("Categorical overlays need plane geometry")?;
        geometry
            .validate()
            .map_err(|_| "Categorical overlays need valid plane geometry")?;
        let reference = source
            .frames()
            .first()
            .and_then(|frame| frame.geometry.as_ref())
            .ok_or("Categorical overlays need source geometry")?;
        if geometry.frame_of_reference != reference.frame_of_reference
            || geometry.nifti_transform != reference.nifti_transform
        {
            return Err("Categorical plane and source references differ");
        }
        let (column, row) = geometry.directions();
        Ok(Self {
            source,
            image,
            dimensions,
            source_size: source.dimensions(),
            origin: geometry.position,
            column: mul(column, geometry.spacing[1]),
            row: mul(row, geometry.spacing[0]),
        })
    }
    pub fn dimensions(&self) -> [usize; 2] {
        self.dimensions
    }
    pub fn count(&self) -> usize {
        self.dimensions[0] * self.dimensions[1]
    }
    /// Nearest cells cover [-0.5, dimension-0.5); half ties select the larger index.
    pub fn cell(&self, index: usize) -> Result<Option<Cell>, &'static str> {
        let x = index % self.dimensions[0];
        let y = index / self.dimensions[0];
        let world = add(
            self.origin,
            add(mul(self.column, x as f64), mul(self.row, y as f64)),
        );
        let voxel = self.source.world_to_voxel(world);
        if !finite(world) || !finite(voxel) {
            return Err("Categorical plane coordinates exceed the supported range");
        }
        if voxel
            .iter()
            .zip(self.source_size)
            .any(|(v, size)| *v < -0.5 || *v >= size as f64 - 0.5)
        {
            return Ok(None);
        }
        let [x, y, z] = voxel.map(|v| (v + 0.5).floor() as usize);
        let pixel = y * self.source_size[0] + x;
        Ok(Some(Cell {
            index: z * self.source_size[0] * self.source_size[1] + pixel,
            z,
            eligible: self.image.has_sample(index) && self.source.frames()[z].has_sample(pixel),
        }))
    }
}
