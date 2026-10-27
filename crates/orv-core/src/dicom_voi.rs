// SPDX-License-Identifier: GPL-3.0-or-later
//! Bounded grayscale choices and scalar context. Explanations and raw tags stay private.
use crate::{
    VoiFunction, WindowLevel,
    load::LoadError,
    scalar::{Rescale, ScalarData, ScalarInfo, Storage, Unit, VoiChoices, VoiLut},
};
use dicom_core::{DataElement, DicomValue, PrimitiveValue, Tag, VR};
use dicom_object::{DefaultDicomObject, InMemDicomObject as Object};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, hash_map::Entry},
    sync::Arc,
};

const SHARED: Tag = Tag(0x5200, 0x9229);
const PER_FRAME: Tag = Tag(0x5200, 0x9230);
const VOI: Tag = Tag(0x0028, 0x9132);
const TRANSFORM: Tag = Tag(0x0028, 0x9145);
const TABLES: Tag = Tag(0x0028, 0x3010);
const LIMIT: u64 = 8 * 1024 * 1024;

fn charge(bytes: &mut u64, additional: u64) -> Result<(), LoadError> {
    *bytes = bytes
        .checked_add(additional)
        .filter(|bytes| *bytes <= LIMIT)
        .ok_or(LoadError::Limit)?;
    Ok(())
}

#[derive(Default)]
pub(crate) struct Presentations {
    pub frames: Vec<Arc<ScalarData>>,
    pub metadata_bytes: u64,
}
impl Presentations {
    pub(crate) fn single(data: Arc<ScalarData>) -> Self {
        let metadata_bytes =
            (std::mem::size_of::<ScalarData>() + 32) as u64 + data.choices().byte_len();
        Self {
            frames: vec![data],
            metadata_bytes,
        }
    }
    pub fn get(&self, frame: u32) -> Option<&Arc<ScalarData>> {
        self.frames.get(if self.frames.len() == 1 {
            0
        } else {
            frame as usize
        })
    }
}

