// SPDX-License-Identifier: GPL-3.0-or-later
//! Independent categorical bits on completed thin planes; overlap is never flattened.
use super::{MAX_SEGMENTS, Segments, bit};
use crate::{
    plane_sampling::ThinPlane,
    resources::{MemoryBudget, Reservation},
    study::Frame,
};
use std::sync::Arc;
#[cfg(test)]
#[path = "sampling_tests.rs"]
mod tests;

pub struct SegmentPlane {
    dimensions: [usize; 2],
    numbers: [u16; MAX_SEGMENTS],
    number_count: usize,
    positive: Vec<u64>,
    supplied: Vec<u64>,
    eligible: Vec<u8>,
    _reservation: Reservation,
}
impl SegmentPlane {
    pub fn dimensions(&self) -> [usize; 2] {
        self.dimensions
    }
    /// Bit positions in both arrays follow these segment numbers, not their numeric values.
    pub fn numbers(&self) -> &[u16] {
        &self.numbers[..self.number_count]
    }
    /// All nearest stored positives, including on missing source/output samples.
    pub fn positive_bits(&self) -> &[u64] {
        &self.positive
    }
    /// A zero bit means this segment's nearest source plane was omitted. Outside is all zero.
    pub fn supplied_bits(&self) -> &[u64] {
        &self.supplied
    }
    /// One only when the nearest source voxel and completed output sample both exist.
    pub fn eligibility(&self) -> &[u8] {
        &self.eligible
    }
}
impl Segments {
    /// Worker-only. Bind the completed image to this exact source and view generation;
    /// matching declared geometry alone does not establish source identity.
    pub fn sample_plane(
        &self,
        image: &Frame,
        cancelled: impl Fn() -> bool,
    ) -> Result<SegmentPlane, &'static str> {
        self.sample_with_budget(image, &cancelled, &MemoryBudget::volumes())
    }
    fn sample_with_budget(
        &self,
        image: &Frame,
        cancel: &dyn Fn() -> bool,
        budget: &Arc<MemoryBudget>,
    ) -> Result<SegmentPlane, &'static str> {
        if cancel() {
            return Err("Cancelled");
        }
        let plane = ThinPlane::new(&self.source, image)?;
        let count = plane.count();
        let reservation = budget
            .reserve(count as u64 * 17)
            .ok_or("Not enough memory for a supplied-mask plane")?;
        let mut positive = Vec::new();
        let mut supplied = Vec::new();
        let mut eligible = Vec::new();
        positive
            .try_reserve_exact(count)
            .map_err(|_| "Not enough memory for supplied-mask plane bits")?;
        supplied
            .try_reserve_exact(count)
            .map_err(|_| "Not enough memory for supplied-mask plane coverage")?;
        eligible
            .try_reserve_exact(count)
            .map_err(|_| "Not enough memory for supplied-mask plane eligibility")?;
        let mut numbers = [0; MAX_SEGMENTS];
        for (number, mask) in numbers.iter_mut().zip(&self.masks) {
            *number = mask.info.number;
        }
        let mut previous_z = usize::MAX;
        let mut coverage = 0u64;
        let [width, height, _] = self.source.dimensions();
        for index in 0..count {
            if index.is_multiple_of(1024) && cancel() {
                return Err("Cancelled");
            }
            let Some(cell) = plane.cell(index)? else {
                positive.push(0);
                supplied.push(0);
                eligible.push(0);
                continue;
            };
            if cell.z != previous_z {
                coverage = 0;
                for (i, mask) in self.masks.iter().enumerate() {
                    if bit(&mask.covered, cell.z) {
                        coverage |= 1u64 << i;
                    }
                }
                previous_z = cell.z;
            }
            let mut bits = 0u64;
            for (i, mask) in self.masks.iter().enumerate() {
                if coverage & (1u64 << i) != 0
                    && mask
                        .samples
                        .positive(cell.index, width * height, self.encoding)
                {
                    bits |= 1u64 << i;
                }
            }
            positive.push(bits);
            supplied.push(coverage);
            eligible.push(u8::from(cell.eligible));
        }
        if cancel() {
            return Err("Cancelled");
        }
        Ok(SegmentPlane {
            dimensions: plane.dimensions(),
            numbers,
            number_count: self.masks.len(),
            positive,
            supplied,
            eligible,
            _reservation: reservation,
        })
    }
}
