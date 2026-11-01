// SPDX-License-Identifier: GPL-3.0-or-later
//! Source voxel counts, independent of display resolution and overlay visibility.
use super::Labels;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LabelCount {
    pub stored_voxels: u64,
    pub valid_voxels: u64,
}
impl LabelCount {
    pub fn missing_voxels(&self) -> u64 {
        self.stored_voxels.saturating_sub(self.valid_voxels)
    }
}
#[derive(Clone, Debug)]
pub struct LabelStatistics {
    /// Includes ID zero (background); no anatomical meaning is inferred.
    pub by_id: [LabelCount; 256],
    pub voxel_volume_mm3: f64,
}
impl Labels {
    /// Worker-only. Stored labels on missing source samples remain in the map
    /// and in stored counts, but never become valid image samples.
    pub fn statistics(
        &self,
        cancelled: impl Fn() -> bool,
    ) -> Result<LabelStatistics, &'static str> {
        let voxel_volume_mm3 = self.source.voxel_volume_mm3();
        if !voxel_volume_mm3.is_finite() || voxel_volume_mm3 <= 0. {
            return Err("The source voxel volume is invalid");
        }
        let mut by_id = [LabelCount::default(); 256];
        let [width, height, _] = self.source.dimensions();
        let plane_size = width * height;
        for (index, &id) in self.values.iter().enumerate() {
            if index.is_multiple_of(1024) && cancelled() {
                return Err("Cancelled");
            }
            let count = &mut by_id[id as usize];
            count.stored_voxels += 1;
            if self.source.frames()[index / plane_size].has_sample(index % plane_size) {
                count.valid_voxels += 1;
            }
        }
        if cancelled() {
            return Err("Cancelled");
        }
        Ok(LabelStatistics {
            by_id,
            voxel_volume_mm3,
        })
    }
}
