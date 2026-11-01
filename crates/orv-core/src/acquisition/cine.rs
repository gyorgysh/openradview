// SPDX-License-Identifier: GPL-3.0-or-later
//! Declared image intervals only; no dates, audio, inferred rate or wrap interval.
use crate::load::LoadError;
use dicom_core::{PrimitiveValue, Tag, VR};
use dicom_object::InMemDicomObject as Object;
use std::{borrow::Cow, sync::Arc, time::Duration};
const TIME: Tag = Tag(0x0018, 0x1063);
const VECTOR: Tag = Tag(0x0018, 0x1065);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CineError {
    MissingPointer,
    Pointer,
    Values,
    Trim,
    Range,
    Order,
    Limit,
    Cancelled,
}
impl CineError {
    pub fn text(self) -> &'static str {
        match self {
            Self::MissingPointer => "Source timing needs a Frame Increment Pointer.",
            Self::Pointer => {
                "Source timing supports one pointer to Frame Time or Frame Time Vector."
            }
            Self::Values => {
                "Source timing has missing or invalid intervals; a vector needs one value per frame and an initial zero."
            }
            Self::Trim => "Source timing has an invalid declared frame trim.",
            Self::Range => "Source timing cannot represent these intervals safely.",
            Self::Order => {
                "Source timing is unavailable for partitioned, reordered or mixed-object frames."
            }
            Self::Limit => "Source timing metadata exceeds its supported resource limit.",
            Self::Cancelled => "Source timing check was cancelled.",
        }
    }
}
#[derive(Clone, Debug)]
pub enum CineMetadata {
    Absent,
    Unavailable(CineError),
    Available(Arc<CineTiming>),
}
enum Intervals {
    Fixed(Duration),
    Variable(Vec<Duration>),
}
pub struct CineTiming {
    count: u32,
    trim: [u32; 2],
    delay_ms: Option<f64>,
    intervals: Intervals,
}
impl std::fmt::Debug for CineTiming {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CineTiming")
            .field("frame_count", &self.count)
            .field("trim", &self.trim)
            .field("variable", &self.variable())
            .field("delay_ms", &self.delay_ms)
            .finish_non_exhaustive()
    }
}
impl CineTiming {
    pub fn frame_count(&self) -> u32 {
        self.count
    }
    /// Zero-based inclusive frame range.
    pub fn trim(&self) -> [u32; 2] {
        self.trim
    }
    pub fn delay_ms(&self) -> Option<f64> {
        self.delay_ms
    }
    pub fn variable(&self) -> bool {
        matches!(self.intervals, Intervals::Variable(_))
    }
    pub fn interval_before(&self, frame: u32) -> Option<Duration> {
        if frame == 0 || frame >= self.count {
            return None;
        }
        match &self.intervals {
            Intervals::Fixed(interval) => Some(*interval),
            Intervals::Variable(times) => times
                .get(frame as usize)?
                .checked_sub(*times.get(frame as usize - 1)?),
        }
    }
    pub fn seconds_from_first(&self, frame: u32) -> Option<f64> {
        if frame >= self.count {
            return None;
        }
        Some(
            match &self.intervals {
                Intervals::Fixed(interval) => interval.checked_mul(frame)?,
                Intervals::Variable(times) => *times.get(frame as usize)?,
            }
            .as_secs_f64(),
        )
    }
}
fn decimal_text(object: &Object, tag: Tag) -> Result<Cow<'_, str>, CineError> {
    let element = object.element(tag).map_err(|_| CineError::Values)?;
    match (element.vr(), element.value().primitive()) {
        (VR::DS, Some(PrimitiveValue::Str(_) | PrimitiveValue::Strs(_))) => {
            element.to_str().map_err(|_| CineError::Values)
        }
        (VR::UN, Some(PrimitiveValue::U8(bytes))) => {
            let text = std::str::from_utf8(bytes).map_err(|_| CineError::Values)?;
            Ok(Cow::Borrowed(text.strip_suffix('\0').unwrap_or(text)))
        }
        _ => Err(CineError::Values),
    }
}
fn decimal(text: &str) -> Result<f64, CineError> {
    let text = text.trim();
    if text.is_empty()
        || text.len() > 16
        || !text
            .bytes()
            .all(|b| b.is_ascii_digit() || matches!(b, b'+' | b'-' | b'.' | b'e' | b'E'))
    {
        return Err(CineError::Values);
    }
    text.parse::<f64>()
        .ok()
        .filter(|v| v.is_finite())
        .ok_or(CineError::Values)
}
fn one(object: &Object, tag: Tag) -> Result<f64, CineError> {
    decimal(&decimal_text(object, tag)?)
}
fn duration(ms: f64) -> Result<Duration, CineError> {
    let duration = Duration::try_from_secs_f64(ms / 1000.).map_err(|_| CineError::Range)?;
    if ms > 0. && duration.is_zero() {
        return Err(CineError::Range);
    }
    Ok(duration)
}
fn parse(
    object: &Object,
    count: u32,
    cancelled: &impl Fn() -> bool,
) -> Result<CineTiming, CineError> {
    let pointer = object
        .element(Tag(0x0028, 0x0009))
        .map_err(|_| CineError::MissingPointer)?;
    let pointer = match pointer.value().primitive() {
        Some(PrimitiveValue::Tags(tags)) if tags.len() == 1 => tags[0],
        _ => return Err(CineError::Pointer),
    };
    let intervals = match pointer {
        TIME => {
            let ms = one(object, TIME)?;
            if ms < 0. || (count > 1 && ms == 0.) {
                return Err(CineError::Values);
            }
            let interval = duration(ms)?;
            interval
                .checked_mul(count.saturating_sub(1))
                .ok_or(CineError::Range)?;
            Intervals::Fixed(interval)
        }
        VECTOR => {
            if count > 65536 {
                return Err(CineError::Limit);
            }
            let text = decimal_text(object, VECTOR)?;
            let mut times = Vec::new();
            times
                .try_reserve_exact(count as usize)
                .map_err(|_| CineError::Limit)?;
            let mut accumulated = Duration::ZERO;
            for (index, value) in text.split('\\').enumerate() {
                if index % 1024 == 0 && cancelled() {
                    return Err(CineError::Cancelled);
                }
                if index >= count as usize {
                    return Err(CineError::Values);
                }
                let ms = decimal(value)?;
                if (index == 0 && ms != 0.) || (index > 0 && ms <= 0.) {
                    return Err(CineError::Values);
                }
                accumulated = accumulated
                    .checked_add(duration(ms)?)
                    .ok_or(CineError::Range)?;
                times.push(accumulated);
            }
            if times.len() != count as usize {
                return Err(CineError::Values);
            }
            Intervals::Variable(times)
        }
        _ => return Err(CineError::Pointer),
    };
    let bound = |tag, default| -> Result<u32, CineError> {
        let value = match object.element(tag) {
            Ok(element) if element.value().multiplicity() == 1 => {
                element.to_int::<u32>().map_err(|_| CineError::Trim)?
            }
            Ok(_) => return Err(CineError::Trim),
            Err(_) => default,
        };
        if value == 0 || value > count {
            return Err(CineError::Trim);
        }
        Ok(value - 1)
    };
    let trim = [
        bound(Tag(0x0008, 0x2142), 1)?,
        bound(Tag(0x0008, 0x2143), count)?,
    ];
    if trim[0] > trim[1] {
        return Err(CineError::Trim);
    }
    let delay_ms = if object.element(Tag(0x0018, 0x1066)).is_ok() {
        Some(one(object, Tag(0x0018, 0x1066))?)
    } else {
        None
    };
    Ok(CineTiming {
        count,
        trim,
        delay_ms,
        intervals,
    })
}
pub(crate) fn read_cine(
    object: &Object,
    count: u32,
    cancelled: &impl Fn() -> bool,
) -> Result<CineMetadata, LoadError> {
    if cancelled() {
        return Err(LoadError::Cancelled);
    }
    match parse(object, count, cancelled) {
        Ok(timing) => Ok(CineMetadata::Available(Arc::new(timing))),
        Err(CineError::Cancelled) => Err(LoadError::Cancelled),
        Err(error) => Ok(CineMetadata::Unavailable(error)),
    }
}

#[cfg(test)]
mod tests;
