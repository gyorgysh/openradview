// SPDX-License-Identifier: GPL-3.0-or-later
//! Face-connected selection on the exact source grid, without anatomical inference.
use super::Labels;
use crate::resources::{MemoryBudget, Reservation};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LabelScope {
    /// Four neighbors; only this source slice can change.
    Slice(usize),
    /// Six neighbors in the regular source grid, including tilted/reversed grids.
    Volume,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RegionOperation {
    /// Add connected valid background/selected-ID cells in the inclusive interval.
    Grow { seed: [usize; 3], range: [f32; 2] },
    /// Remove valid selected-ID cells outside the seeded component, within scope.
    KeepComponent { seed: [usize; 3] },
    /// Remove valid selected-ID components strictly smaller than this cell count.
    RemoveSmall { minimum_voxels: usize },
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SelectionRequest {
    pub scope: LabelScope,
    pub label: u8,
    pub operation: RegionOperation,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SelectionSummary {
    /// Grow: connected cells; keep: retained component; small islands: removed cells.
    pub region_voxels: usize,
    /// Cells whose stored ID will change. Existing target cells are not additions.
    pub changed_voxels: usize,
    /// Grow/keep: one; small islands: number of removed components.
    pub components: usize,
}
/// An immutable, RAM-only proposal bound to the exact original label map.
/// Missing source samples are barriers and are preserved by cleanup operations.
pub struct LabelSelection {
    base: Arc<Labels>,
    request: SelectionRequest,
    offset: usize,
    changes: Vec<u8>,
    summary: SelectionSummary,
    _reservation: Reservation,
}
impl LabelSelection {
    pub fn base(&self) -> &Arc<Labels> {
        &self.base
    }
    pub fn request(&self) -> SelectionRequest {
        self.request
    }
    pub fn summary(&self) -> SelectionSummary {
        self.summary
    }
    /// Worker-only. Failure/cancellation leaves the original map untouched.
    /// The caller must still match `base()` to its current revision before publishing.
    pub fn apply(&self, cancelled: impl Fn() -> bool) -> Result<Labels, &'static str> {
        let mut result = self.base.try_clone(&cancelled)?;
        let value = if matches!(self.request.operation, RegionOperation::Grow { .. }) {
            self.request.label
        } else {
            0
        };
        for (index, changed) in self.changes.iter().enumerate() {
            if index.is_multiple_of(1024) && cancelled() {
                return Err("Cancelled");
            }
            if *changed == 1 {
                result.values[self.offset + index] = value;
            }
        }
        if cancelled() {
            return Err("Cancelled");
        }
        Ok(result)
    }
}

struct Search<'a> {
    labels: &'a Labels,
    request: SelectionRequest,
    width: usize,
    plane: usize,
    count: usize,
    offset: usize,
}
impl Search<'_> {
    fn eligible(&self, local: usize) -> bool {
        let index = local + self.offset;
        let frame = &self.labels.source.frames()[index / self.plane];
        let pixel = index % self.plane;
        if !frame.has_sample(pixel) {
            return false;
        }
        let label = self.labels.values[index];
        match self.request.operation {
            RegionOperation::Grow { range, .. } => {
                (label == 0 || label == self.request.label)
                    && frame.pixels[pixel] >= range[0]
                    && frame.pixels[pixel] <= range[1]
            }
            _ => label == self.request.label,
        }
    }
    fn seed(&self, seed: [usize; 3]) -> Result<usize, &'static str> {
        let [w, h, d] = self.labels.source.dimensions();
        if seed[0] >= w
            || seed[1] >= h
            || seed[2] >= d
            || matches!(self.request.scope, LabelScope::Slice(z) if z != seed[2])
        {
            return Err("The seed is outside the selected source scope");
        }
        let local = seed[2] * self.plane + seed[1] * w + seed[0] - self.offset;
        if !self.eligible(local) {
            return Err(
                "The seed must be a valid source sample in the interval and an allowed label",
            );
        }
        Ok(local)
    }
    fn flood(
        &self,
        seed: usize,
        states: &mut [u8],
        queue: &mut Vec<u32>,
        cancel: &dyn Fn() -> bool,
    ) -> Result<(), &'static str> {
        queue.clear();
        states[seed] = 2;
        queue.push(seed as u32);
        let mut head = 0;
        while head < queue.len() {
            if head.is_multiple_of(1024) && cancel() {
                return Err("Cancelled");
            }
            let index = queue[head] as usize;
            head += 1;
            let x = index % self.width;
            let y = index % self.plane / self.width;
            let height = self.plane / self.width;
            // Each in-bounds cell is enqueued once; the queue is pre-admitted for count u32s.
            let neighbors = [
                (x > 0).then(|| index - 1),
                (x + 1 < self.width).then_some(index + 1),
                (y > 0).then(|| index - self.width),
                (y + 1 < height).then_some(index + self.width),
                (index >= self.plane).then(|| index - self.plane),
                (index + self.plane < self.count).then_some(index + self.plane),
            ];
            for neighbor in neighbors.into_iter().flatten() {
                if states[neighbor] == 0 {
                    states[neighbor] = 1;
                    if self.eligible(neighbor) {
                        states[neighbor] = 2;
                        queue.push(neighbor as u32);
                    }
                }
            }
        }
        Ok(())
    }
}
impl Labels {
    pub fn select_region(
        self: &Arc<Self>,
        request: SelectionRequest,
        cancelled: impl Fn() -> bool,
    ) -> Result<LabelSelection, &'static str> {
        self.select_with_budget(request, &cancelled, &MemoryBudget::volumes())
    }
    fn select_with_budget(
        self: &Arc<Self>,
        request: SelectionRequest,
        cancel: &dyn Fn() -> bool,
        budget: &Arc<MemoryBudget>,
    ) -> Result<LabelSelection, &'static str> {
        if cancel() {
            return Err("Cancelled");
        }
        let [width, height, depth] = self.source.dimensions();
        Self::check_dimensions([width, height, depth])?;
        if request.label == 0 {
            return Err("Choose a nonzero label ID");
        }
        match request.operation {
            RegionOperation::Grow { range, .. }
                if self.source.is_color()
                    || range.iter().any(|v| !v.is_finite())
                    || range[0] > range[1] =>
            {
                return Err("Region growing needs scalar samples and a finite ordered interval");
            }
            RegionOperation::RemoveSmall { minimum_voxels }
                if minimum_voxels == 0 || minimum_voxels > super::MAX_VOXELS =>
            {
                return Err("Choose a component cutoff from 1 to 32 million voxels");
            }
            _ => (),
        }
        let plane = width * height;
        let (offset, count) = match request.scope {
            LabelScope::Slice(z) if z < depth => (z * plane, plane),
            LabelScope::Volume => (0, plane * depth),
            _ => return Err("The selected source slice is outside the volume"),
        };
        let search = Search {
            labels: self,
            request,
            width,
            plane,
            count,
            offset,
        };
        let seed = match request.operation {
            RegionOperation::Grow { seed, .. } | RegionOperation::KeepComponent { seed } => {
                Some(search.seed(seed)?)
            }
            _ => None,
        };
        // Worst case: every scope cell is visited and the whole component is queued.
        // Keep the output mask charged after releasing the transient queue reservation.
        let reservation = budget
            .reserve(count as u64)
            .ok_or("Not enough memory for a region mask")?;
        let _queue_reservation = budget
            .reserve(count as u64 * 4)
            .ok_or("Not enough memory for connected-region traversal")?;
        let mut states = Vec::new();
        states
            .try_reserve_exact(count)
            .map_err(|_| "Not enough memory for a region mask")?;
        states.resize(count, 0);
        let mut queue = Vec::new();
        queue
            .try_reserve_exact(count)
            .map_err(|_| "Not enough memory for connected-region traversal")?;
        let mut summary = SelectionSummary::default();
        match request.operation {
            RegionOperation::Grow { .. } | RegionOperation::KeepComponent { .. } => {
                if let Some(seed) = seed {
                    search.flood(seed, &mut states, &mut queue, cancel)?;
                }
                summary.region_voxels = queue.len();
                summary.components = 1;
            }
            RegionOperation::RemoveSmall { minimum_voxels } => {
                for index in 0..count {
                    if index.is_multiple_of(1024) && cancel() {
                        return Err("Cancelled");
                    }
                    if states[index] != 0 {
                        continue;
                    }
                    if !search.eligible(index) {
                        states[index] = 1;
                        continue;
                    }
                    search.flood(index, &mut states, &mut queue, cancel)?;
                    if queue.len() < minimum_voxels {
                        summary.region_voxels += queue.len();
                        summary.components += 1;
                        for (i, &cell) in queue.iter().enumerate() {
                            if i.is_multiple_of(1024) && cancel() {
                                return Err("Cancelled");
                            }
                            states[cell as usize] = 3;
                        }
                    }
                }
            }
        }
        for (index, state) in states.iter_mut().enumerate() {
            if index.is_multiple_of(1024) && cancel() {
                return Err("Cancelled");
            }
            let change = match request.operation {
                RegionOperation::Grow { .. } => *state == 2 && self.values[offset + index] == 0,
                RegionOperation::KeepComponent { .. } => *state != 2 && search.eligible(index),
                RegionOperation::RemoveSmall { .. } => *state == 3,
            };
            *state = u8::from(change);
            summary.changed_voxels += usize::from(change);
        }
        if cancel() {
            return Err("Cancelled");
        }
        Ok(LabelSelection {
            base: self.clone(),
            request,
            offset,
            changes: states,
            summary,
            _reservation: reservation,
        })
    }
}

#[cfg(test)]
mod tests;
