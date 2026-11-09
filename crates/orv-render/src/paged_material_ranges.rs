// SPDX-License-Identifier: GPL-3.0-or-later
//! Certified optical zeros bound to a paged source, without a dense GPU upload.
use crate::{brick_source::GpuBrickSource, material_ranges::GpuRangeFlags};
use orv_core::{
    resources::{MemoryBudget, Reservation},
    volume_material::VolumeMaterial,
    volume_ranges::VolumeRanges,
};
use std::sync::Arc;
pub struct GpuPagedMaterialRanges {
    source: Arc<GpuBrickSource>,
    ranges: Arc<VolumeRanges>,
    material: Arc<VolumeMaterial>,
    flags: GpuRangeFlags,
    required: Vec<u8>,
    _metadata: Reservation,
}
impl GpuPagedMaterialRanges {
    /// Worker-only. Stored-page numeric safety is conservative, including the
    /// complete gradient halo, even where optical coefficients are all zero.
    pub fn new(
        source: Arc<GpuBrickSource>,
        ranges: Arc<VolumeRanges>,
        material: Arc<VolumeMaterial>,
        cancelled: impl Fn() -> bool,
    ) -> Result<Arc<Self>, &'static str> {
        if cancelled() {
            return Err("Cancelled");
        }
        if !Arc::ptr_eq(source.source(), ranges.source()) {
            return Err("Paged ranges require the exact CPU source");
        }
        let count = source.layout().len();
        let metadata = MemoryBudget::volumes()
            .reserve((std::mem::size_of::<Self>() + 16 + count) as u64)
            .ok_or("Not enough memory for paged range ownership")?;
        let _scratch = MemoryBudget::volumes()
            .reserve(count as u64)
            .ok_or("Not enough memory for paged range safety")?;
        let mut safe = Vec::new();
        safe.try_reserve_exact(count)
            .map_err(|_| "Not enough memory for paged range safety")?;
        let dimensions = source.source().dimensions();
        let side = ranges.side();
        let counts = ranges.counts();
        for i in 0..count {
            if cancelled() {
                return Err("Cancelled");
            }
            let b = source
                .layout()
                .brick(i)
                .ok_or("Invalid source range page")?;
            let lo = std::array::from_fn(|a| b.origin[a].min(dimensions[a] - 2) / side);
            let hi = std::array::from_fn(|a| {
                (b.origin[a] + b.size[a] - 1).min(dimensions[a] - 2) / side
            });
            let valid = all_tiles(counts, lo, hi, |index| {
                let s = ranges.summaries()[index];
                s.minimum.abs() <= 1e20 && s.maximum.abs() <= 1e20
            });
            safe.push(u8::from(valid));
        }
        let cell = source.layout().cell_side()[0];
        let pages = dimensions.map(|d| (d - 1).div_ceil(cell));
        let flags = GpuRangeFlags::new(
            source.device(),
            source.queue(),
            ranges.clone(),
            material.clone(),
            |i| {
                let Some(tile) = ranges.tile(i) else {
                    return false;
                };
                let lo = tile.origin.map(|v| v / cell);
                let hi = std::array::from_fn(|a| (tile.origin[a] + tile.cells[a] - 1) / cell);
                all_tiles(pages, lo, hi, |index| safe[index] != 0)
            },
            &cancelled,
        )?;
        let mut required = Vec::new();
        required
            .try_reserve_exact(count)
            .map_err(|_| "Not enough memory for paged range requirements")?;
        for i in 0..count {
            if cancelled() {
                return Err("Cancelled");
            }
            let bounds = source
                .layout()
                .cells(i)
                .ok_or("Invalid source range cells")?;
            let lo = bounds[0].map(|v| v / side);
            let hi = std::array::from_fn(|a| (bounds[1][a] - 1) / side);
            required.push(u8::from(!all_tiles(counts, lo, hi, |index| {
                flags.zero[index] != 0
            })));
        }
        if cancelled() {
            return Err("Cancelled");
        }
        Ok(Arc::new(Self {
            source,
            ranges,
            material,
            flags,
            required,
            _metadata: metadata,
        }))
    }
    pub fn source(&self) -> &Arc<GpuBrickSource> {
        &self.source
    }
    pub fn ranges(&self) -> &Arc<VolumeRanges> {
        &self.ranges
    }
    pub fn material(&self) -> &Arc<VolumeMaterial> {
        &self.material
    }
    pub fn transparent_tiles(&self) -> usize {
        self.flags.transparent_count
    }
    pub fn required_pages(&self) -> usize {
        self.required.iter().filter(|v| **v != 0).count()
    }
    pub fn page_required(&self, index: usize) -> Option<bool> {
        self.required.get(index).map(|v| *v != 0)
    }
    pub(crate) fn required_indices(&self) -> impl Iterator<Item = usize> + '_ {
        self.required
            .iter()
            .enumerate()
            .filter_map(|(i, v)| (*v != 0).then_some(i))
    }
    pub(crate) fn layout(&self) -> &wgpu::BindGroupLayout {
        &self.flags.layout
    }
    pub(crate) fn bindings(&self) -> &wgpu::BindGroup {
        &self.flags.bindings
    }
}
fn all_tiles(
    counts: [usize; 3],
    lo: [usize; 3],
    hi: [usize; 3],
    test: impl Fn(usize) -> bool,
) -> bool {
    for z in lo[2]..=hi[2] {
        for y in lo[1]..=hi[1] {
            for x in lo[0]..=hi[0] {
                if !test(x + counts[0] * (y + counts[1] * z)) {
                    return false;
                }
            }
        }
    }
    true
}
