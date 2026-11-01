// SPDX-License-Identifier: GPL-3.0-or-later
//! Explicit local BINARY masks; source references remain private to this module.
use super::{IndexedSeries, header, source::Stamp};
use crate::{
    dicom_geometry::{group, item},
    import_report::SkipReason,
    load::LoadError,
    resources::MemoryBudget,
    segments::{MAX_FRAMES, MAX_INPUT_BYTES, MAX_SEGMENTS, Segments},
    study::{Geometry, Series},
};
use dicom_core::Tag;
use dicom_object::InMemDicomObject as Object;
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{BufReader, Read},
    path::Path,
    sync::Arc,
};
const CLASS: &str = "1.2.840.10008.5.1.4.1.1.66.4";
const SHARED: Tag = Tag(0x5200, 0x9229);
const PER_FRAME: Tag = Tag(0x5200, 0x9230);
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegmentationError {
    Cancelled,
    Changed,
    Io,
    Kind,
    Encoding,
    Metadata,
    Association,
    Geometry,
    Preparation,
    Limit,
    Mask(&'static str),
}
impl std::fmt::Display for SegmentationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Cancelled=>"Mask import cancelled.", Self::Changed=>"A selected source changed. Close and reopen it before loading masks.",
            Self::Io=>"Could not read the selected local mask file.",Self::Kind=>"Choose a BINARY DICOM segmentation object. Other segmentation kinds are not supported.",
            Self::Encoding=>"Supplied masks currently require native, uncompressed little-endian one-bit pixels.",
            Self::Metadata=>"The mask metadata is incomplete, contradictory or outside the supported subset.",
            Self::Association=>"Mask source references do not uniquely match the selected loaded DICOM series.",
            Self::Geometry=>"Mask rows, columns, reference and plane geometry must match their referenced source images. Resampling is not performed.",
            Self::Preparation=>"The selected source could not be prepared as a regular volume. Check Image info for its geometry and size.",
            Self::Limit=>"The mask input exceeds its metadata, frame, grid or memory allowance.",Self::Mask(message)=>message,
        })
    }
}
impl std::error::Error for SegmentationError {}
impl From<LoadError> for SegmentationError {
    fn from(error: LoadError) -> Self {
        match error {
            LoadError::Cancelled => Self::Cancelled,
            LoadError::Changed => Self::Changed,
            LoadError::Limit => Self::Limit,
            LoadError::Io => Self::Io,
            _ => Self::Metadata,
        }
    }
}
impl From<crate::spatial::GeometryError> for SegmentationError {
    fn from(error: crate::spatial::GeometryError) -> Self {
        match error {
            crate::spatial::GeometryError::Cancelled => Self::Cancelled,
            crate::spatial::GeometryError::Limit => Self::Limit,
            _ => Self::Geometry,
        }
    }
}
type Result<T> = std::result::Result<T, SegmentationError>;
fn text(object: &Object, tag: Tag) -> Result<String> {
    object
        .element(tag)
        .ok()
        .and_then(|e| e.to_str().ok())
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .ok_or(SegmentationError::Metadata)
}
fn uid(object: &Object, tag: Tag) -> Result<String> {
    let value = text(object, tag)?;
    if !crate::load::valid_identifier(&value) {
        return Err(SegmentationError::Metadata);
    };
    Ok(value)
}
fn one_u32(object: &Object, tag: Tag) -> Result<u32> {
    let values = object
        .element(tag)
        .map_err(|_| SegmentationError::Metadata)?
        .to_multi_int::<u32>()
        .map_err(|_| SegmentationError::Metadata)?;
    if values.len() != 1 {
        return Err(SegmentationError::Metadata);
    };
    Ok(values[0])
}
fn numbers<const N: usize>(object: &Object, tag: Tag) -> Result<[f64; N]> {
    let values: [f64; N] = object
        .element(tag)
        .map_err(|_| SegmentationError::Metadata)?
        .to_multi_float64()
        .map_err(|_| SegmentationError::Metadata)?
        .try_into()
        .map_err(|_| SegmentationError::Metadata)?;
    if values.iter().any(|v| !v.is_finite()) {
        return Err(SegmentationError::Metadata);
    };
    Ok(values)
}
fn sequence(object: &Object, tag: Tag) -> Result<&[Object]> {
    object
        .element(tag)
        .map_err(|_| SegmentationError::Metadata)?
        .value()
        .items()
        .ok_or(SegmentationError::Metadata)
}
#[derive(Clone)]
struct SourcePlane {
    original: usize,
    geometry: Geometry,
    size: [u32; 2],
}
type SourceKey = (String, String, u32);
struct References {
    planes: HashMap<SourceKey, SourcePlane>,
    single: HashSet<(String, String)>,
}
fn source_references(source: &IndexedSeries, cancel: &impl Fn() -> bool) -> Result<References> {
    if source.frames.is_empty() || source.frames.len() > MAX_FRAMES {
        return Err(SegmentationError::Limit);
    };
    let mut planes = HashMap::new();
    planes
        .try_reserve(source.frames.len())
        .map_err(|_| SegmentationError::Limit)?;
    let mut single = HashSet::new();
    single
        .try_reserve(source.frames.len())
        .map_err(|_| SegmentationError::Limit)?;
    let mut checked = HashSet::new();
    checked
        .try_reserve(source.frames.len())
        .map_err(|_| SegmentationError::Limit)?;
    for (original, entry) in source.frames.iter().enumerate() {
        if cancel() {
            return Err(SegmentationError::Cancelled);
        };
        if checked.insert(entry.source.id) {
            entry.source.check()?;
        }
        let object = entry
            .header
            .dicom()
            .map_err(|_| SegmentationError::Association)?;
        let class = uid(object, Tag(8, 0x16)).map_err(|_| SegmentationError::Association)?;
        let instance = uid(object, Tag(8, 0x18)).map_err(|_| SegmentationError::Association)?;
        if class != object.meta().media_storage_sop_class_uid()
            || instance != object.meta().media_storage_sop_instance_uid()
        {
            return Err(SegmentationError::Association);
        };
        let geometry = entry
            .header
            .geometry_for(entry.frame)
            .cloned()
            .ok_or(SegmentationError::Geometry)?;
        if entry.header.frames == 1 {
            single.insert((class.clone(), instance.clone()));
        }
        let key = (class, instance, entry.frame + 1);
        if planes
            .insert(
                key,
                SourcePlane {
                    original,
                    geometry,
                    size: [entry.header.width, entry.header.height],
                },
            )
            .is_some()
        {
            return Err(SegmentationError::Association);
        };
    }
    Ok(References { planes, single })
}
fn verify_sources(source: &IndexedSeries, cancel: &impl Fn() -> bool) -> Result<()> {
    let mut seen = HashSet::new();
    seen.try_reserve(source.frames.len())
        .map_err(|_| SegmentationError::Limit)?;
    for frame in &source.frames {
        if cancel() {
            return Err(SegmentationError::Cancelled);
        }
        if seen.insert(frame.source.id) {
            frame.source.check()?;
        }
    }
    Ok(())
}
fn referenced_plane<'a>(
    derivation: &Object,
    references: &'a References,
) -> Result<&'a SourcePlane> {
    let sources =
        sequence(derivation, Tag(8, 0x2112)).map_err(|_| SegmentationError::Association)?;
    if sources.len() != 1 {
        return Err(SegmentationError::Association);
    };
    let source = &sources[0];
    let class = uid(source, Tag(8, 0x1150)).map_err(|_| SegmentationError::Association)?;
    let instance = uid(source, Tag(8, 0x1155)).map_err(|_| SegmentationError::Association)?;
    let frame = if source.element(Tag(8, 0x1160)).is_ok() {
        one_u32(source, Tag(8, 0x1160)).map_err(|_| SegmentationError::Association)?
    } else if references
        .single
        .contains(&(class.clone(), instance.clone()))
    {
        1
    } else {
        return Err(SegmentationError::Association);
    };
    if source.element(Tag(0x28, 0x135a)).is_ok() && text(source, Tag(0x28, 0x135a))? != "YES" {
        return Err(SegmentationError::Geometry);
    };
    references
        .planes
        .get(&(class, instance, frame))
        .ok_or(SegmentationError::Association)
}
fn matches_plane(
    shared: &Object,
    frame: Option<&Object>,
    root: &Object,
    reference: &SourcePlane,
    size: [u32; 2],
) -> Result<()> {
    let position =
        group(Some(shared), frame, Tag(0x20, 0x9113))?.ok_or(SegmentationError::Geometry)?;
    let orientation =
        group(Some(shared), frame, Tag(0x20, 0x9116))?.ok_or(SegmentationError::Geometry)?;
    let measures =
        group(Some(shared), frame, Tag(0x28, 0x9110))?.ok_or(SegmentationError::Geometry)?;
    let geometry = Geometry {
        position: numbers(position, Tag(0x20, 0x32))?,
        orientation: numbers(orientation, Tag(0x20, 0x37))?,
        spacing: numbers(measures, Tag(0x28, 0x30))?,
        frame_of_reference: Some(uid(root, Tag(0x20, 0x52))?),
        nifti_transform: None,
    };
    geometry
        .validate()
        .map_err(|_| SegmentationError::Geometry)?;
    let source = &reference.geometry;
    if size != reference.size
        || geometry.frame_of_reference != source.frame_of_reference
        || geometry
            .position
            .iter()
            .zip(source.position)
            .any(|(a, b)| (*a - b).abs() > 1e-5)
        || geometry
            .orientation
            .iter()
            .zip(source.orientation)
            .any(|(a, b)| (*a - b).abs() > 1e-5)
        || geometry
            .spacing
            .iter()
            .zip(source.spacing)
            .any(|(a, b)| (*a - b).abs() > 1e-5)
    {
        return Err(SegmentationError::Geometry);
    };
    Ok(())
}
/// Worker-only selected-file import. It follows source SOP/frame references only
/// within the selected indexed series; no referenced paths or identifiers leave core.
pub fn read_binary_segmentation(
    path: &Path,
    series: &Series,
    cancelled: impl Fn() -> bool,
) -> Result<Segments> {
    let result = read_inner(path, series, &cancelled);
    if cancelled() {
        Err(SegmentationError::Cancelled)
    } else {
        result
    }
}
fn read_inner(path: &Path, series: &Series, cancel: &impl Fn() -> bool) -> Result<Segments> {
    if cancel() {
        return Err(SegmentationError::Cancelled);
    };
    crate::resources::refresh_memory_policy();
    let source = series
        .source
        .as_ref()
        .ok_or(SegmentationError::Association)?;
    if source.is_empty() || source.len() > MAX_FRAMES {
        return Err(SegmentationError::Limit);
    }
    let before = fs::symlink_metadata(path).map_err(|_| SegmentationError::Io)?;
    if !before.is_file() || before.file_type().is_symlink() {
        return Err(SegmentationError::Io);
    };
    if before.len() > MAX_INPUT_BYTES as u64 + 8 * 1024 * 1024 {
        return Err(SegmentationError::Limit);
    };
    let stamp = Stamp::new(&before);
    let file = fs::File::open(path).map_err(|_| SegmentationError::Io)?;
    if Stamp::new(&file.metadata().map_err(|_| SegmentationError::Io)?) != stamp {
        return Err(SegmentationError::Changed);
    };
    // Prefix buffer, parsed object, temporary reference maps and conversion headroom.
    let _scratch = MemoryBudget::volumes()
        .reserve(before.len().min(8 * 1024 * 1024) * 12 + 8 * 1024 * 1024)
        .ok_or(SegmentationError::Limit)?;
    let mut reader = BufReader::new(file);
    let mut reason = SkipReason::UnrecognizedInput;
    let prefix = header::read_prefix_with_syntax(
        &mut reader,
        cancel,
        &mut reason,
        |class| class == CLASS,
        |syntax| matches!(syntax, "1.2.840.10008.1.2" | "1.2.840.10008.1.2.1"),
    )
    .map_err(|error| match reason {
        SkipReason::UnsupportedObject | SkipReason::UnrecognizedInput => SegmentationError::Kind,
        SkipReason::UnsupportedEncoding => SegmentationError::Encoding,
        _ => error.into(),
    })?;
    if !matches!(
        prefix.syntax.as_str(),
        "1.2.840.10008.1.2" | "1.2.840.10008.1.2.1"
    ) || prefix.deflated_at.is_some()
    {
        return Err(SegmentationError::Encoding);
    };
    let root: &Object = &prefix.object;
    if uid(root, Tag(8, 0x16))? != CLASS
        || uid(root, Tag(8, 0x18))? != prefix.object.meta().media_storage_sop_instance_uid()
    {
        return Err(SegmentationError::Metadata);
    };
    if text(root, Tag(0x62, 1))? != "BINARY" {
        return Err(SegmentationError::Kind);
    };
    if one_u32(root, Tag(0x28, 2))? != 1
        || text(root, Tag(0x28, 4))? != "MONOCHROME2"
        || one_u32(root, Tag(0x28, 0x100))? != 1
        || one_u32(root, Tag(0x28, 0x101))? != 1
        || one_u32(root, Tag(0x28, 0x102))? != 0
        || one_u32(root, Tag(0x28, 0x103))? != 0
    {
        return Err(SegmentationError::Encoding);
    };
    if root.element(Tag(0x20, 0x9161)).is_ok() {
        return Err(SegmentationError::Metadata);
    };
    let frames = one_u32(root, Tag(0x28, 8))? as usize;
    let size = [
        one_u32(root, Tag(0x28, 0x11))?,
        one_u32(root, Tag(0x28, 0x10))?,
    ];
    if frames == 0 || frames > MAX_FRAMES {
        return Err(SegmentationError::Limit);
    };
    let segment_sequence = sequence(root, Tag(0x62, 2))?;
    if segment_sequence.is_empty() || segment_sequence.len() > MAX_SEGMENTS {
        return Err(SegmentationError::Limit);
    };
    let mut numbers = Vec::new();
    numbers
        .try_reserve_exact(segment_sequence.len())
        .map_err(|_| SegmentationError::Limit)?;
    for segment in segment_sequence {
        let number = u16::try_from(one_u32(segment, Tag(0x62, 4))?)
            .map_err(|_| SegmentationError::Metadata)?;
        if number == 0 || numbers.contains(&number) {
            return Err(SegmentationError::Metadata);
        };
        numbers.push(number);
    }
    let layout = crate::inspection::series_info(series, cancel)?.geometry?;
    Segments::check_layout(layout.dimensions, numbers.len()).map_err(SegmentationError::Mask)?;
    let expected = (size[0] as usize)
        .checked_mul(size[1] as usize)
        .and_then(|n| n.checked_mul(frames))
        .ok_or(SegmentationError::Limit)?
        .div_ceil(8)
        .next_multiple_of(2);
    if expected > MAX_INPUT_BYTES {
        return Err(SegmentationError::Limit);
    }
    if prefix.pixel_length != Some(expected as u64)
        || prefix.pixel_offset.checked_add(expected as u64) != Some(before.len())
    {
        return Err(SegmentationError::Encoding);
    };
    let references = source_references(source, cancel)?;
    let shared = item(root, SHARED)?.ok_or(SegmentationError::Metadata)?;
    let per_frame = if root.element(PER_FRAME).is_ok() {
        let values = sequence(root, PER_FRAME)?;
        if values.len() != frames {
            return Err(SegmentationError::Metadata);
        };
        Some(values)
    } else {
        None
    };
    let mut associated = Vec::new();
    associated
        .try_reserve_exact(frames)
        .map_err(|_| SegmentationError::Limit)?;
    let mut seen = HashSet::new();
    seen.try_reserve(frames)
        .map_err(|_| SegmentationError::Limit)?;
    for i in 0..frames {
        if cancel() {
            return Err(SegmentationError::Cancelled);
        };
        let frame = per_frame.map(|values| &values[i]);
        let segment =
            group(Some(shared), frame, Tag(0x62, 0xa))?.ok_or(SegmentationError::Metadata)?;
        let number = u16::try_from(one_u32(segment, Tag(0x62, 0xb))?)
            .map_err(|_| SegmentationError::Metadata)?;
        if !numbers.contains(&number) {
            return Err(SegmentationError::Metadata);
        };
        let derivation =
            group(Some(shared), frame, Tag(8, 0x9124))?.ok_or(SegmentationError::Association)?;
        let reference = referenced_plane(derivation, &references)?;
        matches_plane(shared, frame, root, reference, size)?;
        if !seen.insert((number, reference.original)) {
            return Err(SegmentationError::Metadata);
        };
        associated.push((number, reference.original));
    }
    let volume = match source.volume(cancel) {
        Ok(volume) => volume,
        Err(_) => {
            verify_sources(source, cancel)?;
            return Err(SegmentationError::Preparation);
        }
    };
    if volume.source_frames().len() != source.frames.len() {
        return Err(SegmentationError::Association);
    };
    let mut sorted = HashMap::new();
    sorted
        .try_reserve(volume.frames().len())
        .map_err(|_| SegmentationError::Limit)?;
    for (z, frame) in volume.frames().iter().enumerate() {
        sorted.insert(Arc::as_ptr(frame), z);
    }
    for (_, original) in &mut associated {
        let frame = volume
            .source_frames()
            .get(*original)
            .ok_or(SegmentationError::Association)?;
        *original = *sorted
            .get(&Arc::as_ptr(frame))
            .ok_or(SegmentationError::Association)?;
    }
    let _payload_memory = MemoryBudget::volumes()
        .reserve(expected as u64)
        .ok_or(SegmentationError::Limit)?;
    let mut payload = Vec::new();
    payload
        .try_reserve_exact(expected)
        .map_err(|_| SegmentationError::Limit)?;
    payload.resize(expected, 0);
    for block in payload.chunks_mut(64 * 1024) {
        if cancel() {
            return Err(SegmentationError::Cancelled);
        };
        reader
            .read_exact(block)
            .map_err(|_| SegmentationError::Io)?;
    }
    let after = fs::symlink_metadata(path).map_err(|_| SegmentationError::Changed)?;
    if !after.is_file()
        || after.file_type().is_symlink()
        || Stamp::new(&after) != stamp
        || Stamp::new(
            &reader
                .get_ref()
                .metadata()
                .map_err(|_| SegmentationError::Io)?,
        ) != stamp
    {
        return Err(SegmentationError::Changed);
    };
    verify_sources(source, cancel)?;
    let overlap = if root.element(Tag(0x62, 0x13)).is_ok() {
        match text(root, Tag(0x62, 0x13))?.as_str() {
            "YES" => Some(true),
            "NO" => Some(false),
            "UNDEFINED" => None,
            _ => return Err(SegmentationError::Metadata),
        }
    } else {
        None
    };
    Segments::from_binary(volume, &numbers, &associated, &payload, overlap, cancel)
        .map_err(SegmentationError::Mask)
}
