// SPDX-License-Identifier: GPL-3.0-or-later
//! Bounded structural metadata parsing stops before allocating pixel data.
use crate::{
    import_report::{InputFailure, SkipReason},
    load::LoadError,
    study::Geometry,
};
use dicom_core::Tag;
use dicom_encoding::transfer_syntax::TransferSyntaxIndex;
use dicom_object::{DefaultDicomObject, FileMetaTable, OpenFileOptions};
use dicom_parser::dataset::{DataSetReader, DataToken};
use dicom_transfer_syntax_registry::TransferSyntaxRegistry;
use std::{
    cell::Cell,
    io::{Cursor, Read},
    sync::{Arc, OnceLock},
};
mod scopes;

const MAX_HEADER: usize = 8 * 1024 * 1024;
pub(super) const MAX_FRAME_SAMPLES: u64 = 32 * 1024 * 1024;

pub(super) struct Header {
    pub offsets: OnceLock<super::encapsulated::Offsets>,
    /// Part 10 prefix length before a raw-DEFLATE dataset, if present.
    pub deflated_at: Option<u64>,
    pub format: Format,
    pub offset: u64,
    pub pixel_length: Option<u64>,
    /// OF identifies native Float Pixel Data.
    pub pixel_vr: dicom_core::VR,
    pub float_padding: Option<crate::dicom_float::Padding>,
    pub presentation: Option<Arc<crate::presentation::Layers>>,
    pub width: u32,
    pub height: u32,
    pub frames: u32,
    pub native: bool,
    pub bits: u16,
    pub big_endian: bool,
    pub geometry: Option<Geometry>,
    pub frame_spatial: Vec<crate::dicom_geometry::FrameSpatial>,
    pub acquisition: Option<[u8; 32]>,
    pub scalar: crate::dicom_voi::Presentations,
    pub key: String,
    pub modality: &'static str,
    pub series_number: i32,
    pub instance: i32,
}

pub(super) enum Format {
    Dicom(Arc<DefaultDicomObject>),
    Nifti(super::nifti::Nifti),
}
impl Header {
    pub fn geometry_for(&self, frame: u32) -> Option<&Geometry> {
        if self.frame_spatial.is_empty() {
            self.geometry.as_ref()
        } else {
            self.frame_spatial.get(frame as usize)?.geometry.as_ref()
        }
    }
    pub fn acquisition_for(&self, frame: u32) -> Option<[u8; 32]> {
        if self.frame_spatial.is_empty() {
            self.acquisition
        } else {
            self.frame_spatial.get(frame as usize)?.acquisition
        }
    }
    pub fn spatial_bytes(&self) -> u64 {
        self.scalar.metadata_bytes
            + self
                .frame_spatial
                .iter()
                .map(|frame| {
                    std::mem::size_of::<crate::dicom_geometry::FrameSpatial>() as u64
                        + frame
                            .geometry
                            .as_ref()
                            .and_then(|geometry| geometry.frame_of_reference.as_ref())
                            .map_or(0, |value| value.len() as u64)
                })
                .sum::<u64>()
    }
    pub fn dicom(&self) -> Result<&DefaultDicomObject, LoadError> {
        match &self.format {
            Format::Dicom(object) => Ok(object),
            Format::Nifti(_) => Err(LoadError::Unsupported),
        }
    }
}

struct Recording<'a, R, F> {
    reader: R,
    prefix: &'a mut Vec<u8>,
    position: &'a Cell<u64>,
    cancelled: &'a F,
}
impl<R: Read, F: Fn() -> bool> Read for Recording<'_, R, F> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if (self.cancelled)() {
            return Err(std::io::Error::other("Cancelled"));
        }
        let size = buffer
            .len()
            .min(MAX_HEADER.saturating_sub(self.prefix.len()))
            .min(32 * 1024);
        if size == 0 {
            return Err(std::io::Error::other("Metadata limit"));
        }
        let count = self.reader.read(&mut buffer[..size])?;
        self.prefix.extend_from_slice(&buffer[..count]);
        self.position.set(self.position.get() + count as u64);
        Ok(count)
    }
}

#[cfg(test)]
pub(super) fn read_header(
    reader: &mut impl Read,
    cancelled: impl Fn() -> bool,
) -> Result<Header, LoadError> {
    read_header_detailed(reader, cancelled).map_err(|failure| failure.error)
}

pub(super) fn read_header_detailed(
    reader: &mut impl Read,
    cancelled: impl Fn() -> bool,
) -> Result<Header, InputFailure> {
    let mut reason = SkipReason::UnrecognizedInput;
    read_header_inner(reader, &cancelled, &mut reason).map_err(|error| InputFailure {
        error: if cancelled() {
            LoadError::Cancelled
        } else {
            error
        },
        reason,
    })
}

