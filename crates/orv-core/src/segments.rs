// SPDX-License-Identifier: GPL-3.0-or-later
//! Read-only masks on an explicitly associated source grid. No semantic names or inference.
use crate::{
    resources::{MemoryBudget, Reservation},
    volume::Volume,
};
use std::sync::Arc;
mod sampling;
pub use sampling::SegmentPlane;

pub const MAX_SEGMENTS: usize = 64;
pub const MAX_FRAMES: usize = 4096;
pub const MAX_MASK_BYTES: usize = 128 * 1024 * 1024;
pub const MAX_INPUT_BYTES: usize = 128 * 1024 * 1024;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SegmentInfo {
    pub number: u16,
    pub positive_cells: usize,
    pub valid_source_cells: usize,
    pub supplied_planes: usize,
}
struct Mask {
    info: SegmentInfo,
    bits: Vec<u8>,
    covered: Vec<u8>,
}
pub struct Segments {
    source: Arc<Volume>,
    masks: Vec<Mask>,
    overlap_cells: usize,
    _reservation: Reservation,
}
fn bit(bytes: &[u8], index: usize) -> bool {
    bytes[index / 8] & (1 << (index % 8)) != 0
}
fn set(bytes: &mut [u8], index: usize) {
    bytes[index / 8] |= 1 << (index % 8);
}
fn zeros(count: usize) -> Result<Vec<u8>, &'static str> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(count)
        .map_err(|_| "Not enough memory for supplied masks")?;
    values.resize(count, 0);
    Ok(values)
}
fn shape(
    dimensions: [usize; 3],
    segments: usize,
) -> Result<(usize, usize, usize, u64), &'static str> {
    if !(1..=MAX_SEGMENTS).contains(&segments) {
        return Err("Supplied masks need one to 64 segment IDs");
    }
    let cells = crate::labels::Labels::check_dimensions(dimensions)?;
    if dimensions[2] > MAX_FRAMES {
        return Err("Supplied masks support up to 4096 source planes");
    }
    let bytes = cells.div_ceil(8);
    let coverage = dimensions[2].div_ceil(8);
    let mask_bytes = bytes
        .checked_mul(segments)
        .filter(|n| *n <= MAX_MASK_BYTES)
        .ok_or("Supplied masks exceed the 128 MiB packed-mask limit")?;
    let retained = mask_bytes + coverage * segments + std::mem::size_of::<Mask>() * segments;
    Ok((cells, bytes, coverage, retained as u64))
}
impl Segments {
    /// Header-only admission before preparing the source volume or reading mask pixels.
    pub fn check_layout(dimensions: [usize; 3], segments: usize) -> Result<(), &'static str> {
        shape(dimensions, segments).map(|_| ())
    }
    /// Worker-only: decode native one-bit frames after source references and geometry have been
    /// verified. Each (segment number, affine-sorted source slice) names one frame.
    /// This validates encoding/grid bounds, not DICOM source association. Sparse
    /// omitted planes remain distinct from supplied planes whose bits are all zero.
    pub fn from_binary(
        source: Arc<Volume>,
        numbers: &[u16],
        frames: &[(u16, usize)],
        payload: &[u8],
        declared_overlap: Option<bool>,
        cancelled: impl Fn() -> bool,
    ) -> Result<Self, &'static str> {
        Self::with_budget(
            source,
            numbers,
            frames,
            payload,
            declared_overlap,
            &cancelled,
            &MemoryBudget::volumes(),
        )
    }
    fn with_budget(
        source: Arc<Volume>,
        numbers: &[u16],
        frames: &[(u16, usize)],
        payload: &[u8],
        declared_overlap: Option<bool>,
        cancelled: &dyn Fn() -> bool,
        budget: &Arc<MemoryBudget>,
    ) -> Result<Self, &'static str> {
        if cancelled() {
            return Err("Cancelled");
        }
        let dimensions = source.dimensions();
        let (_, bytes, coverage, retained) = shape(dimensions, numbers.len())?;
        if numbers
            .iter()
            .enumerate()
            .any(|(i, n)| *n == 0 || numbers[..i].contains(n))
        {
            return Err("Segment IDs must be unique nonzero numbers");
        }
        if frames.is_empty() || frames.len() > MAX_FRAMES {
            return Err("Supplied masks need one to 4096 frames");
        }
        let plane = dimensions[0] * dimensions[1];
        let encoded_bits = plane
            .checked_mul(frames.len())
            .ok_or("Mask payload size overflow")?;
        let encoded_bytes = encoded_bits.div_ceil(8).next_multiple_of(2);
        if encoded_bytes > MAX_INPUT_BYTES || payload.len() != encoded_bytes {
            return Err("Mask payload length does not match the supported native bit frames");
        }
        if frames
            .iter()
            .any(|(number, z)| !numbers.contains(number) || *z >= dimensions[2])
        {
            return Err("Mask frame references an unknown segment or source plane");
        }
        let reservation = budget
            .reserve(retained)
            .ok_or("Not enough memory for supplied masks")?;
        let _temporary = budget
            .reserve(bytes as u64 * 2)
            .ok_or("Not enough memory to check mask overlap")?;
        let mut union = zeros(bytes)?;
        let mut overlaps = zeros(bytes)?;
        let mut masks = Vec::new();
        masks
            .try_reserve_exact(numbers.len())
            .map_err(|_| "Not enough memory for segment metadata")?;
        for &number in numbers {
            if cancelled() {
                return Err("Cancelled");
            }
            masks.push(Mask {
                info: SegmentInfo {
                    number,
                    positive_cells: 0,
                    valid_source_cells: 0,
                    supplied_planes: 0,
                },
                bits: zeros(bytes)?,
                covered: zeros(coverage)?,
            });
        }
        let mut overlap_cells = 0;
        for (frame_index, &(number, z)) in frames.iter().enumerate() {
            if cancelled() {
                return Err("Cancelled");
            }
            let mask = masks
                .iter_mut()
                .find(|m| m.info.number == number)
                .ok_or("Unknown segment ID")?;
            if bit(&mask.covered, z) {
                return Err("Repeated segment/source-plane frames are unsupported");
            }
            set(&mut mask.covered, z);
            mask.info.supplied_planes += 1;
            let image = &source.frames()[z];
            for pixel in 0..plane {
                if pixel.is_multiple_of(1024) && cancelled() {
                    return Err("Cancelled");
                }
                if !bit(payload, frame_index * plane + pixel) {
                    continue;
                }
                let index = z * plane + pixel;
                set(&mut mask.bits, index);
                mask.info.positive_cells += 1;
                mask.info.valid_source_cells += usize::from(image.has_sample(pixel));
                if bit(&union, index) && !bit(&overlaps, index) {
                    set(&mut overlaps, index);
                    overlap_cells += 1;
                }
                set(&mut union, index);
            }
        }
        if declared_overlap == Some(false) && overlap_cells > 0 {
            return Err("Masks overlap despite a declaration that they do not");
        }
        if cancelled() {
            return Err("Cancelled");
        }
        Ok(Self {
            source,
            masks,
            overlap_cells,
            _reservation: reservation,
        })
    }
    pub fn source_arc(&self) -> Arc<Volume> {
        self.source.clone()
    }
    pub fn infos(&self) -> impl ExactSizeIterator<Item = SegmentInfo> + '_ {
        self.masks.iter().map(|m| m.info)
    }
    /// Number of source cells present in at least two segments; counts include
    /// stored positives on missing source samples. Segment counts are not a union.
    pub fn overlap_cells(&self) -> usize {
        self.overlap_cells
    }
    pub fn plane_is_supplied(&self, number: u16, z: usize) -> bool {
        z < self.source.dimensions()[2]
            && self
                .masks
                .iter()
                .find(|m| m.info.number == number)
                .is_some_and(|m| bit(&m.covered, z))
    }
    /// None means outside the grid, an unknown segment, or an omitted plane.
    /// Some retains the supplied bit even if the corresponding source sample is missing.
    pub fn value(&self, number: u16, voxel: [usize; 3]) -> Option<bool> {
        let [w, h, d] = self.source.dimensions();
        let [x, y, z] = voxel;
        if x >= w || y >= h || z >= d {
            return None;
        }
        let mask = self.masks.iter().find(|m| m.info.number == number)?;
        bit(&mask.covered, z).then(|| bit(&mask.bits, (z * h + y) * w + x))
    }
}
#[cfg(test)]
mod tests;
