// SPDX-License-Identifier: GPL-3.0-or-later
//! Explicit RGB presentation and source code values, separate from scalar modality data.
mod icc;
mod jpegls;
mod palette;
use crate::{
    color::{ColorData, ColorEncoding, ColorInfo},
    load::{LoadError, integer, text},
};
use dicom_object::DefaultDicomObject;
use dicom_pixeldata::{DecodedPixelData, PlanarConfiguration};
use std::sync::Arc;
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Color {
    Rgb,
    Ybr,
    PackedYbr,
    Palette,
}
fn native(object: &DefaultDicomObject) -> bool {
    matches!(
        object.meta().transfer_syntax(),
        "1.2.840.10008.1.2"
            | "1.2.840.10008.1.2.1"
            | "1.2.840.10008.1.2.2"
            | "1.2.840.10008.1.2.1.99"
    )
}
pub(crate) fn native_samples(object: &DefaultDicomObject) -> Result<u64, LoadError> {
    Ok(if layout(object)? == Some(Color::PackedYbr) {
        2
    } else {
        integer(object, "SamplesPerPixel").ok_or(LoadError::Unsupported)?
    })
}
pub(crate) fn layout(object: &DefaultDicomObject) -> Result<Option<Color>, LoadError> {
    let pi = text(object, "PhotometricInterpretation").ok_or(LoadError::Unsupported)?;
    if integer(object, "SamplesPerPixel") == Some(1)
        && matches!(pi.as_str(), "MONOCHROME1" | "MONOCHROME2")
    {
        return Ok(None);
    }
    let allocated = integer(object, "BitsAllocated").ok_or(LoadError::Unsupported)?;
    let stored = integer(object, "BitsStored").ok_or(LoadError::Unsupported)?;
    if !matches!(allocated, 8 | 16)
        || !(1..=allocated).contains(&stored)
        || integer(object, "HighBit") != Some(stored - 1)
    {
        return Err(LoadError::Unsupported);
    }
    if pi == "PALETTE COLOR"
        && integer(object, "SamplesPerPixel") == Some(1)
        && matches!(integer(object, "PixelRepresentation"), Some(0 | 1))
    {
        if [0x1104, 0x1204, 0x1224, 0x140b]
            .iter()
            .any(|tag| object.element(dicom_core::Tag(0x0028, *tag)).is_ok())
        {
            return Err(LoadError::Unsupported); // alpha/enhanced palettes need explicit presentation semantics
        }
        return Ok(Some(Color::Palette));
    }
    if integer(object, "SamplesPerPixel") != Some(3)
        || integer(object, "PixelRepresentation") != Some(0)
        || !matches!(integer(object, "PlanarConfiguration"), Some(0 | 1))
    {
        return Err(LoadError::Unsupported);
    }
    if native(object) {
        return match pi.as_str() {
            "RGB" => Ok(Some(Color::Rgb)),
            "YBR_FULL" => Ok(Some(Color::Ybr)),
            "YBR_FULL_422"
                if allocated == 8
                    && stored == 8
                    && integer(object, "PlanarConfiguration") == Some(0)
                    && integer(object, "Columns").is_some_and(|n| n.is_multiple_of(2)) =>
            {
                Ok(Some(Color::PackedYbr))
            }
            _ => Err(LoadError::Unsupported),
        };
    }
    // JPEG/JPEG2000 decode into RGB; RLE/JPEG-LS preserve channel code values.
    match object.meta().transfer_syntax() {
        "1.2.840.10008.1.2.4.50"
        | "1.2.840.10008.1.2.4.51"
        | "1.2.840.10008.1.2.4.57"
        | "1.2.840.10008.1.2.4.70"
            if matches!(pi.as_str(), "RGB" | "YBR_FULL" | "YBR_FULL_422") =>
        {
            Ok(Some(Color::Rgb))
        }
        "1.2.840.10008.1.2.4.90" | "1.2.840.10008.1.2.4.91"
            if matches!(pi.as_str(), "RGB" | "YBR_RCT" | "YBR_ICT") =>
        {
            Ok(Some(Color::Rgb))
        }
        "1.2.840.10008.1.2.5" | "1.2.840.10008.1.2.4.80" | "1.2.840.10008.1.2.4.81" => {
            match pi.as_str() {
                "RGB" => Ok(Some(Color::Rgb)),
                "YBR_FULL" => Ok(Some(Color::Ybr)),
                _ => Err(LoadError::Unsupported),
            }
        }
        _ => Err(LoadError::Unsupported),
    }
}
pub(crate) struct Presentation {
    pub rgba: Arc<[u8]>,
    pub source: Arc<ColorData>,
}
pub(crate) fn present(
    object: &DefaultDicomObject,
    decoded: Option<&DecodedPixelData<'_>>,
    mode: Color,
    frame: u32,
    padding: Option<crate::dicom_padding::Padding>,
    cancelled: &impl Fn() -> bool,
) -> Result<Presentation, LoadError> {
    let count = integer(object, "Rows").ok_or(LoadError::Unsupported)? as usize
        * integer(object, "Columns").ok_or(LoadError::Unsupported)? as usize;
    let stored = integer(object, "BitsStored").ok_or(LoadError::Unsupported)? as u16;
    let mask = ((1u32 << stored) - 1) as u16;
    let (rgb, bits) = if mode == Color::PackedYbr {
        let bytes = object
            .element_by_name("PixelData")
            .map_err(|_| LoadError::Unsupported)?
            .to_bytes()
            .map_err(|_| LoadError::Unsupported)?;
        let begin = count
            .checked_mul(2)
            .and_then(|n| n.checked_mul(frame as usize))
            .ok_or(LoadError::Limit)?;
        let input = bytes
            .get(begin..begin + count * 2)
            .ok_or(LoadError::Unsupported)?;
        let mut rgb = Vec::with_capacity(count * 3);
        for (i, v) in input.as_chunks::<4>().0.iter().enumerate() {
            if i.is_multiple_of(8192) && cancelled() {
                return Err(LoadError::Cancelled);
            }
            rgb.extend_from_slice(&ybr([v[0] as u16, v[2] as u16, v[3] as u16], stored));
            rgb.extend_from_slice(&ybr([v[1] as u16, v[2] as u16, v[3] as u16], stored));
        }
        (rgb, stored)
    } else {
        let decoded = decoded.ok_or(LoadError::Unsupported)?;
        let bytes = decoded
            .frame_data(frame)
            .map_err(|_| LoadError::Unsupported)?;
        // dicom-rs 0.10's 3-channel RLE adapter emits MSB/LSB byte order for
        // 16-bit samples, unlike its 1-channel/native/JPEG-LS adapters. Verified
        // against an independently packed six-segment fixture; do not mask first.
        let rle_big = object.meta().transfer_syntax() == "1.2.840.10008.1.2.5"
            && decoded.samples_per_pixel() == 3;
        let samples = |index: usize| -> u16 {
            if decoded.bits_allocated() == 8 {
                bytes[index] as u16 & mask
            } else {
                let raw = [bytes[index * 2], bytes[index * 2 + 1]];
                (if rle_big {
                    u16::from_be_bytes(raw)
                } else {
                    u16::from_ne_bytes(raw)
                }) & mask
            }
        };
        if mode == Color::Palette {
            let palette = palette::Palette::read(object)?;
            let signed = integer(object, "PixelRepresentation") == Some(1);
            let mut rgb = Vec::with_capacity(count * 3);
            for i in 0..count {
                if i.is_multiple_of(16384) && cancelled() {
                    return Err(LoadError::Cancelled);
                }
                let raw = samples(i) as i32;
                let index = if signed && (raw & (1 << (stored - 1))) != 0 {
                    raw - (1 << stored)
                } else {
                    raw
                };
                rgb.extend_from_slice(&palette.at(index));
            }
            (rgb, palette.bits)
        } else {
            if bytes.len() != count * 3 * (decoded.bits_allocated() / 8) as usize {
                return Err(LoadError::Unsupported);
            }
            // JPEG-LS specifies its own component/line/sample interleave in
            // SOS. CharLS normalizes line/sample mode to pixels but leaves
            // component mode planar. dicom-rs marks every encapsulated buffer
            // Standard; DICOM Planar Configuration is irrelevant here.
            let jls_layout = jpegls::layout(object, cancelled)?;
            let planar = decoded.planar_configuration() == PlanarConfiguration::PixelFirst;
            let mut rgb = Vec::with_capacity(count * 3);
            for i in 0..count {
                if i.is_multiple_of(16384) && cancelled() {
                    return Err(LoadError::Cancelled);
                }
                let value = std::array::from_fn(|c| {
                    samples(match jls_layout {
                        Some(jpegls::Layout::Component) => c * count + i,
                        Some(jpegls::Layout::Sample) => i * 3 + c,
                        None if planar => c * count + i,
                        None => i * 3 + c,
                    })
                });
                rgb.extend_from_slice(&if mode == Color::Ybr {
                    ybr(value, stored)
                } else {
                    value
                });
            }
            (rgb, stored)
        }
    };
    if rgb.len() != count * 3 {
        return Err(LoadError::Unsupported);
    }
    let (mut rgba, profile) = icc::to_srgb(object, &rgb, bits, cancelled)?;
    if let Some(padding) = padding {
        let mask = padding.mask(decoded.ok_or(LoadError::Unsupported)?, frame, cancelled)?;
        if let Some(mask) = mask {
            for (i, valid) in mask.iter().enumerate() {
                if i.is_multiple_of(16384) && cancelled() {
                    return Err(LoadError::Cancelled);
                }
                if *valid == 0 {
                    rgba[i * 4..i * 4 + 4].fill(0);
                }
            }
        }
    }
    let source_encoding = match text(object, "PhotometricInterpretation").as_deref() {
        Some("PALETTE COLOR") => ColorEncoding::Palette,
        Some("YBR_FULL_422") => ColorEncoding::YbrFull422,
        Some("YBR_FULL") => ColorEncoding::YbrFull,
        Some("YBR_RCT") => ColorEncoding::YbrRct,
        Some("YBR_ICT") => ColorEncoding::YbrIct,
        _ => ColorEncoding::Rgb,
    };
    Ok(Presentation {
        rgba: rgba.into(),
        source: Arc::new(ColorData {
            info: ColorInfo {
                source_encoding,
                bits,
                icc_sha256: profile,
            },
            rgb: rgb.into(),
        }),
    })
}
fn ybr(value: [u16; 3], bits: u16) -> [u16; 3] {
    let (y, cb, cr) = (
        value[0] as f64,
        value[1] as f64 - (1u32 << (bits - 1)) as f64,
        value[2] as f64 - (1u32 << (bits - 1)) as f64,
    );
    let maximum = ((1u32 << bits) - 1) as f64;
    [
        y + 1.402 * cr,
        y - 0.344136 * cb - 0.714136 * cr,
        y + 1.772 * cb,
    ]
    .map(|v| v.round().clamp(0., maximum) as u16)
}
