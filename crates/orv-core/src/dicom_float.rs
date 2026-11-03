// SPDX-License-Identifier: GPL-3.0-or-later
//! Native Float Pixel Data (PS3.3 C.7.6.3, C.7.6.24). Values are kept as stored
//! 32-bit floats. Integer Bits Stored, rescale and modality tables do not apply.
use crate::{
    load::{LoadError, integer, text},
    scalar::RealWorld,
};
use dicom_core::{PrimitiveValue, Tag, VR};
use dicom_object::DefaultDicomObject;
use std::sync::Arc;

pub(crate) const FLOAT: Tag = Tag(0x7fe0, 0x0008);
pub(crate) const DOUBLE: Tag = Tag(0x7fe0, 0x0009);
const PADDING: Tag = Tag(0x0028, 0x0122);
const PADDING_LIMIT: Tag = Tag(0x0028, 0x0124);

/// Stored samples with a validity mask when any sample is missing.
pub(crate) type Samples = (Vec<f32>, Option<Arc<[u8]>>);

/// Inclusive padding range in the stored float domain.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Padding {
    low: f32,
    high: f32,
}

/// Validate the image description shared by indexed and eager reads. The pixel
/// element itself may be absent from a bounded metadata prefix.
pub(crate) fn validate(object: &DefaultDicomObject) -> Result<Option<Padding>, LoadError> {
    let absent = |tag| object.element(tag).is_err();
    if integer(object, "SamplesPerPixel") != Some(1)
        || integer(object, "BitsAllocated") != Some(32)
        || text(object, "PhotometricInterpretation").as_deref() != Some("MONOCHROME2")
        // Integer pixel descriptions, integer padding and stored-value
        // transforms have no defined meaning for float samples.
        || ![
            Tag(0x0028, 0x0101),
            Tag(0x0028, 0x0102),
            Tag(0x0028, 0x0103),
            Tag(0x0028, 0x0120),
            Tag(0x0028, 0x0121),
            Tag(0x0028, 0x1052),
            Tag(0x0028, 0x1053),
            Tag(0x0028, 0x1054),
            Tag(0x0028, 0x3000),
            Tag(0x0028, 0x3010),
            Tag(0x0028, 0x1101),
            Tag(0x0028, 0x1201),
            Tag(0x7fe0, 0x0010),
            DOUBLE,
        ]
        .into_iter()
        .all(absent)
    {
        return Err(LoadError::Unsupported);
    }
    let value = |tag| -> Result<Option<f32>, LoadError> {
        let Ok(element) = object.element(tag) else {
            return Ok(None);
        };
        if element.vr() != VR::FL || element.value().multiplicity() != 1 {
            return Err(LoadError::Unsupported);
        }
        let value = element.to_float32().map_err(|_| LoadError::Unsupported)?;
        if !value.is_finite() {
            return Err(LoadError::Unsupported);
        }
        Ok(Some(value))
    };
    let first = value(PADDING)?;
    let last = value(PADDING_LIMIT)?;
    match (first, last) {
        (None, None) => Ok(None),
        (None, Some(_)) => Err(LoadError::Unsupported),
        (Some(first), last) => {
            let last = last.unwrap_or(first);
            if first > last {
                return Err(LoadError::Unsupported);
            }
            Ok(Some(Padding {
                low: first,
                high: last,
            }))
        }
    }
}

/// Source samples and validity. Padding, nonfinite values and stored values
/// outside a real-world mapping's range are missing; their placeholder value is
/// never used for display ranges, statistics or sampling.
pub(crate) fn samples(
    count: usize,
    value: impl Fn(usize) -> Option<f32>,
    padding: Option<Padding>,
    real_world: Option<RealWorld>,
    cancelled: &impl Fn() -> bool,
) -> Result<Samples, LoadError> {
    if count > 32 * 1024 * 1024 {
        return Err(LoadError::Limit);
    }
    let mut pixels = Vec::new();
    pixels
        .try_reserve_exact(count)
        .map_err(|_| LoadError::Limit)?;
    let mut mask = Vec::new();
    mask.try_reserve_exact(count)
        .map_err(|_| LoadError::Limit)?;
    let mut missing = false;
    for index in 0..count {
        if index.is_multiple_of(16384) && cancelled() {
            return Err(LoadError::Cancelled);
        }
        let stored = value(index).ok_or(LoadError::Unsupported)?;
        let sample = (stored.is_finite()
            && padding.is_none_or(|padding| !(padding.low..=padding.high).contains(&stored)))
        .then(|| match real_world {
            Some(mapping) => mapping.map(f64::from(stored)),
            None => Some(stored),
        })
        .flatten();
        missing |= sample.is_none();
        mask.push(u8::from(sample.is_some()));
        pixels.push(sample.unwrap_or(0.));
    }
    if cancelled() {
        return Err(LoadError::Cancelled);
    }
    Ok((pixels, missing.then(|| mask.into())))
}

