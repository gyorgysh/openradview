// SPDX-License-Identifier: GPL-3.0-or-later
//! Categorical labels on a thin plane; scalar interpolation never blends IDs.
use super::Labels;
use crate::{
    plane_sampling::ThinPlane,
    resources::{MemoryBudget, Reservation},
    study::Frame,
};

pub const MAX_PLANE_PIXELS: usize = crate::plane_sampling::MAX_PIXELS;

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
        let plane = ThinPlane::new(&self.source, image)?;
        let dimensions = plane.dimensions();
        let count = plane.count();
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
        for index in 0..count {
            if index.is_multiple_of(1024) && cancelled() {
                return Err("Cancelled");
            }
            let Some(cell) = plane.cell(index)? else {
                ids.push(0);
                eligible.push(0);
                continue;
            };
            ids.push(self.values[cell.index]);
            eligible.push(u8::from(cell.eligible));
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