pub(super) struct MetadataPrefix {
    pub object: DefaultDicomObject,
    pub syntax: String,
    pub deflated_at: Option<u64>,
    pub pixel_offset: u64,
    pub pixel_length: Option<u64>,
    pub pixel_vr: dicom_core::VR,
}

/// Bounded metadata only. The class policy runs before dataset values are read.
/// The reader is left at the pixel payload; no pixel value is allocated here.
pub(super) fn read_prefix(
    reader: &mut impl Read,
    cancelled: &impl Fn() -> bool,
    reason: &mut SkipReason,
    class_allowed: impl Fn(&str) -> bool,
) -> Result<MetadataPrefix, LoadError> {
    read_prefix_with_syntax(reader, cancelled, reason, class_allowed, |_| true)
}

pub(super) fn read_prefix_with_syntax(
    reader: &mut impl Read,
    cancelled: &impl Fn() -> bool,
    reason: &mut SkipReason,
    class_allowed: impl Fn(&str) -> bool,
    syntax_allowed: impl Fn(&str) -> bool,
) -> Result<MetadataPrefix, LoadError> {
    let mut first = [0u8; 132];
    reader
        .read_exact(&mut first[..4])
        .map_err(|_| LoadError::Unsupported)?;
    let start = if &first[..4] == b"DICM" {
        0
    } else {
        reader
            .read_exact(&mut first[4..])
            .map_err(|_| LoadError::Unsupported)?;
        if &first[128..] != b"DICM" {
            return Err(LoadError::Unsupported);
        }
        128
    };
    *reason = SkipReason::MalformedMetadata;
    let mut prefix = first[..start + 4].to_vec();
    let mut group = [0u8; 12];
    reader
        .read_exact(&mut group)
        .map_err(|_| LoadError::Unsupported)?;
    // Part 10 File Meta Information is always explicit VR little endian and
    // begins with its group length, as required by the existing object parser.
    if group[..8] != [2, 0, 0, 0, b'U', b'L', 4, 0] {
        return Err(LoadError::Unsupported);
    }
    let length = u32::from_le_bytes(
        group[8..12]
            .try_into()
            .map_err(|_| LoadError::Unsupported)?,
    ) as usize;
    if length > MAX_HEADER / 2 {
        return Err(LoadError::Limit);
    }
    prefix.extend_from_slice(&group);
    let end = prefix.len() + length;
    prefix.resize(end, 0);
    reader
        .read_exact(&mut prefix[end - length..])
        .map_err(|_| LoadError::Unsupported)?;
    validate_meta(&prefix[start + 4..])?;
    let mut meta = FileMetaTable::from_reader(Cursor::new(&prefix[start..]))
        .map_err(|_| LoadError::Unsupported)?;
    let class = meta.media_storage_sop_class_uid();
    if !class_allowed(class) {
        *reason = SkipReason::UnsupportedObject;
        return Err(LoadError::Unsupported);
    }
    let syntax = meta.transfer_syntax().to_owned();
    if !syntax_allowed(&syntax) {
        *reason = SkipReason::UnsupportedEncoding;
        return Err(LoadError::Unsupported);
    }
    let dataset_start = prefix.len();
    let deflated_at = (syntax == "1.2.840.10008.1.2.1.99").then_some(dataset_start as u64);
    let reader: Box<dyn Read + '_> = if deflated_at.is_some() {
        meta.transfer_syntax = "1.2.840.10008.1.2.1".into();
        meta.update_information_group_length();
        Box::new(flate2::read::DeflateDecoder::new(reader))
    } else {
        Box::new(reader)
    };
    *reason = SkipReason::UnsupportedEncoding;
    let ts = TransferSyntaxRegistry
        .get(meta.transfer_syntax())
        .ok_or(LoadError::Unsupported)?;
    if !ts.can_decode_all() {
        return Err(LoadError::Unsupported);
    }
    *reason = SkipReason::MalformedMetadata;
    let position = Cell::new(prefix.len() as u64);
    let mut dataset = DataSetReader::new_with_ts(
        Recording {
            reader,
            prefix: &mut prefix,
            position: &position,
            cancelled: &cancelled,
        },
        ts,
    )
    .map_err(|_| LoadError::Unsupported)?;
    let mut scopes = scopes::Scopes::new()?;
    let mut tokens = 0usize;
    let (pixel_length, pixel_header, pixel_vr) = loop {
        if cancelled() {
            return Err(LoadError::Cancelled);
        }
        tokens += 1;
        if tokens > 100_000 {
            return Err(LoadError::Limit);
        }
        let before = position.get();
        let token = dataset.next().ok_or(LoadError::Unsupported)?.map_err(|_| {
            if cancelled() {
                LoadError::Cancelled
            } else {
                LoadError::Unsupported
            }
        })?;
        match token {
            DataToken::ElementHeader(header)
                if scopes.depth() == 0 && header.tag == Tag(0x7fe0, 0x0010) =>
            {
                break (header.len.get().map(u64::from), before as usize, header.vr);
            }
            // Float Pixel Data uses its own OF element. Double Float Pixel Data
            // stays refused until a retained double representation is designed.
            DataToken::ElementHeader(header)
                if scopes.depth() == 0
                    && header.tag == crate::dicom_float::FLOAT
                    && header.vr == dicom_core::VR::OF =>
            {
                break (header.len.get().map(u64::from), before as usize, header.vr);
            }
            DataToken::ElementHeader(header)
                if scopes.depth() == 0 && matches!(header.tag, Tag(0x7fe0, 0x0008 | 0x0009)) =>
            {
                *reason = SkipReason::UnsupportedOrInvalid;
                return Err(LoadError::Unsupported);
            }
            DataToken::PixelSequenceStart if scopes.depth() == 0 => {
                break (None, before as usize, dicom_core::VR::OB);
            }
            DataToken::ElementHeader(header) => {
                scopes.attribute(header.tag)?;
                if header
                    .len
                    .get()
                    .is_none_or(|n| n as usize > MAX_HEADER.saturating_sub(position.get() as usize))
                {
                    return Err(LoadError::Limit);
                }
            }
            DataToken::SequenceStart { tag, len } => {
                scopes.sequence(tag)?;
                if len.get().is_some_and(|n| {
                    n as usize > MAX_HEADER.saturating_sub(position.get() as usize)
                }) {
                    return Err(LoadError::Limit);
                }
            }
            DataToken::ItemStart { len } => {
                scopes.item()?;
                if len.get().is_some_and(|n| {
                    n as usize > MAX_HEADER.saturating_sub(position.get() as usize)
                }) {
                    return Err(LoadError::Limit);
                }
            }
            DataToken::SequenceEnd => scopes.end_sequence()?,
            DataToken::ItemEnd => scopes.end_item()?,
            DataToken::PixelSequenceStart => {
                scopes.sequence(Tag(0x7fe0, 0x0010))?;
            }
            DataToken::ItemValue(_) | DataToken::OffsetTable(_) if scopes.depth() == 0 => {
                return Err(LoadError::Unsupported);
            }
            DataToken::ItemValue(_) | DataToken::OffsetTable(_) => (),
            DataToken::PrimitiveValue(_) => (),
        }
    };
    drop(dataset);
    let object = if deflated_at.is_some() {
        dicom_object::InMemDicomObject::read_dataset_with_ts(
            Cursor::new(&prefix[dataset_start..pixel_header]),
            ts,
        )
        .map_err(|_| LoadError::Unsupported)?
        .with_exact_meta(meta)
    } else {
        OpenFileOptions::new()
            .read_until(if pixel_vr == dicom_core::VR::OF {
                crate::dicom_float::FLOAT
            } else {
                Tag(0x7fe0, 0x0010)
            })
            .from_reader(Cursor::new(&prefix))
            .map_err(|_| LoadError::Unsupported)?
    };
    Ok(MetadataPrefix {
        object,
        syntax,
        deflated_at,
        pixel_offset: position.get(),
        pixel_length,
        pixel_vr,
    })
}