/// One frame from encoded native bytes, in the transfer syntax byte order.
pub(crate) fn from_bytes(
    bytes: &[u8],
    big_endian: bool,
    padding: Option<Padding>,
    real_world: Option<RealWorld>,
    cancelled: &impl Fn() -> bool,
) -> Result<Samples, LoadError> {
    if !bytes.len().is_multiple_of(4) {
        return Err(LoadError::Unsupported);
    }
    samples(
        bytes.len() / 4,
        |index| {
            let word: [u8; 4] = bytes.get(index * 4..index * 4 + 4)?.try_into().ok()?;
            Some(if big_endian {
                f32::from_be_bytes(word)
            } else {
                f32::from_le_bytes(word)
            })
        },
        padding,
        real_world,
        cancelled,
    )
}

/// One frame from a fully parsed object, whose parser already applied byte order.
pub(crate) fn from_object(
    object: &DefaultDicomObject,
    frame: usize,
    count: usize,
    padding: Option<Padding>,
    real_world: Option<RealWorld>,
    cancelled: &impl Fn() -> bool,
) -> Result<Samples, LoadError> {
    let element = object.element(FLOAT).map_err(|_| LoadError::Unsupported)?;
    let values = match element.value().primitive() {
        Some(PrimitiveValue::F32(values)) if element.vr() == VR::OF => values,
        _ => return Err(LoadError::Unsupported),
    };
    let start = frame.checked_mul(count).ok_or(LoadError::Limit)?;
    let frame_values = values
        .get(start..start.checked_add(count).ok_or(LoadError::Limit)?)
        .ok_or(LoadError::Unsupported)?;
    samples(
        count,
        |index| frame_values.get(index).copied(),
        padding,
        real_world,
        cancelled,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn padding_and_nonfinite_samples_are_missing_but_extreme_finite_values_remain() {
        let values = [
            -f32::MAX,
            f32::MAX,
            -1.5,
            f32::NAN,
            f32::INFINITY,
            -0.,
            f32::MIN_POSITIVE,
            -2.,
        ];
        let padding = Some(Padding {
            low: -2.,
            high: -1.5,
        });
        let (pixels, mask) = samples(
            values.len(),
            |i| values.get(i).copied(),
            padding,
            None,
            &|| false,
        )
        .unwrap();
        assert_eq!(mask.as_deref(), Some(&[1, 1, 0, 0, 0, 1, 1, 0][..]));
        assert_eq!(pixels[0], -f32::MAX);
        assert_eq!(pixels[1], f32::MAX);
        assert_eq!(pixels[6], f32::MIN_POSITIVE);
        assert!(pixels.iter().all(|value| value.is_finite()));
        let (_, none) = samples(2, |i| [1., 2.].get(i).copied(), None, None, &|| false).unwrap();
        assert!(none.is_none());
    }

    #[test]
    fn byte_order_follows_the_transfer_syntax_and_rejects_partial_words() {
        let value = 1234.5f32;
        let (little, _) = from_bytes(&value.to_le_bytes(), false, None, None, &|| false).unwrap();
        let (big, _) = from_bytes(&value.to_be_bytes(), true, None, None, &|| false).unwrap();
        assert_eq!((little[0], big[0]), (value, value));
        assert!(from_bytes(&[0; 6], false, None, None, &|| false).is_err());
        assert!(matches!(
            from_bytes(&[0; 8], false, None, None, &|| true),
            Err(LoadError::Cancelled)
        ));
    }
}
