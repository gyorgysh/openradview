// SPDX-License-Identifier: GPL-3.0-or-later
//! Explicit patient geometry and acquisition partitions; no inferred frame positions.
use crate::{load::LoadError, study::Geometry};
use dicom_core::{DicomValue, PrimitiveValue, Tag};
use dicom_object::InMemDicomObject as Object;
use sha2::{Digest, Sha256};

pub(crate) struct FrameSpatial {
    pub geometry: Option<Geometry>,
    pub acquisition: Option<[u8; 32]>,
}
pub(crate) struct Spatial {
    pub geometry: Option<Geometry>,
    pub acquisition: Option<[u8; 32]>,
    pub frames: Vec<FrameSpatial>,
}
impl Spatial {
    pub fn geometry_for(&self, frame: usize) -> Option<&Geometry> {
        if self.frames.is_empty() {
            self.geometry.as_ref()
        } else {
            self.frames.get(frame)?.geometry.as_ref()
        }
    }
    pub fn uniform_acquisition(&self) -> Option<[u8; 32]> {
        if self.frames.is_empty() {
            return self.acquisition;
        }
        let first = self.frames.first()?.acquisition;
        self.frames
            .iter()
            .all(|frame| frame.acquisition == first)
            .then_some(first)
            .flatten()
    }
    pub fn uniform(&self) -> bool {
        self.frames.first().is_none_or(|first| {
            self.frames
                .iter()
                .all(|frame| frame.acquisition == first.acquisition)
        })
    }
}
const SHARED: Tag = Tag(0x5200, 0x9229);
const PER_FRAME: Tag = Tag(0x5200, 0x9230);
const CONTENT: Tag = Tag(0x0020, 0x9111);
const IN_STACK: Tag = Tag(0x0020, 0x9057);
const FRAME_TYPES: [Tag; 3] = [
    Tag(0x0018, 0x9329),
    Tag(0x0018, 0x9226),
    Tag(0x0018, 0x9751),
];
const MAX_FRAMES: usize = 65536;

fn text(object: &Object, tag: Tag) -> String {
    object
        .element(tag)
        .ok()
        .and_then(|value| value.to_str().ok())
        .map(|s| s.trim().to_owned())
        .unwrap_or_default()
}
fn numbers<const N: usize>(object: &Object, tag: Tag) -> Option<[f64; N]> {
    let values: [f64; N] = object
        .element(tag)
        .ok()?
        .to_multi_float64()
        .ok()?
        .try_into()
        .ok()?;
    values
        .iter()
        .all(|value| value.is_finite())
        .then_some(values)
}
pub(crate) fn item(object: &Object, tag: Tag) -> Result<Option<&Object>, LoadError> {
    let Ok(element) = object.element(tag) else {
        return Ok(None);
    };
    let items = element.value().items().ok_or(LoadError::Unsupported)?;
    if items.len() != 1 {
        return Err(LoadError::Unsupported);
    }
    Ok(items.first())
}
pub(crate) fn group<'a>(
    shared: Option<&'a Object>,
    frame: Option<&'a Object>,
    tag: Tag,
) -> Result<Option<&'a Object>, LoadError> {
    let a = shared
        .map(|object| item(object, tag))
        .transpose()?
        .flatten();
    let b = frame.map(|object| item(object, tag)).transpose()?.flatten();
    if a.is_some() && b.is_some() {
        return Err(LoadError::Unsupported);
    }
    Ok(b.or(a))
}
fn tag_value(object: &Object, tag: Tag) -> Option<Tag> {
    match object.element(tag).ok()?.value() {
        DicomValue::Primitive(PrimitiveValue::Tags(values)) if values.len() == 1 => Some(values[0]),
        _ => None,
    }
}
fn geometry(
    position: &Object,
    orientation: &Object,
    measures: &Object,
    reference: Option<String>,
) -> Option<Geometry> {
    let geometry = Geometry {
        nifti_transform: None,
        frame_of_reference: reference,
        position: numbers(position, Tag(0x0020, 0x0032))?,
        orientation: numbers(orientation, Tag(0x0020, 0x0037))?,
        spacing: numbers(measures, Tag(0x0028, 0x0030))?,
    };
    geometry.validate().is_ok().then_some(geometry)
}
fn reference(object: &Object) -> Option<String> {
    let value = text(object, Tag(0x0020, 0x0052));
    crate::load::valid_identifier(&value).then_some(value)
}

fn frame_type<'a>(
    shared: Option<&'a Object>,
    frame: Option<&'a Object>,
) -> Result<Option<&'a Object>, LoadError> {
    let mut result = None;
    for tag in FRAME_TYPES {
        if let Some(context) = group(shared, frame, tag)? {
            if result.is_some() {
                return Err(LoadError::Unsupported);
            }
            result = Some(context);
        }
    }
    Ok(result)
}

