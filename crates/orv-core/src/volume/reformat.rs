// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use crate::resources::{MemoryBudget, Reservation};

/// Fixed-size, admitted scalar staging. No incomplete Frame can escape it.
pub struct ScalarReformatBuffer {
    grid: ReformatGrid,
    template: Arc<Frame>,
    pixels: Vec<f32>,
    memory: Arc<Reservation>,
}
impl ScalarReformatBuffer {
    pub fn pixels_mut(&mut self) -> &mut [f32] {
        &mut self.pixels
    }
    pub fn finish(self) -> Arc<Frame> {
        let [width, height] = self.grid.dimensions;
        Arc::new(Frame {
            memory: Some(self.memory),
            presentation: None,
            calibration: None,
            projection: None,
            sop: None,
            slab: self.grid.slab,
            scalar: self.template.scalar.as_ref().map(|data| data.resampled()),
            validity: None,
            color: None,
            nifti: self.template.nifti.clone(),
            rgba: None,
            width,
            height,
            pixels: self.pixels.into(),
            geometry: Some(self.grid.geometry),
            instance: 0,
            frame_index: 0,
            monochrome1: self.template.monochrome1,
            default_window: self.template.default_window,
        })
    }
}
impl Volume {
    /// Evaluate an exact requested grid pixel on a worker, including all declared
    /// slab samples. This shares the CPU reslicer's rebased evaluation order.
    pub fn reformat_sample(
        &self,
        grid: &ReformatGrid,
        pixel: [u32; 2],
        cancelled: impl Fn() -> bool,
    ) -> Result<Option<f32>, GeometryError> {
        if !grid.belongs_to(self) {
            return Err(GeometryError::Reference);
        }
        if self.is_color() || pixel.iter().zip(grid.dimensions).any(|(p, n)| *p >= n) {
            return Err(GeometryError::Dimensions);
        }
        if cancelled() {
            return Err(GeometryError::Cancelled);
        }
        let g = &grid.geometry;
        let (u, v) = g.directions();
        let origin = self.world_to_voxel(g.position);
        let du = self.world_vector_to_voxel(mul(u, g.spacing[1]));
        let dv = self.world_vector_to_voxel(mul(v, g.spacing[0]));
        let center = add(
            add(origin, mul(dv, f64::from(pixel[1]))),
            mul(du, f64::from(pixel[0])),
        );
        let Some(slab) = grid.slab else {
            return Ok(self.sample_voxel(center));
        };
        let normal = self.world_vector_to_voxel(cross(u, v));
        let (mut sum, mut count, mut extreme) = (0f64, 0u32, None::<f32>);
        for i in 0..=slab.intervals {
            if i % 16 == 0 && cancelled() {
                return Err(GeometryError::Cancelled);
            }
            let distance = slab.thickness_mm * (f64::from(i) / f64::from(slab.intervals) - 0.5);
            if let Some(value) = self.sample_voxel(add(center, mul(normal, distance))) {
                sum += f64::from(value);
                count += 1;
                extreme = Some(extreme.map_or(value, |old| match slab.projection {
                    crate::slab::SlabProjection::Maximum => old.max(value),
                    crate::slab::SlabProjection::Minimum => old.min(value),
                    crate::slab::SlabProjection::Mean => old,
                }));
            }
        }
        Ok(match slab.projection {
            crate::slab::SlabProjection::Mean => {
                (count > 0).then(|| (sum / f64::from(count)) as f32)
            }
            _ => extreme,
        })
    }
    /// Worker-only staging for these exact source/grid owners. Admission also
    /// covers the temporary Vec-to-Arc conversion; no pixel data is duplicated
    /// in the retained source volume.
    pub fn scalar_reformat_buffer(
        &self,
        grid: &ReformatGrid,
    ) -> Result<ScalarReformatBuffer, GeometryError> {
        self.scalar_reformat_buffer_with_budget(grid, &MemoryBudget::volumes())
    }
    fn scalar_reformat_buffer_with_budget(
        &self,
        grid: &ReformatGrid,
        budget: &Arc<MemoryBudget>,
    ) -> Result<ScalarReformatBuffer, GeometryError> {
        if !grid.belongs_to(self) {
            return Err(GeometryError::Reference);
        }
        if self.is_color() {
            return Err(GeometryError::Pixels);
        }
        let [width, height] = grid.dimensions;
        let count = width as usize * height as usize;
        let memory = Arc::new(
            budget
                .reserve(
                    (count * 8
                        + std::mem::size_of::<Frame>()
                        + std::mem::size_of::<ScalarReformatBuffer>()
                        + usize::from(self.frames[0].scalar.is_some())
                            * std::mem::size_of::<crate::scalar::ScalarData>()
                        + grid
                            .geometry
                            .frame_of_reference
                            .as_ref()
                            .map_or(0, String::len)
                        + 128) as u64,
                )
                .ok_or(GeometryError::Memory)?,
        );
        let mut pixels = Vec::new();
        pixels
            .try_reserve_exact(count)
            .map_err(|_| GeometryError::Memory)?;
        pixels.resize(count, f32::NAN);
        Ok(ScalarReformatBuffer {
            grid: grid.clone(),
            template: self.frames[0].clone(),
            pixels,
            memory,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::study::demo_study;
    #[test]
    fn sampling_grids_require_exact_volume_identity_and_frames_retain_admission() {
        let frames = demo_study().series.remove(0).frames;
        let volume = Volume::new(frames.clone()).unwrap();
        let foreign = Volume::new(frames).unwrap();
        let grid = volume
            .reformat_grid(Plane::Axial, volume.center(), 64)
            .unwrap();
        assert!(grid.belongs_to(&volume));
        assert!(!grid.belongs_to(&foreign));
        assert!(matches!(
            foreign.scalar_reformat_buffer(&grid),
            Err(GeometryError::Reference)
        ));
        let budget = MemoryBudget::new(0);
        assert!(matches!(
            volume.scalar_reformat_buffer_with_budget(&grid, &budget),
            Err(GeometryError::Memory)
        ));
        assert_eq!(budget.used(), 0);
        budget.set_limit(1024 * 1024);
        let mut buffer = volume
            .scalar_reformat_buffer_with_budget(&grid, &budget)
            .unwrap();
        let used = budget.used();
        assert!(used > u64::from(grid.dimensions[0]) * u64::from(grid.dimensions[1]) * 4);
        buffer.pixels_mut().fill(123.);
        let frame = buffer.finish();
        let duplicate = frame.clone();
        assert_eq!(frame.geometry.as_ref(), Some(&grid.geometry));
        assert!(frame.pixels.iter().all(|v| *v == 123.));
        drop(frame);
        assert_eq!(budget.used(), used);
        let copy = (*duplicate).clone();
        drop(duplicate);
        assert_eq!(budget.used(), used);
        drop(copy);
        assert_eq!(budget.used(), 0);
        drop(
            volume
                .scalar_reformat_buffer_with_budget(&grid, &budget)
                .unwrap(),
        );
        assert_eq!(budget.used(), 0);
    }
}