pub(crate) fn conversion(data: &ScalarData) -> dicom_pixeldata::ConvertOptions {
    let scale = data.info().rescale.unwrap_or(Rescale {
        slope: 1.,
        intercept: 0.,
    });
    dicom_pixeldata::ConvertOptions::new()
        .with_modality_lut(dicom_pixeldata::ModalityLutOption::Override(
            dicom_pixeldata::Rescale {
                slope: scale.slope,
                intercept: scale.intercept,
            },
        ))
        .with_voi_lut(dicom_pixeldata::VoiLutOption::Identity)
}
fn text(object: &Object, tag: Tag) -> Result<Option<String>, LoadError> {
    let Ok(element) = object.element(tag) else {
        return Ok(None);
    };
    let text = element.to_str().map_err(|_| LoadError::Unsupported)?;
    if text.len() > 64 {
        return Err(LoadError::Unsupported);
    }
    Ok((!text.trim().is_empty()).then(|| text.trim().to_owned()))
}
fn value(object: &Object, tag: Tag) -> Result<Option<f64>, LoadError> {
    let Some(_) = text(object, tag)? else {
        return Ok(None);
    };
    let values = object
        .element(tag)
        .map_err(|_| LoadError::Unsupported)?
        .to_multi_float64()
        .map_err(|_| LoadError::Unsupported)?;
    if values.len() != 1 || !values[0].is_finite() {
        return Err(LoadError::Unsupported);
    }
    Ok(Some(values[0]))
}
fn info(
    root: &Object,
    transform: Option<&Object>,
) -> Result<(ScalarInfo, Option<[u8; 32]>), LoadError> {
    let integer = |tag| root.element(tag).ok()?.to_int::<u16>().ok();
    let signed = match integer(Tag(0x0028, 0x0103)) {
        Some(0) => false,
        Some(1) => true,
        _ => return Err(LoadError::Unsupported),
    };
    let bits = integer(Tag(0x0028, 0x0101)).ok_or(LoadError::Unsupported)?;
    let allocated = integer(Tag(0x0028, 0x0100)).ok_or(LoadError::Unsupported)?;
    if !matches!(allocated, 1 | 8 | 16 | 32)
        || !(1..=allocated).contains(&bits)
        || integer(Tag(0x0028, 0x0102)) != Some(bits - 1)
    {
        return Err(LoadError::Unsupported);
    }
    let context = transform.unwrap_or(root);
    let slope = value(context, Tag(0x0028, 0x1053))?;
    let intercept = value(context, Tag(0x0028, 0x1052))?;
    let rescale = match (slope, intercept) {
        (None, None) if transform.is_none() => None,
        (Some(slope), Some(intercept)) => Some(Rescale { slope, intercept }),
        _ => return Err(LoadError::Unsupported),
    };
    let rescale_type = text(context, Tag(0x0028, 0x1054))?;
    let pet_units = text(root, Tag(0x0054, 0x1001))?;
    let units = |value: Option<&str>| match value {
        Some("HU") => Unit::Hounsfield,
        Some("OD") => Unit::OpticalDensity,
        Some("CNTS") => Unit::Counts,
        Some("CPS") => Unit::CountsPerSecond,
        Some("BQML") => Unit::BecquerelsPerMillilitre,
        _ => Unit::Unknown,
    };
    let a = units(rescale_type.as_deref());
    let b = units(pet_units.as_deref());
    let declared_a = rescale_type.as_deref().filter(|value| *value != "US");
    let declared_b = pet_units.as_deref().filter(|value| *value != "US");
    if declared_a.is_some()
        && declared_b.is_some()
        && (a != b || (a == Unit::Unknown && declared_a != declared_b))
    {
        return Err(LoadError::Unsupported);
    }
    let unit = if a != Unit::Unknown { a } else { b };
    let identity = if unit == Unit::Unknown
        && (rescale_type.as_deref().is_some_and(|value| value != "US") || pet_units.is_some())
    {
        let mut hash = Sha256::new();
        for code in [&rescale_type, &pet_units] {
            let code = code.as_deref().unwrap_or_default();
            hash.update((code.len() as u32).to_le_bytes());
            hash.update(code.as_bytes());
        }
        Some(hash.finalize().into())
    } else {
        None
    };
    let info = ScalarInfo {
        value_type: crate::scalar::ValueType::F32,
        storage: if signed {
            Storage::SignedInteger
        } else {
            Storage::UnsignedInteger
        },
        stored_bits: bits,
        unit,
        rescale,
        resampled: false,
    };
    if !info.validate() {
        return Err(LoadError::Unsupported);
    }
    Ok((info, identity))
}
fn signed_output(info: ScalarInfo) -> bool {
    let scale = info.rescale.unwrap_or(Rescale {
        slope: 1.,
        intercept: 0.,
    });
    let (low, high) = if info.storage == Storage::SignedInteger {
        let magnitude = 2f64.powi(i32::from(info.stored_bits) - 1);
        (-magnitude, magnitude - 1.)
    } else {
        (0., 2f64.powi(i32::from(info.stored_bits)) - 1.)
    };
    (low * scale.slope + scale.intercept).min(high * scale.slope + scale.intercept) < 0.
}
fn table_source_supported(info: ScalarInfo) -> bool {
    let scale = info.rescale.unwrap_or(Rescale {
        slope: 1.,
        intercept: 0.,
    });
    if info.stored_bits > 16 || scale.slope.fract() != 0. || scale.intercept.fract() != 0. {
        return false;
    }
    let (low, high) = if info.storage == Storage::SignedInteger {
        let magnitude = 2f64.powi(i32::from(info.stored_bits) - 1);
        (-magnitude, magnitude - 1.)
    } else {
        (0., 2f64.powi(i32::from(info.stored_bits)) - 1.)
    };
    [low, high]
        .into_iter()
        .all(|value| (value * scale.slope + scale.intercept).abs() <= 16777216.)
}
fn windows(object: &Object) -> Result<Vec<WindowLevel>, LoadError> {
    let function = match text(object, Tag(0x0028, 0x1056))?.as_deref() {
        None | Some("LINEAR") => VoiFunction::Linear,
        Some("LINEAR_EXACT") => VoiFunction::LinearExact,
        Some("SIGMOID") => VoiFunction::Sigmoid,
        _ => return Err(LoadError::Unsupported),
    };
    let numbers = |tag| -> Result<Vec<f64>, LoadError> {
        let Ok(element) = object.element(tag) else {
            return Ok(Vec::new());
        };
        if element
            .to_str()
            .ok()
            .is_some_and(|text| text.trim().is_empty())
        {
            return Ok(Vec::new());
        }
        let values = element
            .to_multi_float64()
            .map_err(|_| LoadError::Unsupported)?;
        if values.len() > 16 || values.iter().any(|v| !v.is_finite()) {
            return Err(LoadError::Unsupported);
        }
        Ok(values)
    };
    let widths = numbers(Tag(0x0028, 0x1051))?;
    let centers = numbers(Tag(0x0028, 0x1050))?;
    if widths.len() != centers.len() {
        return Err(LoadError::Unsupported);
    }
    widths
        .into_iter()
        .zip(centers)
        .map(|(width, center)| {
            let window = WindowLevel {
                width: width as f32,
                center: center as f32,
                function,
            };
            window
                .is_valid()
                .then_some(window)
                .ok_or(LoadError::Unsupported)
        })
        .collect()
}
fn table_values(
    element: &DataElement<Object>,
    count: usize,
    bits: u16,
    big: bool,
) -> Result<Arc<[u16]>, LoadError> {
    if !matches!(element.vr(), VR::US | VR::OW) {
        return Err(LoadError::Unsupported);
    }
    let raw = match element.value() {
        DicomValue::Primitive(PrimitiveValue::U16(words)) => {
            if words.len() == count && (bits == 16 || words.iter().all(|v| *v <= 255)) {
                return Ok(words.to_vec().into());
            }
            words
                .iter()
                .flat_map(|word| {
                    if big {
                        word.to_be_bytes()
                    } else {
                        word.to_le_bytes()
                    }
                })
                .collect::<Vec<_>>()
        }
        DicomValue::Primitive(PrimitiveValue::U8(bytes)) => bytes.to_vec(),
        _ => return Err(LoadError::Unsupported),
    };
    if raw.len() == count * 2 {
        let words = raw
            .as_chunks::<2>()
            .0
            .iter()
            .map(|bytes| {
                if big {
                    u16::from_be_bytes(*bytes)
                } else {
                    u16::from_le_bytes(*bytes)
                }
            })
            .collect::<Vec<_>>();
        if bits == 16 || words.iter().all(|v| *v <= 255) {
            return Ok(words.into());
        }
    }
    if bits == 8 && raw.len() == count + count % 2 && (count.is_multiple_of(2) || raw[count] == 0) {
        return Ok(raw[..count]
            .iter()
            .map(|v| u16::from(*v))
            .collect::<Vec<_>>()
            .into());
    }
    Err(LoadError::Unsupported)
}
fn luts(
    object: &Object,
    implicit: bool,
    signed: bool,
    big: bool,
    cancelled: &impl Fn() -> bool,
) -> Result<Vec<VoiLut>, LoadError> {
    let Ok(sequence) = object.element(TABLES) else {
        return Ok(Vec::new());
    };
    let items = sequence.items().ok_or(LoadError::Unsupported)?;
    if items.is_empty() || items.len() > 16 {
        return Err(LoadError::Unsupported);
    }
    let mut tables = Vec::new();
    for item in items {
        if cancelled() {
            return Err(LoadError::Cancelled);
        }
        let descriptor = item
            .element(Tag(0x0028, 0x3002))
            .map_err(|_| LoadError::Unsupported)?;
        let words = descriptor
            .to_multi_int::<i32>()
            .map_err(|_| LoadError::Unsupported)?;
        if words.len() != 3
            || !matches!(descriptor.vr(), VR::US | VR::SS)
            || words.iter().any(|v| !(-32768..=65535).contains(v))
        {
            return Err(LoadError::Unsupported);
        }
        let count = if words[0] as u16 == 0 {
            65536
        } else {
            words[0] as u16 as usize
        };
        let first = if descriptor.vr() == VR::SS || (implicit && signed) {
            words[1] as i16 as i32
        } else {
            words[1] as u16 as i32
        };
        let bits = words[2] as u16;
        if !matches!(bits, 8 | 16) {
            return Err(LoadError::Unsupported);
        }
        let values = table_values(
            item.element(Tag(0x0028, 0x3006))
                .map_err(|_| LoadError::Unsupported)?,
            count,
            bits,
            big,
        )?;
        tables.push(VoiLut::new(first, bits, values).map_err(|_| LoadError::Unsupported)?);
    }
    Ok(tables)
}

