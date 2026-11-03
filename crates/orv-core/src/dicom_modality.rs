// SPDX-License-Identifier: GPL-3.0-or-later
//! A single integer modality table; stored padding is handled independently.
use crate::{
    load::LoadError,
    scalar::{ModalityLut, ScalarData, Storage},
};
use dicom_core::{PrimitiveValue, Tag, VR};
use dicom_object::DefaultDicomObject;
use dicom_pixeldata::DecodedPixelData;
use std::sync::Arc;
pub(crate) const TABLE: Tag = Tag(0x0028, 0x3000);
pub(crate) struct Mapping {
    pub lut: Arc<ModalityLut>,
    pub unit: String,
}
pub(crate) fn read(object: &DefaultDicomObject) -> Result<Option<Mapping>, LoadError> {
    let Ok(sequence) = object.element(TABLE) else {
        return Ok(None);
    };
    let items = sequence.items().ok_or(LoadError::Unsupported)?;
    if sequence.vr() != VR::SQ
        || items.len() != 1
        || [
            Tag(0x0028, 0x1052),
            Tag(0x0028, 0x1053),
            Tag(0x0028, 0x1054),
        ]
        .iter()
        .any(|tag| object.element(*tag).is_ok())
    {
        return Err(LoadError::Unsupported);
    }
    let item = &items[0];
    let descriptor = item
        .element(Tag(0x0028, 0x3002))
        .map_err(|_| LoadError::Unsupported)?;
    let signed = match crate::load::integer(object, "PixelRepresentation") {
        Some(0) => false,
        Some(1) => true,
        _ => return Err(LoadError::Unsupported),
    };
    let implicit = object.meta().transfer_syntax() == "1.2.840.10008.1.2";
    if !matches!(descriptor.vr(), VR::US | VR::SS)
        || (!implicit && descriptor.vr() != if signed { VR::SS } else { VR::US })
    {
        return Err(LoadError::Unsupported);
    }
    let words = descriptor
        .to_multi_int::<i32>()
        .map_err(|_| LoadError::Unsupported)?;
    if words.len() != 3 || words.iter().any(|v| !(-32768..=65535).contains(v)) {
        return Err(LoadError::Unsupported);
    }
    let count = if words[0] as u16 == 0 {
        65536
    } else {
        words[0] as u16 as usize
    };
    let first = if signed {
        words[1] as u16 as i16 as i32
    } else {
        words[1] as u16 as i32
    };
    let bits = words[2] as u16;
    if !matches!(bits, 8 | 16) {
        return Err(LoadError::Unsupported);
    }
    let values = crate::dicom_voi::table_values(
        item.element(Tag(0x0028, 0x3006))
            .map_err(|_| LoadError::Unsupported)?,
        count,
        bits,
        object.meta().transfer_syntax() == "1.2.840.10008.1.2.2",
    )?;
    let unit = item
        .element(Tag(0x0028, 0x3004))
        .map_err(|_| LoadError::Unsupported)?;
    if unit.vr() != VR::LO || unit.value().multiplicity() != 1 {
        return Err(LoadError::Unsupported);
    }
    let unit = unit.to_str().map_err(|_| LoadError::Unsupported)?;
    let unit = unit.trim();
    if unit.is_empty() || unit.len() > 64 || unit.contains('\\') {
        return Err(LoadError::Unsupported);
    }
    Ok(Some(Mapping {
        lut: ModalityLut::new(first, bits, values).map_err(|_| LoadError::Unsupported)?,
        unit: unit.into(),
    }))
}

pub(crate) fn pixels(
    object: &DefaultDicomObject,
    decoded: &DecodedPixelData<'_>,
    frame: u32,
    source_frame: u32,
    scalar: &ScalarData,
    cancelled: &impl Fn() -> bool,
) -> Result<Vec<f32>, LoadError> {
    let lut = scalar.modality_lut().ok_or(LoadError::Unsupported)?;
    let info = scalar.info();
    let allocated = decoded.bits_allocated();
    if info.resampled
        || decoded.samples_per_pixel() != 1
        || !matches!(allocated, 1 | 8 | 16 | 32)
        || decoded.bits_stored() != info.stored_bits
        || !(1..=allocated).contains(&info.stored_bits)
    {
        return Err(LoadError::Unsupported);
    }
    let count = (decoded.rows() as usize)
        .checked_mul(decoded.columns() as usize)
        .filter(|v| *v <= 32 * 1024 * 1024)
        .ok_or(LoadError::Limit)?;
    let width = usize::from(allocated.div_ceil(8));
    let (codes, offset) = codes(object, decoded, frame, source_frame, count, width)?;
    let mut pixels = Vec::new();
    pixels
        .try_reserve_exact(count)
        .map_err(|_| LoadError::Limit)?;
    let mask = (1u64 << info.stored_bits) - 1;
    for index in 0..count {
        if index.is_multiple_of(16384) && cancelled() {
            return Err(LoadError::Cancelled);
        }
        let raw = u64::from(codes.at(offset + index).ok_or(LoadError::Unsupported)?) & mask;
        let stored = if info.storage == Storage::SignedInteger
            && raw & (1u64 << (info.stored_bits - 1)) != 0
        {
            raw as i64 - (1i64 << info.stored_bits)
        } else {
            raw as i64
        };
        pixels.push(lut.map(stored));
    }
    if cancelled() {
        return Err(LoadError::Cancelled);
    }
    Ok(pixels)
}

