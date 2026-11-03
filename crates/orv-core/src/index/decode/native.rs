// SPDX-License-Identifier: GPL-3.0-or-later
//! Bounded native reads. OW cells use low bits first in each 16-bit word.
use super::{Header, ReadState, Source, frame_object};
use crate::load::LoadError;
use dicom_core::{DataElement, PrimitiveValue, Tag, VR};
use dicom_object::DefaultDicomObject;

pub(super) fn object(
    source: &Source,
    header: &Header,
    frame: u32,
    state: &mut ReadState,
    cancelled: &impl Fn() -> bool,
) -> Result<DefaultDicomObject, LoadError> {
    let samples = u64::from(header.width)
        * u64::from(header.height)
        * crate::dicom_color::native_samples(header.dicom()?)?;
    let mut object = frame_object(header.dicom()?, frame)?;
    let value = if header.bits == 1
        || (header.bits == 8 && header.big_endian && header.pixel_vr == VR::OW)
    {
        let bytes = packed_bytes(
            source,
            header,
            frame,
            samples,
            header.bits,
            state,
            cancelled,
        )?;
        if header.bits == 1 {
            object.put(DataElement::new(
                Tag(0x0028, 0x0100),
                VR::US,
                PrimitiveValue::from(8u16),
            ));
        }
        PrimitiveValue::U8(bytes.into())
    } else {
        let length = samples * u64::from(header.bits / 8);
        let offset = header
            .offset
            .checked_add(
                length
                    .checked_mul(u64::from(frame))
                    .ok_or(LoadError::Limit)?,
            )
            .ok_or(LoadError::Limit)?;
        let bytes = source.range(offset, length as usize, state, cancelled)?;
        let word = |b| {
            if header.big_endian {
                u16::from_be_bytes(b)
            } else {
                u16::from_le_bytes(b)
            }
        };
        match header.bits {
            8 => PrimitiveValue::U8(bytes.into()),
            16 => PrimitiveValue::U16(bytes.as_chunks::<2>().0.iter().map(|b| word(*b)).collect()),
            32 => PrimitiveValue::U32(
                bytes
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|b| u32::from(word([b[0], b[1]])) | (u32::from(word([b[2], b[3]])) << 16))
                    .collect(),
            ),
            _ => return Err(LoadError::Unsupported),
        }
    };
    object.put(DataElement::new(
        Tag(0x7fe0, 0x0010),
        if header.bits <= 8 { VR::OB } else { VR::OW },
        value,
    ));
    Ok(object)
}

fn packed_bytes(
    source: &Source,
    header: &Header,
    frame: u32,
    count: u64,
    cell_bits: u16,
    state: &mut ReadState,
    cancelled: &impl Fn() -> bool,
) -> Result<Vec<u8>, LoadError> {
    let unit_bits = if header.pixel_vr == VR::OW { 16u64 } else { 8 };
    let cell_bits = u64::from(cell_bits);
    let frame_bits = count.checked_mul(cell_bits).ok_or(LoadError::Limit)?;
    let start = frame_bits
        .checked_mul(u64::from(frame))
        .ok_or(LoadError::Limit)?;
    let within = start % unit_bits;
    let length = (within + frame_bits).div_ceil(unit_bits) * (unit_bits / 8);
    let offset = header
        .offset
        .checked_add((start / unit_bits) * (unit_bits / 8))
        .ok_or(LoadError::Limit)?;
    let raw = source.range(offset, length as usize, state, cancelled)?;
    let mut result = Vec::new();
    result
        .try_reserve_exact(count as usize)
        .map_err(|_| LoadError::Limit)?;
    for index in 0..count {
        if index.is_multiple_of(16384) && cancelled() {
            return Err(LoadError::Cancelled);
        }
        let at = within + index * cell_bits;
        let unit = if unit_bits == 8 {
            u16::from(raw[(at / 8) as usize])
        } else {
            let offset = (at / 16 * 2) as usize;
            if header.big_endian {
                u16::from_be_bytes([raw[offset], raw[offset + 1]])
            } else {
                u16::from_le_bytes([raw[offset], raw[offset + 1]])
            }
        };
        result.push(((unit >> (at % unit_bits)) & ((1 << cell_bits) - 1)) as u8);
    }
    Ok(result)
}
