// SPDX-License-Identifier: GPL-3.0-or-later
//! Source-bound scalar vertex summaries for conservative rendering acceleration.
//!
//! These describe stored vertices, including the positive interpolation halo.
//! They do not by themselves bound rounded GPU interpolation or authorize a skip.
use crate::{
    resources::{MemoryBudget, Reservation},
    volume::Volume,
};
use std::sync::Arc;

/// At most 4 MiB of records, independent of the admitted source dimensions.
pub const MAX_RANGE_TILES: usize = 262_144;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RangeError {
    Color,
    Layout,
    Memory,
    Cancelled,
}
impl std::fmt::Display for RangeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Color => "Scalar range summaries do not accept color sources",
            Self::Layout => "Scalar range tile layout exceeds its supported bounds",
            Self::Memory => "Not enough memory for scalar range summaries",
            Self::Cancelled => "Cancelled",
        })
    }
}
impl std::error::Error for RangeError {}

/// Original finite scalar extrema and coverage of one tile's stored vertices.
/// Invalid vertices are included in the extrema: zero-weight shader arithmetic
/// can still read their original values at an otherwise valid grid node.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScalarSummary {
    pub minimum: f32,
    pub maximum: f32,
    pub valid_vertices: u32,
    pub vertices: u32,
}
impl ScalarSummary {
    pub fn all_valid(self) -> bool {
        self.valid_vertices == self.vertices
    }
    pub fn all_missing(self) -> bool {
        self.valid_vertices == 0
    }
}

/// Owns lower-corner interpolation cells and stores one positive vertex halo.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RangeTile {
    pub origin: [usize; 3],
    pub cells: [usize; 3],
    pub summary: ScalarSummary,
}

pub struct VolumeRanges {
    source: Arc<Volume>,
    side: usize,
    counts: [usize; 3],
    summaries: Vec<ScalarSummary>,
    _reservation: Reservation,
}
impl VolumeRanges {
    /// Worker-only, bounded/cancellable scan, with no second scalar volume.
    /// Side is a power of two from 8 through 128 interpolation cells. The grid
    /// retains the immutable source; another source needs another summary grid.
    pub fn build(
        source: Arc<Volume>,
        side: usize,
        cancelled: impl Fn() -> bool,
    ) -> Result<Arc<Self>, RangeError> {
        Self::build_with_budget(source, side, &MemoryBudget::volumes(), &cancelled)
    }
    fn build_with_budget(
        source: Arc<Volume>,
        side: usize,
        budget: &Arc<MemoryBudget>,
        cancelled: &impl Fn() -> bool,
    ) -> Result<Arc<Self>, RangeError> {
        if cancelled() {
            return Err(RangeError::Cancelled);
        }
        if source.is_color() {
            return Err(RangeError::Color);
        }
        let dimensions = source.dimensions();
        let counts = layout(dimensions, side)?;
        let count = counts.iter().product::<usize>();
        let bytes = count * std::mem::size_of::<ScalarSummary>()
            + std::mem::size_of::<Self>()
            + 2 * std::mem::size_of::<usize>();
        let reservation = budget.reserve(bytes as u64).ok_or(RangeError::Memory)?;
        let mut summaries = Vec::new();
        summaries
            .try_reserve_exact(count)
            .map_err(|_| RangeError::Memory)?;
        for index in 0..count {
            let (origin, cells) = tile_bounds(index, counts, dimensions, side);
            let mut summary = ScalarSummary {
                minimum: f32::INFINITY,
                maximum: f32::NEG_INFINITY,
                valid_vertices: 0,
                vertices: cells.iter().map(|c| (c + 1) as u32).product(),
            };
            for z in origin[2]..=origin[2] + cells[2] {
                let frame = &source.frames()[z];
                for y in origin[1]..=origin[1] + cells[1] {
                    if cancelled() {
                        return Err(RangeError::Cancelled);
                    }
                    let start = y * dimensions[0] + origin[0];
                    for i in start..=start + cells[0] {
                        let value = frame.pixels[i];
                        summary.minimum = summary.minimum.min(value);
                        summary.maximum = summary.maximum.max(value);
                        summary.valid_vertices += u32::from(frame.has_sample(i));
                    }
                }
            }
            summaries.push(summary);
        }
        if cancelled() {
            return Err(RangeError::Cancelled);
        }
        Ok(Arc::new(Self {
            source,
            side,
            counts,
            summaries,
            _reservation: reservation,
        }))
    }
    pub fn source(&self) -> &Arc<Volume> {
        &self.source
    }
    pub fn side(&self) -> usize {
        self.side
    }
    pub fn counts(&self) -> [usize; 3] {
        self.counts
    }
    pub fn summaries(&self) -> &[ScalarSummary] {
        &self.summaries
    }
    pub fn tile(&self, index: usize) -> Option<RangeTile> {
        let summary = *self.summaries.get(index)?;
        let (origin, cells) = tile_bounds(index, self.counts, self.source.dimensions(), self.side);
        Some(RangeTile {
            origin,
            cells,
            summary,
        })
    }
    /// Scalar-centre domain only, with the final vertex owned by the last cell.
    /// Missing source samples still have a tile; they are not nonresident data.
    pub fn owner(&self, voxel: [f64; 3]) -> Option<usize> {
        let dimensions = self.source.dimensions();
        if voxel
            .iter()
            .enumerate()
            .any(|(axis, p)| !p.is_finite() || *p < 0. || *p > (dimensions[axis] - 1) as f64)
        {
            return None;
        }
        let cell: [usize; 3] = std::array::from_fn(|axis| {
            (voxel[axis].floor() as usize).min(dimensions[axis] - 2) / self.side
        });
        Some(cell[0] + self.counts[0] * (cell[1] + self.counts[1] * cell[2]))
    }
}

fn layout(dimensions: [usize; 3], side: usize) -> Result<[usize; 3], RangeError> {
    if !(8..=128).contains(&side) || !side.is_power_of_two() || dimensions.iter().any(|d| *d < 2) {
        return Err(RangeError::Layout);
    }
    let counts = dimensions.map(|d| (d - 1).div_ceil(side));
    if counts
        .iter()
        .try_fold(1usize, |n, d| n.checked_mul(*d))
        .is_none_or(|n| n > MAX_RANGE_TILES)
    {
        return Err(RangeError::Layout);
    }
    Ok(counts)
}

fn tile_bounds(
    index: usize,
    counts: [usize; 3],
    dimensions: [usize; 3],
    side: usize,
) -> ([usize; 3], [usize; 3]) {
    let origin = [
        index % counts[0] * side,
        index / counts[0] % counts[1] * side,
        index / (counts[0] * counts[1]) * side,
    ];
    let cells = std::array::from_fn(|axis| side.min(dimensions[axis] - 1 - origin[axis]));
    (origin, cells)
}

#[cfg(test)]
mod tests;