pub(crate) fn mask(
    object: &DefaultDicomObject,
    decoded: &DecodedPixelData<'_>,
    frame: u32,
    source_frame: u32,
    padding: crate::dicom_padding::Padding,
    cancelled: &impl Fn() -> bool,
) -> Result<Option<Arc<[u8]>>, LoadError> {
    let count = (decoded.rows() as usize)
        .checked_mul(decoded.columns() as usize)
        .filter(|v| *v <= 32 * 1024 * 1024)
        .ok_or(LoadError::Limit)?;
    let width = usize::from(decoded.bits_allocated().div_ceil(8));
    let (codes, offset) = codes(object, decoded, frame, source_frame, count, width)?;
    padding.mask_codes(count, |index| codes.at(offset + index), cancelled)
}

enum Codes<'a> {
    BitsBytes(&'a [u8]),
    BitsWords(&'a [u16]),
    Bytes(&'a [u8]),
    ByteWords(&'a [u16]),
    Words(&'a [u16]),
    DoubleWords(&'a [u16]),
    Dwords(&'a [u32]),
    Decoded(&'a [u8], usize),
}
impl Codes<'_> {
    fn at(&self, index: usize) -> Option<u32> {
        Some(match self {
            Self::BitsBytes(values) => u32::from((*values.get(index / 8)? >> (index % 8)) & 1),
            Self::BitsWords(values) => u32::from((*values.get(index / 16)? >> (index % 16)) & 1),
            Self::Bytes(values) => u32::from(*values.get(index)?),
            Self::ByteWords(values) => {
                let word = *values.get(index / 2)?;
                let high = !index.is_multiple_of(2);
                u32::from(if high { word >> 8 } else { word & 255 })
            }
            Self::Words(values) => u32::from(*values.get(index)?),
            Self::DoubleWords(values) => {
                let words =
                    values.get(index.checked_mul(2)?..index.checked_mul(2)?.checked_add(2)?)?;
                let (low, high) = (words[0], words[1]);
                u32::from(low) | (u32::from(high) << 16)
            }
            Self::Dwords(values) => *values.get(index)?,
            Self::Decoded(values, width) => {
                let bytes = values.get(
                    index.checked_mul(*width)?..index.checked_mul(*width)?.checked_add(*width)?,
                )?;
                match width {
                    1 => u32::from(bytes[0]),
                    2 => u32::from(u16::from_ne_bytes([bytes[0], bytes[1]])),
                    4 => u32::from_ne_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
                    _ => return None,
                }
            }
        })
    }
}
fn codes<'a>(
    object: &'a DefaultDicomObject,
    decoded: &'a DecodedPixelData<'_>,
    frame: u32,
    source_frame: u32,
    count: usize,
    width: usize,
) -> Result<(Codes<'a>, usize), LoadError> {
    let syntax = object.meta().transfer_syntax();
    if matches!(
        syntax,
        "1.2.840.10008.1.2"
            | "1.2.840.10008.1.2.1"
            | "1.2.840.10008.1.2.2"
            | "1.2.840.10008.1.2.1.99"
    ) {
        let value = object
            .element(Tag(0x7fe0, 0x0010))
            .map_err(|_| LoadError::Unsupported)?
            .value()
            .primitive()
            .ok_or(LoadError::Unsupported)?;
        // OW is a stream of native 16-bit words. Pixel cells start in its low
        // bits and can cross a word or frame boundary (PS3.5 8.2 and Annex D).
        let codes = match (value, width) {
            (PrimitiveValue::U8(values), 1) if decoded.bits_allocated() == 1 => {
                Codes::BitsBytes(values)
            }
            (PrimitiveValue::U16(values), 1) if decoded.bits_allocated() == 1 => {
                Codes::BitsWords(values)
            }
            (PrimitiveValue::U8(values), 1) => Codes::Bytes(values),
            (PrimitiveValue::U16(values), 1) => Codes::ByteWords(values),
            (PrimitiveValue::U16(values), 2) => Codes::Words(values),
            (PrimitiveValue::U16(values), 4) => Codes::DoubleWords(values),
            (PrimitiveValue::U32(values), 4) => Codes::Dwords(values),
            _ => return Err(LoadError::Unsupported),
        };
        let offset = (source_frame as usize)
            .checked_mul(count)
            .ok_or(LoadError::Limit)?;
        return Ok((codes, offset));
    }
    let bytes = decoded
        .frame_data(frame)
        .map_err(|_| LoadError::Unsupported)?;
    if bytes.len() != count * width {
        return Err(LoadError::Unsupported);
    }
    Ok((Codes::Decoded(bytes, width), 0))
}