fn spatial_allowed(object: &Object, description: Option<&Object>) -> bool {
    if !matches!(text(object, Tag(0x0010, 0x2210)).as_str(), "" | "BIPED") {
        return false;
    }
    for tag in [Tag(0x0008, 0x9206), Tag(0x0008, 0x9208)] {
        let root = text(object, tag);
        let local = description
            .map(|context| text(context, tag))
            .unwrap_or_default();
        let value = if local.is_empty() { &root } else { &local };
        if value == "MIXED"
            || (tag == Tag(0x0008, 0x9206) && !matches!(value.as_str(), "" | "VOLUME"))
        {
            return false;
        }
    }
    let image_type = description
        .map(|context| text(context, Tag(0x0008, 0x9007)))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| text(object, Tag(0x0008, 0x0008)));
    !image_type.split('\\').any(|value| value == "MIXED")
}

fn supported_mapping(context: &Object) -> Result<(), LoadError> {
    if context.element(Tag(0x0028, 0x3000)).is_ok() || context.element(Tag(0x0040, 0x9096)).is_ok()
    {
        return Err(LoadError::Unsupported);
    }
    Ok(())
}

// Numeric acquisition context is separated from patient names, free text and UIDs.
fn acquisition(
    object: &Object,
    shared: Option<&Object>,
    frame: Option<&Object>,
    dimensions: &[(Tag, Tag)],
    values: &[u32],
) -> Result<Option<[u8; 32]>, LoadError> {
    let mut hash = Sha256::new();
    let mut count = 0u32;
    for ((tag, group), value) in dimensions.iter().zip(values) {
        if *tag == IN_STACK && *group == CONTENT {
            continue;
        }
        hash.update(tag.0.to_le_bytes());
        hash.update(tag.1.to_le_bytes());
        hash.update(group.0.to_le_bytes());
        hash.update(group.1.to_le_bytes());
        hash.update(value.to_le_bytes());
        count += 1;
    }
    for tag in [
        Tag(0x0020, 0x0100),
        Tag(0x0018, 0x0086),
        Tag(0x0018, 0x0081),
        Tag(0x0018, 0x0080),
        Tag(0x0018, 0x9087),
        Tag(0x0018, 0x9089),
        Tag(0x0018, 0x1060),
        Tag(0x0020, 0x0012),
    ] {
        if let Ok(element) = object.element(tag) {
            if element
                .to_str()
                .ok()
                .is_some_and(|value| value.trim().is_empty())
            {
                continue;
            }
            let numbers = element
                .to_multi_float64()
                .map_err(|_| LoadError::Unsupported)?;
            if numbers.is_empty()
                || numbers.len() > 16
                || numbers.iter().any(|value| !value.is_finite())
            {
                return Err(LoadError::Unsupported);
            }
            hash.update(tag.0.to_le_bytes());
            hash.update(tag.1.to_le_bytes());
            hash.update((numbers.len() as u32).to_le_bytes());
            for value in numbers {
                hash.update((if value == 0. { 0. } else { value }).to_le_bytes());
            }
            count += 1;
        }
    }
    for (sequence, tags) in [
        (Tag(0x0020, 0x9111), &[Tag(0x0020, 0x9128)][..]),
        (Tag(0x0018, 0x9114), &[Tag(0x0018, 0x9082)][..]),
        (Tag(0x0018, 0x9117), &[Tag(0x0018, 0x9087)][..]),
    ] {
        if let Some(context) = group(shared, frame, sequence)? {
            for tag in tags {
                if context.element(*tag).is_ok() {
                    let value = numbers::<1>(context, *tag).ok_or(LoadError::Unsupported)?[0];
                    hash.update(tag.0.to_le_bytes());
                    hash.update(tag.1.to_le_bytes());
                    hash.update((if value == 0. { 0. } else { value }).to_le_bytes());
                    count += 1;
                }
            }
            if sequence == Tag(0x0018, 0x9117)
                && let Some(direction) = item(context, Tag(0x0018, 0x9076))?
            {
                let direction =
                    numbers::<3>(direction, Tag(0x0018, 0x9089)).ok_or(LoadError::Unsupported)?;
                hash.update(b"diffusion direction");
                for value in direction {
                    hash.update((if value == 0. { 0. } else { value }).to_le_bytes());
                }
                count += 1;
            }
        }
    }
    let description = frame_type(shared, frame)?;
    for (context, tags) in [
        (
            Some(object),
            &[
                Tag(0x0008, 0x0008),
                Tag(0x0008, 0x9208),
                Tag(0x0008, 0x9209),
            ][..],
        ),
        (
            description,
            &[
                Tag(0x0008, 0x9007),
                Tag(0x0008, 0x9208),
                Tag(0x0008, 0x9209),
            ][..],
        ),
    ] {
        let Some(context) = context else { continue };
        for tag in tags {
            let value = text(context, *tag);
            if value.is_empty() {
                continue;
            }
            if value.len() > 256
                || !value.bytes().all(|byte| {
                    byte.is_ascii_uppercase()
                        || byte.is_ascii_digit()
                        || matches!(byte, b'_' | b' ' | b'\\')
                })
            {
                return Err(LoadError::Unsupported);
            }
            hash.update(tag.0.to_le_bytes());
            hash.update(tag.1.to_le_bytes());
            hash.update((value.len() as u32).to_le_bytes());
            hash.update(value.as_bytes());
            count += 1;
        }
    }
    Ok((count > 0).then(|| hash.finalize().into()))
}

