// SPDX-License-Identifier: GPL-3.0-or-later
//! Original fractional samples, sparse source planes and a separate binary view.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FractionalKind {
    Probability,
    Occupancy,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FractionalEncoding {
    pub kind: FractionalKind,
    pub maximum: u8,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SegmentEncoding {
    Binary,
    Fractional(FractionalEncoding),
}
impl SegmentEncoding {
    pub fn maximum(self) -> u8 {
        match self {
            Self::Binary => 1,
            Self::Fractional(e) => e.maximum,
        }
    }
    /// Existing categorical counts/overlays use a fixed half-maximum view.
    /// This does not replace the original fractional samples or their meaning.
    pub fn positive(self, value: u8) -> bool {
        value > 0 && u16::from(value) * 2 >= u16::from(self.maximum())
    }
}
struct Input<'a> {
    numbers: &'a [u16],
    frames: &'a [(u16, usize)],
    payload: &'a [u8],
    encoding: FractionalEncoding,
    declared_overlap: Option<bool>,
}
fn shape(
    dimensions: [usize; 3],
    segments: usize,
    frames: usize,
) -> Result<(usize, usize, u64), &'static str> {
    if !(1..=MAX_SEGMENTS).contains(&segments) || !(1..=MAX_FRAMES).contains(&frames) {
        return Err("Supplied masks need one to 64 IDs and one to 4096 frames");
    }
    let cells = crate::labels::Labels::check_dimensions(dimensions)?;
    if dimensions[2] > MAX_FRAMES {
        return Err("Supplied masks support up to 4096 source planes");
    }
    let bytes = dimensions[0]
        .checked_mul(dimensions[1])
        .and_then(|n| n.checked_mul(frames))
        .filter(|n| *n <= MAX_MASK_BYTES && n.next_multiple_of(2) <= MAX_INPUT_BYTES)
        .ok_or("Supplied fractional samples exceed the 128 MiB mask limit")?;
    let coverage = dimensions[2].div_ceil(8);
    let metadata =
        (dimensions[2] * std::mem::size_of::<u32>() + coverage + std::mem::size_of::<Mask>())
            * segments;
    Ok((cells.div_ceil(8), bytes, (bytes + metadata) as u64))
}
impl Segments {
    /// Header-only admission for raw 8-bit values, stored only on supplied planes.
    pub fn check_fractional_layout(
        dimensions: [usize; 3],
        segments: usize,
        frames: usize,
    ) -> Result<(), &'static str> {
        shape(dimensions, segments, frames).map(|_| ())
    }
    /// Source references must already be verified. Retain every original byte,
    /// its maximum and probability/occupancy meaning. Counts/overlays remain the
    /// fixed half-maximum categorical view. Final even-byte padding is ignored.
    pub fn from_fractional(
        source: Arc<Volume>,
        numbers: &[u16],
        frames: &[(u16, usize)],
        payload: &[u8],
        encoding: FractionalEncoding,
        declared_overlap: Option<bool>,
        cancelled: impl Fn() -> bool,
    ) -> Result<Self, &'static str> {
        build(
            source,
            Input {
                numbers,
                frames,
                payload,
                encoding,
                declared_overlap,
            },
            &cancelled,
            &MemoryBudget::volumes(),
        )
    }
}
fn build(
    source: Arc<Volume>,
    input: Input<'_>,
    cancelled: &dyn Fn() -> bool,
    budget: &Arc<MemoryBudget>,
) -> Result<Segments, &'static str> {
    if cancelled() {
        return Err("Cancelled");
    }
    let dimensions = source.dimensions();
    let (bit_bytes, bytes, retained) = shape(dimensions, input.numbers.len(), input.frames.len())?;
    if input.encoding.maximum == 0 {
        return Err("Fractional maximum must be nonzero");
    }
    if input.payload.len() != bytes.next_multiple_of(2) {
        return Err("Fractional payload length does not match its frames");
    }
    if input
        .numbers
        .iter()
        .enumerate()
        .any(|(i, n)| *n == 0 || input.numbers[..i].contains(n))
    {
        return Err("Segment IDs must be unique nonzero numbers");
    }
    if input
        .frames
        .iter()
        .any(|(n, z)| !input.numbers.contains(n) || *z >= dimensions[2])
    {
        return Err("Mask frame references an unknown segment or source plane");
    }
    let reservation = budget
        .reserve(retained)
        .ok_or("Not enough memory for supplied fractional masks")?;
    let _temporary = budget
        .reserve(bit_bytes as u64 * 2)
        .ok_or("Not enough memory to check mask overlap")?;
    let mut union = zeros(bit_bytes)?;
    let mut overlaps = zeros(bit_bytes)?;
    let plane = dimensions[0] * dimensions[1];
    let mut masks = Vec::new();
    masks
        .try_reserve_exact(input.numbers.len())
        .map_err(|_| "Not enough memory for segment metadata")?;
    for &number in input.numbers {
        if cancelled() {
            return Err("Cancelled");
        }
        let count = input.frames.iter().filter(|(n, _)| *n == number).count();
        let mut offsets = Vec::new();
        offsets
            .try_reserve_exact(dimensions[2])
            .map_err(|_| "Not enough memory for fractional plane addresses")?;
        offsets.resize(dimensions[2], u32::MAX);
        masks.push(Mask {
            info: SegmentInfo {
                number,
                positive_cells: 0,
                valid_source_cells: 0,
                supplied_planes: 0,
            },
            samples: Samples::Fractional {
                values: zeros(count * plane)?,
                plane_offsets: offsets,
            },
            covered: zeros(dimensions[2].div_ceil(8))?,
        });
    }
    let encoding = SegmentEncoding::Fractional(input.encoding);
    let mut overlap_cells = 0;
    for (f, &(number, z)) in input.frames.iter().enumerate() {
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
        let Samples::Fractional {
            values,
            plane_offsets,
        } = &mut mask.samples
        else {
            return Err("Expected fractional samples");
        };
        let offset = mask.info.supplied_planes * plane;
        plane_offsets[z] =
            u32::try_from(offset).map_err(|_| "Fractional plane address overflow")?;
        mask.info.supplied_planes += 1;
        set(&mut mask.covered, z);
        for pixel in 0..plane {
            if pixel.is_multiple_of(1024) && cancelled() {
                return Err("Cancelled");
            }
            let value = input.payload[f * plane + pixel];
            if value > input.encoding.maximum {
                return Err("Fractional mask values exceed the declared maximum");
            }
            values[offset + pixel] = value;
            if !encoding.positive(value) {
                continue;
            }
            mask.info.positive_cells += 1;
            mask.info.valid_source_cells += usize::from(source.frames()[z].has_sample(pixel));
            let index = z * plane + pixel;
            if bit(&union, index) && !bit(&overlaps, index) {
                set(&mut overlaps, index);
                overlap_cells += 1;
            }
            set(&mut union, index);
        }
    }
    if input.declared_overlap == Some(false) && overlap_cells > 0 {
        return Err("Masks overlap despite a declaration that they do not");
    }
    if cancelled() {
        return Err("Cancelled");
    }
    let (object, revision) = crate::mask_identity::MaskObjectId::new(budget)?;
    Ok(Segments {
        source,
        encoding,
        object,
        revision,
        masks,
        overlap_cells,
        _reservation: reservation,
    })
}

#[cfg(test)]
mod tests;