fn read_header_inner(
    reader: &mut impl Read,
    cancelled: &impl Fn() -> bool,
    reason: &mut SkipReason,
) -> Result<Header, LoadError> {
    let MetadataPrefix {
        object,
        syntax,
        deflated_at,
        pixel_offset,
        pixel_length,
        pixel_vr,
    } = read_prefix(reader, cancelled, reason, |class| {
        !crate::dicom_codec::non_image_class(class)
    })?;
    if crate::dicom_codec::non_image_object(&object) {
        *reason = SkipReason::UnsupportedObject;
        return Err(LoadError::Unsupported);
    }
    *reason = SkipReason::UnsupportedOrInvalid;
    let width = crate::load::integer(&object, "Columns").ok_or(LoadError::Unsupported)?;
    let height = crate::load::integer(&object, "Rows").ok_or(LoadError::Unsupported)?;
    let frames = crate::load::integer(&object, "NumberOfFrames").unwrap_or(1);
    if width == 0
        || height == 0
        || width > 8192
        || height > 8192
        || width * height > MAX_FRAME_SAMPLES
        || frames == 0
        || frames > 1_000_000
    {
        return Err(LoadError::Limit);
    }
    let bits = crate::load::integer(&object, "BitsAllocated").ok_or(LoadError::Unsupported)?;
    let float = pixel_vr == dicom_core::VR::OF;
    let float_padding = float
        .then(|| crate::dicom_float::validate(&object))
        .transpose()?
        .flatten();
    let native_syntax = matches!(
        syntax.as_str(),
        "1.2.840.10008.1.2"
            | "1.2.840.10008.1.2.1"
            | "1.2.840.10008.1.2.2"
            | "1.2.840.10008.1.2.1.99"
    );
    if float && (!native_syntax || pixel_length.is_none()) {
        return Err(LoadError::Unsupported);
    }
    if native_syntax
        && !float
        && (!matches!(pixel_vr, dicom_core::VR::OB | dicom_core::VR::OW)
            || (bits > 8 && pixel_vr != dicom_core::VR::OW))
    {
        return Err(LoadError::Unsupported);
    }
    if !matches!(bits, 1 | 8 | 16 | 32) {
        return Err(LoadError::Unsupported);
    }
    if deflated_at.is_some() && bits == 1 {
        return Err(LoadError::Unsupported);
    }
    let color = crate::dicom_color::layout(&object)?;
    if color.is_some() && width * height * 3 > MAX_FRAME_SAMPLES {
        return Err(LoadError::Limit);
    }
    let reference = crate::load::identifier(&object, "FrameOfReferenceUID")?;
    let uid =
        crate::load::identifier(&object, "SeriesInstanceUID")?.ok_or(LoadError::Unsupported)?;
    let spatial = crate::dicom_geometry::read(&object, frames as u32, &cancelled)?;
    let scalar = crate::dicom_voi::read(&object, frames as u32, float, &cancelled)?;
    let presentation = crate::presentation::read(&object, frames as u32)?;
    // Layers hold the used overlay bits; drop the encoded copies from the
    // retained object. Import already charged the full prefix length.
    let mut object = object;
    for group in (0x6000..=0x601e).step_by(2) {
        object.remove_element(Tag(group, 0x3000));
    }
    let modality = match crate::load::text(&object, "Modality").as_deref() {
        Some("CT") => "CT",
        Some("MR") => "MR",
        Some("PT") => "PT",
        Some("DX") => "DX",
        Some("CR") => "CR",
        Some("XA") => "XA",
        Some("MG") => "MG",
        Some("US") => "US",
        _ => "IMAGE",
    };
    let key = format!(
        "{}|{}|{}",
        crate::load::identifier(&object, "StudyInstanceUID")?.unwrap_or_default(),
        uid,
        reference.as_deref().unwrap_or_default()
    );
    Ok(Header {
        offsets: OnceLock::new(),
        deflated_at,
        offset: pixel_offset,
        pixel_length,
        pixel_vr,
        float_padding,
        presentation,
        width: width as u32,
        height: height as u32,
        frames: frames as u32,
        native: native_syntax
            && (bits != 1
                || scalar
                    .get(0)
                    .is_some_and(|data| data.modality_lut().is_some())),
        bits: bits as u16,
        big_endian: syntax == "1.2.840.10008.1.2.2",
        geometry: spatial.geometry,
        frame_spatial: spatial.frames,
        acquisition: spatial.acquisition,
        scalar,
        key,
        modality,
        series_number: crate::load::number(&object, "SeriesNumber").unwrap_or(1.) as i32,
        instance: crate::load::number(&object, "InstanceNumber").unwrap_or(0.) as i32,
        format: Format::Dicom(Arc::new(object)),
    })
}

