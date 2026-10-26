// SPDX-License-Identifier: GPL-3.0-or-later
//! CharLS decoded buffer layout, independent of DICOM's mandatory zero Planar tag.
use crate::load::LoadError;
use dicom_object::DefaultDicomObject;

#[derive(Clone, Copy)]
pub(super) enum Layout {
    Component,
    Sample,
}

pub(super) fn layout(
    object: &DefaultDicomObject,
    cancelled: impl Fn() -> bool,
) -> Result<Option<Layout>, LoadError> {
    if !matches!(
        object.meta().transfer_syntax(),
        "1.2.840.10008.1.2.4.80" | "1.2.840.10008.1.2.4.81"
    ) {
        return Ok(None);
    }
    // The caller has validated/assembled one bounded frame before decoding.
    let fragments = object
        .element_by_name("PixelData")
        .map_err(|_| LoadError::Unsupported)?
        .value()
        .fragments()
        .ok_or(LoadError::Unsupported)?;
    let mut bytes = fragments.iter().flatten().copied();
    let invalid = || LoadError::Unsupported;
    if bytes.next() != Some(0xff) || bytes.next() != Some(0xd8) {
        return Err(invalid());
    }
    loop {
        if cancelled() {
            return Err(LoadError::Cancelled);
        }
        if bytes.next() != Some(0xff) {
            return Err(invalid());
        }
        let mut marker = bytes.next().ok_or_else(invalid)?;
        while marker == 0xff {
            marker = bytes.next().ok_or_else(invalid)?;
        }
        if matches!(marker, 0xd0..=0xd9) {
            return Err(invalid());
        }
        let length = u16::from_be_bytes([
            bytes.next().ok_or_else(invalid)?,
            bytes.next().ok_or_else(invalid)?,
        ]);
        if length < 2 {
            return Err(invalid());
        }
        if marker == 0xda {
            let components = bytes.next().ok_or_else(invalid)?;
            if !matches!(components, 1 | 3) || length != 2 * u16::from(components) + 6 {
                return Err(invalid());
            }
            for _ in 0..components * 2 {
                bytes.next().ok_or_else(invalid)?;
            }
            bytes.next().ok_or_else(invalid)?; // NEAR: native decoder validates it.
            let interleave = bytes.next().ok_or_else(invalid)?;
            bytes.next().ok_or_else(invalid)?; // Point transform, validated natively.
            return match (components, interleave) {
                (1, 0) => Ok(Some(Layout::Component)),
                (3, 1 | 2) => Ok(Some(Layout::Sample)),
                _ => Err(invalid()),
            };
        }
        for _ in 0..length - 2 {
            bytes.next().ok_or_else(invalid)?;
        }
    }
}
