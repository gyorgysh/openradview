// SPDX-License-Identifier: GPL-3.0-or-later
//! Check new codecs' declared dimensions before asking a decoder to allocate.
use crate::load::{LoadError, integer};
const MAX_FRAME_SAMPLES: u64 = 32 * 1024 * 1024;
use dicom_object::DefaultDicomObject;
mod rle;
#[cfg(test)]
mod security_tests;

pub(crate) fn validate(object: &DefaultDicomObject) -> Result<(), LoadError> {
    let uid = object.meta().transfer_syntax();
    if uid == "1.2.840.10008.1.2.5" {
        return rle::validate(object);
    }
    let jpeg2k = matches!(uid, "1.2.840.10008.1.2.4.90" | "1.2.840.10008.1.2.4.91");
    let jpegls = matches!(uid, "1.2.840.10008.1.2.4.80" | "1.2.840.10008.1.2.4.81");
    if !jpeg2k && !jpegls {
        return Ok(());
    }
    // Indexed imports assemble exactly one frame before reaching this function.
    if integer(object, "NumberOfFrames").unwrap_or(1) != 1 {
        return Err(LoadError::Unsupported);
    }
    let data: Vec<u8> = object
        .element_by_name("PixelData")
        .map_err(|_| LoadError::Unsupported)?
        .value()
        .fragments()
        .ok_or(LoadError::Unsupported)?
        .iter()
        .flat_map(|f| f.iter().copied())
        .take(64 * 1024)
        .collect();
    let rows = integer(object, "Rows").ok_or(LoadError::Unsupported)?;
    let cols = integer(object, "Columns").ok_or(LoadError::Unsupported)?;
    let samples = integer(object, "SamplesPerPixel").ok_or(LoadError::Unsupported)?;
    let bits = integer(object, "BitsStored").ok_or(LoadError::Unsupported)?;
    if !matches!(samples, 1 | 3)
        || !matches!(bits, 1..=16)
        || rows * cols * samples > MAX_FRAME_SAMPLES
    {
        return Err(LoadError::Limit);
    }
    let fail = || LoadError::Unsupported;
    if jpeg2k {
        if data.len() < 42 || data[..4] != [0xff, 0x4f, 0xff, 0x51] {
            return Err(fail());
        }
        let u16be = |n| u16::from_be_bytes([data[n], data[n + 1]]) as u64;
        let u32be = |n| u32::from_be_bytes([data[n], data[n + 1], data[n + 2], data[n + 3]]) as u64;
        let n = u16be(40);
        if n != samples
            || u16be(4) != 38 + 3 * n
            || data.len() < 42 + 3 * n as usize
            || u32be(8).checked_sub(u32be(16)) != Some(cols)
            || u32be(12).checked_sub(u32be(20)) != Some(rows)
            || !(1..=8192).contains(&u32be(24))
            || !(1..=8192).contains(&u32be(28))
        {
            return Err(fail());
        }
        for component in data[42..42 + 3 * n as usize].as_chunks::<3>().0.iter() {
            if (component[0] & 127) as u64 + 1 != bits
                || u64::from(component[0] >> 7)
                    != integer(object, "PixelRepresentation").unwrap_or(0)
                || component[1..] != [1, 1]
            {
                return Err(fail());
            }
        }
    } else {
        if !data.starts_with(&[0xff, 0xd8]) {
            return Err(fail());
        }
        let mut at = 2;
        loop {
            if at + 4 > data.len() || data[at] != 0xff {
                return Err(fail());
            }
            let marker = data[at + 1];
            let size = u16::from_be_bytes([data[at + 2], data[at + 3]]) as usize;
            if size < 2 || at + 2 + size > data.len() {
                return Err(fail());
            }
            if marker == 0xf7 {
                if size != 8 + 3 * samples as usize
                    || data[at + 4] as u64 != bits
                    || u16::from_be_bytes([data[at + 5], data[at + 6]]) as u64 != rows
                    || u16::from_be_bytes([data[at + 7], data[at + 8]]) as u64 != cols
                    || data[at + 9] as u64 != samples
                {
                    return Err(fail());
                }
                break;
            }
            if matches!(marker, 0xda | 0xd9) {
                return Err(fail());
            }
            at += size + 2;
        }
    }
    Ok(())
}