pub(crate) fn read(
    object: &DefaultDicomObject,
    frame_count: u32,
    cancelled: &impl Fn() -> bool,
) -> Result<Presentations, LoadError> {
    if !matches!(
        text(object, Tag(0x0028, 0x0004))?.as_deref(),
        Some("MONOCHROME1" | "MONOCHROME2")
    ) {
        return Ok(Presentations::default());
    }
    let shared = crate::dicom_geometry::item(object, SHARED)?;
    let per_frame = object
        .element(PER_FRAME)
        .ok()
        .map(|element| element.items().ok_or(LoadError::Unsupported))
        .transpose()?;
    if per_frame.is_some_and(|frames| frames.len() != frame_count as usize || frames.len() > 65536)
    {
        return Err(LoadError::Unsupported);
    }
    let mut contexts = Vec::new();
    for frame in 0..per_frame.map_or(1, |frames| frames.len()) {
        if cancelled() {
            return Err(LoadError::Cancelled);
        }
        let local = per_frame.and_then(|frames| frames.get(frame));
        let transform = crate::dicom_geometry::group(shared, local, TRANSFORM)?;
        let (info, unit) = info(object, transform)?;
        let voi = crate::dicom_geometry::group(shared, local, VOI)?.unwrap_or(object);
        contexts.push((info, unit, voi));
    }
    let syntax = object.meta().transfer_syntax();
    let (implicit, big) = (
        syntax == "1.2.840.10008.1.2",
        syntax == "1.2.840.10008.1.2.2",
    );
    let mut node_choices = HashMap::<(usize, bool), Arc<VoiChoices>>::new();
    let mut choice_intern = HashMap::<[u8; 32], Arc<VoiChoices>>::new();
    let mut table_intern = HashMap::<[u8; 32], VoiLut>::new();
    let mut interned = HashMap::<[u8; 32], Arc<ScalarData>>::new();
    let mut result = Presentations::default();
    for (info, unit, node) in contexts {
        if cancelled() {
            return Err(LoadError::Cancelled);
        }
        let signed = implicit && signed_output(info);
        let key = (std::ptr::from_ref(node) as usize, signed);
        let choices = if let Some(choices) = node_choices.get(&key) {
            choices.clone()
        } else {
            let mut tables = luts(node, implicit, signed, big, cancelled)?;
            for table in &mut tables {
                let entry = match table_intern.entry(table.digest()) {
                    Entry::Occupied(entry) => entry.into_mut(),
                    Entry::Vacant(entry) => {
                        charge(&mut result.metadata_bytes, table.entries().len() as u64 * 2)?;
                        entry.insert(table.clone())
                    }
                };
                *table = entry.clone();
            }
            let choices =
                VoiChoices::new(windows(node)?, tables).map_err(|_| LoadError::Unsupported)?;
            let choices = match choice_intern.entry(choices.digest()) {
                Entry::Occupied(entry) => entry.get().clone(),
                Entry::Vacant(entry) => {
                    let table_bytes = choices
                        .luts()
                        .iter()
                        .map(|table| table.entries().len() as u64 * 2)
                        .sum::<u64>();
                    charge(&mut result.metadata_bytes, choices.byte_len() - table_bytes)?;
                    entry.insert(choices).clone()
                }
            };
            node_choices.insert(key, choices.clone());
            choices
        };
        let scalar = ScalarData::with_unit_identity(info, choices, unit)
            .map_err(|_| LoadError::Unsupported)?;
        if !scalar.choices().luts().is_empty() && !table_source_supported(info) {
            return Err(LoadError::Unsupported);
        }
        let scalar = match interned.entry(scalar.digest()) {
            Entry::Occupied(entry) => entry.get().clone(),
            Entry::Vacant(entry) => {
                charge(
                    &mut result.metadata_bytes,
                    (std::mem::size_of::<ScalarData>() + 16) as u64,
                )?;
                entry.insert(scalar).clone()
            }
        };
        charge(
            &mut result.metadata_bytes,
            std::mem::size_of::<Arc<ScalarData>>() as u64,
        )?;
        result.frames.push(scalar);
    }
    Ok(result)
}

#[cfg(test)]
mod tests;
