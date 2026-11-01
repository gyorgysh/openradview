// SPDX-License-Identifier: GPL-3.0-or-later
//! Worker-only reads of retained headers. No decoding, paths or source handles escape.
use super::*;
use crate::acquisition::{AxesError, DicomAxes, DicomAxis, DimensionValue};
use dicom_core::{DicomValue, PrimitiveValue, Tag, VR};
use dicom_object::InMemDicomObject as Object;
use std::collections::HashMap;
const CONTENT: Tag = Tag(0x0020, 0x9111);
const PER_FRAME: Tag = Tag(0x5200, 0x9230);
const SHARED: Tag = Tag(0x5200, 0x9229);
const TEMPORAL: Tag = Tag(0x0020, 0x9128);
const ECHO: Tag = Tag(0x0018, 0x9082);
const ECHO_GROUP: Tag = Tag(0x0018, 0x9114);
struct Description {
    dimensions: Vec<(Tag, Tag)>,
    organizations: Vec<Option<String>>,
    selected: [Option<usize>; 2],
    excluded: Vec<(Tag, Tag)>,
    other: bool,
}
pub(crate) struct RawAxes {
    pub scope: (String, [u8; 32]),
    pub axes: DicomAxes,
}
#[derive(Default)]
pub(crate) struct AxisReader {
    headers: HashMap<usize, Result<Option<Arc<Description>>, AxesError>>,
}
fn text(object: &Object, tag: Tag) -> Option<String> {
    let element = object.element(tag).ok()?;
    if element.header().len.get()? > 128 {
        return None;
    }
    Some(element.to_str().ok()?.trim().to_owned())
}
fn pointer(object: &Object, tag: Tag) -> Option<Tag> {
    match object.element(tag).ok()?.value() {
        DicomValue::Primitive(PrimitiveValue::Tags(values)) if values.len() == 1 => Some(values[0]),
        _ => None,
    }
}
fn description(object: &Object) -> Result<Option<Arc<Description>>, AxesError> {
    let Ok(sequence) = object.element(Tag(0x0020, 0x9222)) else {
        return Ok(None);
    };
    let items = sequence
        .value()
        .items()
        .filter(|items| !items.is_empty() && items.len() <= 16)
        .ok_or(AxesError::Incomplete)?;
    let declared = object
        .element(Tag(0x0020, 0x9221))
        .ok()
        .and_then(|e| e.value().items())
        .filter(|items| !items.is_empty() && items.len() <= 16)
        .ok_or(AxesError::Incomplete)?;
    let mut organizations = Vec::new();
    for item in declared {
        let uid = text(item, Tag(0x0020, 0x9164))
            .filter(|uid| crate::load::valid_identifier(uid))
            .ok_or(AxesError::Incomplete)?;
        if organizations.contains(&uid) {
            return Err(AxesError::Incomplete);
        }
        organizations.push(uid);
    }
    let mut result = Description {
        dimensions: Vec::new(),
        organizations: Vec::new(),
        selected: [None; 2],
        excluded: Vec::new(),
        other: false,
    };
    for item in items {
        let tag = pointer(item, Tag(0x0020, 0x9165)).ok_or(AxesError::Incomplete)?;
        let group = pointer(item, Tag(0x0020, 0x9167)).ok_or(AxesError::Incomplete)?;
        let organization = text(item, Tag(0x0020, 0x9164))
            .filter(|uid| organizations.contains(uid))
            .ok_or(AxesError::Incomplete)?;
        if result.dimensions.contains(&(tag, group)) {
            return Err(AxesError::Inconsistent);
        }
        let selected = match (tag, group) {
            (TEMPORAL, CONTENT) => Some(DicomAxis::TemporalPosition),
            (ECHO, ECHO_GROUP) => Some(DicomAxis::EchoTime),
            _ => None,
        };
        if let Some(axis) = selected {
            let index = axis.index();
            if result.selected[index].is_some() {
                return Err(AxesError::Inconsistent);
            }
            result.selected[index] = Some(result.dimensions.len());
            result.excluded.push((tag, group));
        } else if (tag, group) != (Tag(0x0020, 0x9057), CONTENT) {
            result.other = true;
        }
        result.dimensions.push((tag, group));
        result.organizations.push(Some(organization));
    }
    if result.selected == [None; 2] {
        return if result.other {
            Err(AxesError::Unsupported)
        } else {
            Ok(None)
        };
    }
    Ok(Some(Arc::new(result)))
}
fn frame_axes(
    object: &Object,
    frame: u32,
    description: &Description,
) -> Result<(DicomAxes, [u8; 32]), AxesError> {
    let shared = crate::dicom_geometry::item(object, SHARED).map_err(|_| AxesError::Incomplete)?;
    let frame = object
        .element(PER_FRAME)
        .ok()
        .and_then(|e| e.value().items())
        .and_then(|items| items.get(frame as usize))
        .ok_or(AxesError::Incomplete)?;
    let content = crate::dicom_geometry::group(shared, Some(frame), CONTENT)
        .map_err(|_| AxesError::Incomplete)?
        .ok_or(AxesError::Incomplete)?;
    let ordinals = content
        .element(Tag(0x0020, 0x9157))
        .ok()
        .and_then(|e| e.to_multi_int::<u32>().ok())
        .filter(|v| v.len() == description.dimensions.len() && v.iter().all(|v| *v > 0))
        .ok_or(AxesError::Incomplete)?;
    let mut values = [None; 2];
    for axis in DicomAxis::ALL {
        let Some(index) = description.selected[axis.index()] else {
            continue;
        };
        let (tag, group) = description.dimensions[index];
        let context = crate::dicom_geometry::group(shared, Some(frame), group)
            .map_err(|_| AxesError::Incomplete)?;
        let value = match context.and_then(|context| context.element(tag).ok()) {
            None => None,
            Some(element) if element.header().len.get() == Some(0) => None,
            Some(element) => {
                let value = match axis {
                    DicomAxis::TemporalPosition
                        if element.vr() == VR::UL && element.value().multiplicity() == 1 =>
                    {
                        f64::from(
                            element
                                .to_int::<u32>()
                                .ok()
                                .filter(|v| *v > 0)
                                .ok_or(AxesError::Incomplete)?,
                        )
                    }
                    DicomAxis::EchoTime
                        if element.vr() == VR::FD && element.value().multiplicity() == 1 =>
                    {
                        element
                            .to_float64()
                            .ok()
                            .filter(|v| v.is_finite() && *v >= 0.)
                            .ok_or(AxesError::Incomplete)?
                    }
                    _ => return Err(AxesError::Incomplete),
                };
                Some(if value == 0. { 0. } else { value })
            }
        };
        values[axis.index()] = Some(DimensionValue {
            ordinal: ordinals[index],
            value,
        });
    }
    let context = crate::dicom_geometry::acquisition_context(
        object,
        shared,
        Some(frame),
        &description.dimensions,
        &ordinals,
        &description.organizations,
        &description.excluded,
    )
    .map_err(|_| AxesError::Incomplete)?
    .ok_or(AxesError::Incomplete)?;
    Ok((
        DicomAxes {
            values,
            other_dimensions: description.other,
        },
        context,
    ))
}
impl AxisReader {
    fn description(
        &mut self,
        header: &Arc<header::Header>,
    ) -> Result<Result<Option<Arc<Description>>, AxesError>, LoadError> {
        let key = Arc::as_ptr(header) as usize;
        if let Some(result) = self.headers.get(&key) {
            return Ok(result.clone());
        }
        if self.headers.len() >= 4096 {
            return Ok(Err(AxesError::Limit));
        }
        let result = header
            .dicom()
            .map_or(Ok(None), |object| description(object));
        self.headers.try_reserve(1).map_err(|_| LoadError::Limit)?;
        self.headers.insert(key, result.clone());
        Ok(result)
    }
    pub(crate) fn read(
        &mut self,
        source: &IndexedSeries,
        cancelled: &impl Fn() -> bool,
    ) -> Result<Option<Result<RawAxes, AxesError>>, LoadError> {
        let Some(first) = source.frames.first() else {
            return Ok(None);
        };
        match self.description(&first.header)? {
            Ok(Some(_)) => {}
            Ok(None) => return Ok(None),
            Err(error) => return Ok(Some(Err(error))),
        }
        let mut result: Option<RawAxes> = None;
        for (index, entry) in source.frames.iter().enumerate() {
            if index % 256 == 0 && cancelled() {
                return Err(LoadError::Cancelled);
            }
            let descriptor = match self.description(&entry.header)? {
                Ok(Some(descriptor)) => descriptor,
                _ => return Ok(Some(Err(AxesError::Incomplete))),
            };
            let Some((axes, context)) = entry
                .header
                .dicom()
                .ok()
                .and_then(|object| frame_axes(object, entry.frame, &descriptor).ok())
            else {
                return Ok(Some(Err(AxesError::Incomplete)));
            };
            if let Some(previous) = &result {
                if previous.scope.0 != entry.header.key
                    || previous.scope.1 != context
                    || previous.axes != axes
                {
                    return Ok(Some(Err(AxesError::Inconsistent)));
                }
            } else {
                result = Some(RawAxes {
                    scope: (entry.header.key.clone(), context),
                    axes,
                });
            }
        }
        Ok(result.map(Ok))
    }
}
impl IndexedSeries {
    pub(crate) fn cine_metadata(
        &self,
        cancelled: &impl Fn() -> bool,
    ) -> Result<crate::acquisition::CineMetadata, LoadError> {
        use crate::acquisition::{CineError, CineMetadata};
        let Some(first) = self.frames.first() else {
            return Ok(CineMetadata::Absent);
        };
        if first.header.dicom().is_err() {
            return Ok(CineMetadata::Absent);
        }
        let mut present = false;
        let mut ordered = first.header.frames as usize == self.frames.len();
        for (index, entry) in self.frames.iter().enumerate() {
            if index % 1024 == 0 && cancelled() {
                return Err(LoadError::Cancelled);
            }
            if !present && (index == 0 || !Arc::ptr_eq(&entry.header, &first.header)) {
                present = entry.header.dicom().is_ok_and(|object| {
                    [
                        Tag(0x0028, 0x0009),
                        Tag(0x0018, 0x1063),
                        Tag(0x0018, 0x1065),
                        Tag(0x0018, 0x1066),
                        Tag(0x0008, 0x2142),
                        Tag(0x0008, 0x2143),
                    ]
                    .iter()
                    .any(|tag| object.element(*tag).is_ok())
                });
            }
            ordered &= Arc::ptr_eq(&entry.header, &first.header)
                && Arc::ptr_eq(&entry.source, &first.source)
                && entry.frame as usize == index;
        }
        if !present {
            return Ok(CineMetadata::Absent);
        }
        if !ordered {
            return Ok(CineMetadata::Unavailable(CineError::Order));
        }
        let Ok(object) = first.header.dicom() else {
            return Ok(CineMetadata::Absent);
        };
        crate::acquisition::read_cine(object, first.header.frames, cancelled)
    }
}
