// SPDX-License-Identifier: GPL-3.0-or-later
//! DICOM padding is an inclusive stored-code range, before rescale or palette lookup.
use crate::load::{LoadError, integer, text};
use dicom_core::{Tag, VR};
use dicom_object::DefaultDicomObject;
use dicom_pixeldata::DecodedPixelData;
use std::sync::Arc;

#[derive(Clone, Copy)]
pub(crate) struct Padding {
    low: i32,
    high: i32,
    stored: u16,
    signed: bool,
}
impl Padding {
    pub fn read(object: &DefaultDicomObject) -> Result<Option<Self>, LoadError> {
        let value = object.element(Tag(0x0028, 0x0120)).ok();
        let limit = object.element(Tag(0x0028, 0x0121)).ok();
        let Some(value) = value else {
            return if limit.is_some() {
                Err(LoadError::Unsupported)
            } else {
                Ok(None)
            };
        };
        let pi = text(object, "PhotometricInterpretation").ok_or(LoadError::Unsupported)?;
        let allocated = integer(object, "BitsAllocated").ok_or(LoadError::Unsupported)?;
        let stored = integer(object, "BitsStored").ok_or(LoadError::Unsupported)?;
        let signed = match integer(object, "PixelRepresentation") {
            Some(0) => false,
            Some(1) => true,
            _ => return Err(LoadError::Unsupported),
        };
        if !matches!(allocated, 1 | 8 | 16)
            || !(1..=allocated).contains(&stored)
            || integer(object, "HighBit") != Some(stored - 1)
            || integer(object, "SamplesPerPixel") != Some(1)
            || !matches!(pi.as_str(), "MONOCHROME1" | "MONOCHROME2" | "PALETTE COLOR")
        {
            return Err(LoadError::Unsupported);
        }
        let stored = stored as u16;
        let first = padding_value(value, signed, stored)?;
        let last = limit
            .map(|e| padding_value(e, signed, stored))
            .transpose()?
            .unwrap_or(first);
        if (pi == "MONOCHROME1" && first < last) || (pi != "MONOCHROME1" && first > last) {
            return Err(LoadError::Unsupported);
        }
        Ok(Some(Self {
            low: first.min(last),
            high: first.max(last),
            stored,
            signed,
        }))
    }

    pub fn mask(
        &self,
        decoded: &DecodedPixelData<'_>,
        frame: u32,
        cancelled: &impl Fn() -> bool,
    ) -> Result<Option<Arc<[u8]>>, LoadError> {
        let count = (decoded.rows() as usize)
            .checked_mul(decoded.columns() as usize)
            .ok_or(LoadError::Limit)?;
        if count > 32 * 1024 * 1024 {
            return Err(LoadError::Limit);
        }
        let bytes = decoded
            .frame_data(frame)
            .map_err(|_| LoadError::Unsupported)?;
        let allocated = decoded.bits_allocated();
        if decoded.samples_per_pixel() != 1
            || !matches!(allocated, 1 | 8 | 16)
            || bytes.len() != count * usize::from(allocated.div_ceil(8))
        {
            return Err(LoadError::Unsupported);
        }
        let mut mask = Vec::with_capacity(count);
        let mut missing = false;
        for i in 0..count {
            if i.is_multiple_of(16384) && cancelled() {
                return Err(LoadError::Cancelled);
            }
            let raw = if allocated <= 8 {
                u32::from(bytes[i])
            } else {
                u32::from(u16::from_ne_bytes([bytes[i * 2], bytes[i * 2 + 1]]))
            };
            let raw = (raw & ((1 << self.stored) - 1)) as i32;
            let value = if self.signed && raw & (1 << (self.stored - 1)) != 0 {
                raw - (1 << self.stored)
            } else {
                raw
            };
            let valid = !(self.low..=self.high).contains(&value);
            missing |= !valid;
            mask.push(u8::from(valid));
        }
        if cancelled() {
            return Err(LoadError::Cancelled);
        }
        Ok(missing.then(|| mask.into()))
    }
}

fn padding_value(
    element: &dicom_core::DataElement<dicom_object::mem::InMemDicomObject>,
    signed: bool,
    stored: u16,
) -> Result<i32, LoadError> {
    if !matches!(element.vr(), VR::US | VR::SS) || element.value().multiplicity() != 1 {
        return Err(LoadError::Unsupported);
    }
    let mut value = element
        .to_int::<i32>()
        .map_err(|_| LoadError::Unsupported)?;
    // The implicit-VR dictionary can select US for this context-dependent tag.
    // Recover its signed 16-bit field before validating Bits Stored.
    if signed && element.vr() == VR::US && (0..=65535).contains(&value) {
        value = value as u16 as i16 as i32;
    }
    let (low, high) = if signed {
        (-(1 << (stored - 1)), (1 << (stored - 1)) - 1)
    } else {
        (0, (1 << stored) - 1)
    };
    if !(low..=high).contains(&value) {
        return Err(LoadError::Unsupported);
    }
    Ok(value)
}

pub(crate) fn automatic_window(pixels: &[f32], mask: Option<&[u8]>) -> crate::WindowLevel {
    let (min, max) = pixels
        .iter()
        .enumerate()
        .filter(|(i, _)| mask.is_none_or(|m| m.get(*i) == Some(&1)))
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(min, max), (_, v)| {
            (min.min(*v), max.max(*v))
        });
    if min.is_finite() && max.is_finite() {
        crate::WindowLevel {
            width: (f64::from(max) - f64::from(min) + 1.).clamp(1., f64::from(f32::MAX)) as f32,
            center: ((f64::from(min) + f64::from(max)) / 2.) as f32,
        }
    } else {
        crate::WindowLevel {
            width: 1.,
            center: 0.,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn automatic_range_stays_finite_for_extreme_finite_samples() {
        for values in [[-f32::MAX, f32::MAX], [3e38, 3e38]] {
            let window = automatic_window(&values, None);
            assert!(window.is_valid());
            assert!(window.center.is_finite());
        }
    }
}
