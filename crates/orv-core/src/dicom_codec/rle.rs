// SPDX-License-Identifier: GPL-3.0-or-later
//! Guard pinned decoder indexing/allocations using PS3.5 Annex G structure.
use super::*;
pub(super) fn validate(object: &DefaultDicomObject) -> Result<(), LoadError> {
    let (rows, cols, samples, bits) = (
        integer(object, "Rows").ok_or(LoadError::Unsupported)?,
        integer(object, "Columns").ok_or(LoadError::Unsupported)?,
        integer(object, "SamplesPerPixel").ok_or(LoadError::Unsupported)?,
        integer(object, "BitsAllocated").ok_or(LoadError::Unsupported)?,
    );
    if !matches!(samples, 1 | 3)
        || !matches!(bits, 8 | 16)
        || rows == 0
        || cols == 0
        || rows * cols * samples > MAX_FRAME_SAMPLES
    {
        return Err(LoadError::Limit);
    }
    let fragments = object
        .element_by_name("PixelData")
        .map_err(|_| LoadError::Unsupported)?
        .value()
        .fragments()
        .ok_or(LoadError::Unsupported)?;
    if fragments.len() as u64 != integer(object, "NumberOfFrames").unwrap_or(1) {
        return Err(LoadError::Unsupported);
    }
    let count = (samples * bits / 8) as usize;
    for frame in fragments {
        if frame.len() < 64 || frame.len() > 64 * 1024 * 1024 {
            return Err(LoadError::Limit);
        }
        let word = |at: usize| {
            u32::from_le_bytes([frame[at], frame[at + 1], frame[at + 2], frame[at + 3]]) as usize
        };
        if word(0) != count {
            return Err(LoadError::Unsupported);
        }
        for segment in 0..count {
            let begin = word(4 + segment * 4);
            let end = if segment + 1 == count {
                frame.len()
            } else {
                word(8 + segment * 4)
            };
            if begin < 64 || begin >= end || end > frame.len() {
                return Err(LoadError::Unsupported);
            }
            let mut at = begin;
            let mut output = 0u64;
            while at < end && output < rows * cols {
                let op = frame[at] as i8;
                at += 1;
                let (input, decoded) = match op {
                    0..=127 => (op as usize + 1, op as u64 + 1),
                    -127..=-1 => (1, (1i16 - op as i16) as u64),
                    _ => (0, 0),
                };
                if input > end - at || decoded > rows * cols - output {
                    return Err(LoadError::Unsupported);
                }
                at += input;
                output += decoded;
            }
            if output != rows * cols {
                return Err(LoadError::Unsupported);
            }
            // DICOM segment padding is optional zero/NOP. Never accept a hidden
            // second decoded image that a bounded decoder would silently ignore.
            if frame[at..end].iter().any(|b| !matches!(b, 0 | 128))
                || frame[at..end].iter().filter(|b| **b == 0).count() > 1
            {
                return Err(LoadError::Unsupported);
            }
        }
    }
    Ok(())
}
