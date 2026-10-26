// SPDX-License-Identifier: GPL-3.0-or-later
//! Frame boundaries from DICOM basic/extended offsets. Never guess ambiguous
//! multi-fragment frames from a large object with an empty offset table.
use super::{
    decode::MAX_ENCODED_OBJECT,
    header::Header,
    source::{ReadState, Source},
};
use crate::load::LoadError;
use dicom_core::Tag;

pub(super) struct Offsets {
    starts: Vec<u64>,
    end: Option<u64>,
}
const ITEM: [u8; 4] = [0xfe, 0xff, 0x00, 0xe0];
const END: [u8; 4] = [0xfe, 0xff, 0xdd, 0xe0];

pub(super) fn fragments(
    source: &Source,
    header: &Header,
    frame: u32,
    state: &mut ReadState,
    cancelled: &impl Fn() -> bool,
) -> Result<Vec<Vec<u8>>, LoadError> {
    if header.offsets.get().is_none() {
        let offsets = index(source, header, state, cancelled)?;
        if cancelled() {
            return Err(LoadError::Cancelled);
        }
        let _ = header.offsets.set(offsets);
    }
    let offsets = header.offsets.get().ok_or(LoadError::Unsupported)?;
    let mut cursor = *offsets
        .starts
        .get(frame as usize)
        .ok_or(LoadError::Unsupported)?;
    let limit = offsets
        .starts
        .get(frame as usize + 1)
        .copied()
        .or(offsets.end);
    let mut output = Vec::new();
    let mut bytes = 0u64;
    loop {
        if cancelled() {
            return Err(LoadError::Cancelled);
        }
        if limit == Some(cursor) {
            break;
        }
        let item = source.range(cursor, 8, state, cancelled)?;
        let length =
            u32::from_le_bytes(item[4..8].try_into().map_err(|_| LoadError::Unsupported)?) as u64;
        if item[..4] == END && length == 0 && limit.is_none() {
            break;
        }
        if item[..4] != ITEM
            || length == 0
            || !length.is_multiple_of(2)
            || length > MAX_ENCODED_OBJECT
        {
            return Err(LoadError::Unsupported);
        }
        bytes = bytes.checked_add(length).ok_or(LoadError::Limit)?;
        let next = cursor
            .checked_add(8)
            .and_then(|n| n.checked_add(length))
            .ok_or(LoadError::Limit)?;
        if bytes > MAX_ENCODED_OBJECT || output.len() >= 4096 || next > source.length {
            return Err(LoadError::Limit);
        }
        if limit.is_some_and(|end| next > end) {
            return Err(LoadError::Unsupported);
        }
        output.push(source.range(cursor + 8, length as usize, state, cancelled)?);
        cursor = next;
    }
    if output.is_empty() {
        return Err(LoadError::Unsupported);
    }
    Ok(output)
}

fn index(
    source: &Source,
    header: &Header,
    state: &mut ReadState,
    cancelled: &impl Fn() -> bool,
) -> Result<Offsets, LoadError> {
    let item = source.range(header.offset, 8, state, cancelled)?;
    if item[..4] != ITEM {
        return Err(LoadError::Unsupported);
    }
    let length =
        u32::from_le_bytes(item[4..8].try_into().map_err(|_| LoadError::Unsupported)?) as usize;
    if length > header.frames as usize * 4 || !length.is_multiple_of(4) {
        return Err(LoadError::Unsupported);
    }
    let first = header
        .offset
        .checked_add(8 + length as u64)
        .ok_or(LoadError::Limit)?;
    let relative = if let Ok(extended) = header.dicom()?.element(Tag(0x7fe0, 0x0001)) {
        if length != 0 {
            return Err(LoadError::Unsupported);
        }
        extended
            .to_multi_int::<u64>()
            .map_err(|_| LoadError::Unsupported)?
    } else if length != 0 {
        source
            .range(header.offset + 8, length, state, cancelled)?
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| u64::from(u32::from_le_bytes([b[0], b[1], b[2], b[3]])))
            .collect()
    } else {
        Vec::new()
    };
    if !relative.is_empty() {
        if relative.len() != header.frames as usize
            || relative.first() != Some(&0)
            || relative.windows(2).any(|p| p[0] >= p[1])
        {
            return Err(LoadError::Unsupported);
        }
        let starts = relative
            .into_iter()
            .map(|offset| {
                first
                    .checked_add(offset)
                    .filter(|n| *n < source.length)
                    .ok_or(LoadError::Unsupported)
            })
            .collect::<Result<Vec<_>, _>>()?;
        return Ok(Offsets { starts, end: None });
    }
    if header.frames == 1 {
        return Ok(Offsets {
            starts: vec![first],
            end: None,
        });
    }
    // With no offsets, only an exactly one-fragment-per-frame sequence is
    // unambiguous. A small ambiguous object may use the library's full parser.
    let mut starts = Vec::new();
    let mut cursor = first;
    loop {
        let item = source.range(cursor, 8, state, cancelled)?;
        let length =
            u32::from_le_bytes(item[4..8].try_into().map_err(|_| LoadError::Unsupported)?) as u64;
        if item[..4] == END && length == 0 {
            break;
        }
        if item[..4] != ITEM
            || length == 0
            || !length.is_multiple_of(2)
            || length > MAX_ENCODED_OBJECT
            || starts.len() >= header.frames as usize
        {
            return Err(LoadError::Unsupported);
        }
        starts.push(cursor);
        cursor = cursor
            .checked_add(8)
            .and_then(|n| n.checked_add(length))
            .filter(|n| *n < source.length)
            .ok_or(LoadError::Unsupported)?;
    }
    if starts.len() != header.frames as usize {
        return Err(LoadError::Unsupported);
    }
    Ok(Offsets {
        starts,
        end: Some(cursor),
    })
}