/// Check declared meta value lengths before the framework allocates them.
fn validate_meta(mut bytes: &[u8]) -> Result<(), LoadError> {
    let mut elements = 0;
    let mut tags = std::collections::HashSet::new();
    while !bytes.is_empty() {
        elements += 1;
        if elements > 4096 || bytes.len() < 8 || bytes[..2] != [2, 0] {
            return Err(LoadError::Unsupported);
        }
        let tag = u16::from_le_bytes([bytes[2], bytes[3]]);
        if tags.contains(&tag) {
            return Err(LoadError::Unsupported);
        }
        tags.try_reserve(1).map_err(|_| LoadError::Limit)?;
        tags.insert(tag);
        let (head, length) = match &bytes[4..6] {
            b"OB" | b"UN" => {
                if bytes.len() < 12 || bytes[6..8] != [0, 0] {
                    return Err(LoadError::Unsupported);
                }
                (
                    12,
                    u32::from_le_bytes(
                        bytes[8..12]
                            .try_into()
                            .map_err(|_| LoadError::Unsupported)?,
                    ) as usize,
                )
            }
            b"UI" | b"SH" | b"AE" | b"UL" => (8, u16::from_le_bytes([bytes[6], bytes[7]]) as usize),
            _ => return Err(LoadError::Unsupported),
        };
        if length > bytes.len() - head {
            return Err(LoadError::Limit);
        }
        bytes = &bytes[head + length..];
    }
    Ok(())
}
