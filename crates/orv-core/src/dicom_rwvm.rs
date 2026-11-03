// SPDX-License-Identifier: GPL-3.0-or-later
//! The smallest real-world value mapping subset: one linear item from stored
//! values (PS3.3 C.7.6.16.2.11). Labels and explanations stay private. Alternative
//! items, lookup data and per-frame mappings are refused rather than chosen.
use crate::{
    load::LoadError,
    scalar::{RealWorld, Unit},
};
use dicom_core::{Tag, VR};
use dicom_object::{DefaultDicomObject, InMemDicomObject as Object};
use sha2::{Digest, Sha256};

pub(crate) const SEQUENCE: Tag = Tag(0x0040, 0x9096);
const SHARED: Tag = Tag(0x5200, 0x9229);
const PER_FRAME: Tag = Tag(0x5200, 0x9230);
const FIRST: Tag = Tag(0x0040, 0x9216);
const LAST: Tag = Tag(0x0040, 0x9211);
const DOUBLE_FIRST: Tag = Tag(0x0040, 0x9214);
const DOUBLE_LAST: Tag = Tag(0x0040, 0x9213);
const LUT_DATA: Tag = Tag(0x0040, 0x9212);
const INTERCEPT: Tag = Tag(0x0040, 0x9224);
const SLOPE: Tag = Tag(0x0040, 0x9225);
const UNITS: Tag = Tag(0x0040, 0x08ea);

pub(crate) struct Mapping {
    pub mapping: RealWorld,
    pub unit: Unit,
    /// Private digest of an unrecognised unit code, never its text.
    pub identity: Option<[u8; 32]>,
}

/// Integer stored range, or None for float pixel data.
pub(crate) type StoredRange = Option<(i64, i64)>;

fn single(object: &Object, tag: Tag) -> Result<Option<f64>, LoadError> {
    let Ok(element) = object.element(tag) else {
        return Ok(None);
    };
    let values = element
        .to_multi_float64()
        .map_err(|_| LoadError::Unsupported)?;
    if values.len() != 1 || !values[0].is_finite() {
        return Err(LoadError::Unsupported);
    }
    Ok(Some(values[0]))
}

fn integer(object: &Object, tag: Tag, signed: bool) -> Result<Option<f64>, LoadError> {
    let Ok(element) = object.element(tag) else {
        return Ok(None);
    };
    if !matches!(element.vr(), VR::US | VR::SS) || element.value().multiplicity() != 1 {
        return Err(LoadError::Unsupported);
    }
    let mut value = element
        .to_int::<i32>()
        .map_err(|_| LoadError::Unsupported)?;
    // Implicit VR may resolve this context-dependent tag as US.
    if signed && element.vr() == VR::US {
        value = value as u16 as i16 as i32;
    }
    Ok(Some(f64::from(value)))
}

fn text(object: &Object, tag: Tag) -> Result<String, LoadError> {
    let value = object
        .element(tag)
        .map_err(|_| LoadError::Unsupported)?
        .to_str()
        .map_err(|_| LoadError::Unsupported)?;
    let value = value.trim();
    if value.is_empty() || value.len() > 64 {
        return Err(LoadError::Unsupported);
    }
    Ok(value.to_owned())
}

fn unit(item: &Object) -> Result<(Unit, Option<[u8; 32]>), LoadError> {
    let codes = item
        .element(UNITS)
        .map_err(|_| LoadError::Unsupported)?
        .items()
        .ok_or(LoadError::Unsupported)?;
    let [code] = codes else {
        return Err(LoadError::Unsupported);
    };
    let value = text(code, Tag(0x0008, 0x0100))?;
    let scheme = text(code, Tag(0x0008, 0x0102))?;
    let known = match (scheme.as_str(), value.as_str()) {
        ("UCUM", "[hnsf'U]") => Unit::Hounsfield,
        ("UCUM", "Bq/ml" | "Bq/mL") => Unit::BecquerelsPerMillilitre,
        ("UCUM", "{counts}") => Unit::Counts,
        ("UCUM", "{counts}/s") => Unit::CountsPerSecond,
        ("UCUM", "rad") => Unit::Radians,
        _ => Unit::Unknown,
    };
    let identity = (known == Unit::Unknown).then(|| {
        let mut hash = Sha256::new();
        hash.update(b"OpenRadView real-world unit v1\0");
        for part in [&scheme, &value] {
            hash.update((part.len() as u32).to_le_bytes());
            hash.update(part.as_bytes());
        }
        hash.finalize().into()
    });
    Ok((known, identity))
}

/// `stored` is the full integer stored-code range, or None for float samples.
pub(crate) fn read(
    object: &DefaultDicomObject,
    stored: impl FnOnce() -> Result<StoredRange, LoadError>,
) -> Result<Option<Mapping>, LoadError> {
    let shared = crate::dicom_geometry::item(object, SHARED)?;
    let root = object.element(SEQUENCE).ok();
    let grouped = shared.and_then(|shared| shared.element(SEQUENCE).ok());
    if let Ok(frames) = object.element(PER_FRAME) {
        let frames = frames.items().ok_or(LoadError::Unsupported)?;
        if frames.iter().any(|frame| frame.element(SEQUENCE).is_ok()) {
            return Err(LoadError::Unsupported);
        }
    }
    let element = match (root, grouped) {
        (None, None) => return Ok(None),
        (Some(element), None) | (None, Some(element)) => element,
        (Some(_), Some(_)) => return Err(LoadError::Unsupported),
    };
    let stored = stored()?;
    let items = element.items().ok_or(LoadError::Unsupported)?;
    // Several items are alternative mappings. Choosing one would be a guess.
    let [item] = items else {
        return Err(LoadError::Unsupported);
    };
    if item.element(LUT_DATA).is_ok() {
        return Err(LoadError::Unsupported);
    }
    let slope = single(item, SLOPE)?.ok_or(LoadError::Unsupported)?;
    let intercept = single(item, INTERCEPT)?.ok_or(LoadError::Unsupported)?;
    let signed = stored.is_none_or(|(low, _)| low < 0);
    let integer_range = (integer(item, FIRST, signed)?, integer(item, LAST, signed)?);
    let double_range = (single(item, DOUBLE_FIRST)?, single(item, DOUBLE_LAST)?);
    let (first, last) = match (integer_range, double_range, stored) {
        ((Some(first), Some(last)), (None, None), _) => (first, last),
        ((None, None), (Some(first), Some(last)), None) => (first, last),
        _ => return Err(LoadError::Unsupported),
    };
    // An integer source keeps one dense value per stored code. Codes outside
    // the mapped range would have no real-world value, so require full cover.
    if let Some((low, high)) = stored
        && (first > low as f64 || last < high as f64)
    {
        return Err(LoadError::Unsupported);
    }
    let mapping = RealWorld {
        slope,
        intercept,
        first,
        last,
    };
    if !mapping.validate() {
        return Err(LoadError::Unsupported);
    }
    let (unit, identity) = unit(item)?;
    Ok(Some(Mapping {
        mapping,
        unit,
        identity,
    }))
}