pub(crate) fn read(
    object: &Object,
    frames: u32,
    cancelled: &impl Fn() -> bool,
) -> Result<Spatial, LoadError> {
    let shared = item(object, SHARED)?;
    supported_mapping(object)?;
    if let Some(shared) = shared {
        supported_mapping(shared)?;
        if let Some(transform) = item(shared, Tag(0x0028, 0x9145))? {
            supported_mapping(transform)?;
        }
    }
    let per_frame = object
        .element(PER_FRAME)
        .ok()
        .map(|value| value.value().items().ok_or(LoadError::Unsupported))
        .transpose()?;
    if per_frame.is_some_and(|items| items.len() != frames as usize) {
        return Err(LoadError::Unsupported);
    }
    if shared.is_some_and(|shared| shared.element(CONTENT).is_ok()) {
        return Err(LoadError::Unsupported);
    }
    let base_acquisition = acquisition(object, shared, None, &[], &[])?;
    let allowed = spatial_allowed(object, frame_type(shared, None)?);
    let classic = (frames == 1 && shared.is_none() && per_frame.is_none() && allowed)
        .then(|| geometry(object, object, object, reference(object)))
        .flatten();
    let mut result = Spatial {
        geometry: classic,
        acquisition: base_acquisition,
        frames: Vec::new(),
    };
    let Some(per_frame) = per_frame else {
        return Ok(result);
    };
    if per_frame.len() > MAX_FRAMES {
        return Err(LoadError::Limit);
    }
    let enhanced = matches!(
        text(object, Tag(0x0008, 0x0016)).as_str(),
        "1.2.840.10008.5.1.4.1.1.2.1"
            | "1.2.840.10008.5.1.4.1.1.4.1"
            | "1.2.840.10008.5.1.4.1.1.4.3"
            | "1.2.840.10008.5.1.4.1.1.130"
    );
    let mut dimensions = Vec::new();
    if let Ok(sequence) = object.element(Tag(0x0020, 0x9222)) {
        let items = sequence.value().items().ok_or(LoadError::Unsupported)?;
        if items.is_empty() || items.len() > 16 {
            return Err(LoadError::Unsupported);
        }
        for item in items {
            let pointer = tag_value(item, Tag(0x0020, 0x9165)).ok_or(LoadError::Unsupported)?;
            let group = tag_value(item, Tag(0x0020, 0x9167)).ok_or(LoadError::Unsupported)?;
            if dimensions.contains(&(pointer, group)) {
                return Err(LoadError::Unsupported);
            }
            dimensions.push((pointer, group));
        }
    }
    for frame in per_frame {
        if cancelled() {
            return Err(LoadError::Cancelled);
        }
        let position = group(shared, Some(frame), Tag(0x0020, 0x9113))?;
        let orientation = group(shared, Some(frame), Tag(0x0020, 0x9116))?;
        let measures = group(shared, Some(frame), Tag(0x0028, 0x9110))?;
        // Contradictory scaling must not be accepted even for source-only browsing.
        let transform = group(shared, Some(frame), Tag(0x0028, 0x9145))?;
        for context in [shared, Some(frame), transform].into_iter().flatten() {
            supported_mapping(context)?;
        }
        let content = group(shared, Some(frame), CONTENT)?;
        let values = content
            .and_then(|content| content.element(Tag(0x0020, 0x9157)).ok())
            .map(|value| {
                value
                    .to_multi_int::<u32>()
                    .map_err(|_| LoadError::Unsupported)
            })
            .transpose()?
            .unwrap_or_default();
        let complete = !dimensions.is_empty()
            && values.len() == dimensions.len()
            && values.iter().all(|value| *value > 0)
            && content.is_none_or(|content| {
                content.element(Tag(0x0020, 0x9056)).is_err()
                    || dimensions.contains(&(Tag(0x0020, 0x9056), CONTENT))
            })
            && dimensions
                .iter()
                .zip(&values)
                .any(|((tag, macro_tag), value)| {
                    *tag == IN_STACK
                        && *macro_tag == CONTENT
                        && content
                            .and_then(|item| item.element(IN_STACK).ok()?.to_int::<u32>().ok())
                            == Some(*value)
                });
        let frame_acquisition = acquisition(object, shared, Some(frame), &dimensions, &values)?;
        let geometry =
            if enhanced && spatial_allowed(object, frame_type(shared, Some(frame))?) && complete {
                position.zip(orientation).zip(measures).and_then(
                    |((position, orientation), measures)| {
                        geometry(position, orientation, measures, reference(object))
                    },
                )
            } else {
                None
            };
        result.frames.push(FrameSpatial {
            geometry,
            acquisition: frame_acquisition,
        });
    }
    Ok(result)
}
