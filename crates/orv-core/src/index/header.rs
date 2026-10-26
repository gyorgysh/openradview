// SPDX-License-Identifier: GPL-3.0-or-later
//! Bounded structural metadata parsing stops before allocating pixel data.
use crate::{load::LoadError, study::Geometry};
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

const MAX_HEADER: usize = 8 * 1024 * 1024;
pub(super) const MAX_FRAME_SAMPLES: u64 = 32 * 1024 * 1024;

pub(super) struct Header {
    pub offsets: OnceLock<super::encapsulated::Offsets>,
    /// Part 10 prefix length before a raw-DEFLATE dataset, if present.
    pub deflated_at: Option<u64>,
    pub format: Format,
    pub offset: u64,
    pub pixel_length: Option<u64>,
    pub width: u32,
    pub height: u32,
    pub frames: u32,
    pub native: bool,
    pub bits: u16,
    pub big_endian: bool,
    pub geometry: Option<Geometry>,
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

pub(super) fn read_header(
    reader: &mut impl Read,
    cancelled: impl Fn() -> bool,
) -> Result<Header, LoadError> {
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
    let syntax = meta.transfer_syntax().to_owned();
    let dataset_start = prefix.len();
    let deflated_at = (syntax == "1.2.840.10008.1.2.1.99").then_some(dataset_start as u64);
    let reader: Box<dyn Read + '_> = if deflated_at.is_some() {
        meta.transfer_syntax = "1.2.840.10008.1.2.1".into();
        meta.update_information_group_length();
        Box::new(flate2::read::DeflateDecoder::new(reader))
    } else {
        Box::new(reader)
    };
    let ts = TransferSyntaxRegistry
        .get(meta.transfer_syntax())
        .ok_or(LoadError::Unsupported)?;
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
    let (mut depth, mut tokens) = (0usize, 0usize);
    let (pixel_length, pixel_header) = loop {
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
            DataToken::ElementHeader(header) if depth == 0 && header.tag == Tag(0x7fe0, 0x0010) => {
                break (header.len.get().map(u64::from), before as usize);
            }
            DataToken::PixelSequenceStart if depth == 0 => break (None, before as usize),
            DataToken::ElementHeader(header) => {
                if header
                    .len
                    .get()
                    .is_none_or(|n| n as usize > MAX_HEADER.saturating_sub(position.get() as usize))
                {
                    return Err(LoadError::Limit);
                }
            }
            DataToken::SequenceStart { len, .. } | DataToken::ItemStart { len } => {
                depth += 1;
                if depth > 64
                    || len.get().is_some_and(|n| {
                        n as usize > MAX_HEADER.saturating_sub(position.get() as usize)
                    })
                {
                    return Err(LoadError::Limit);
                }
            }
            DataToken::SequenceEnd | DataToken::ItemEnd => {
                depth = depth.checked_sub(1).ok_or(LoadError::Unsupported)?;
            }
            DataToken::PixelSequenceStart => {
                depth += 1;
                if depth > 64 {
                    return Err(LoadError::Limit);
                }
            }
            DataToken::ItemValue(_) | DataToken::OffsetTable(_) if depth == 0 => {
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
            .read_until(Tag(0x7fe0, 0x0010))
            .from_reader(Cursor::new(&prefix))
            .map_err(|_| LoadError::Unsupported)?
    };
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
    if !matches!(bits, 1 | 8 | 16 | 32) || object.element_by_name("ModalityLUTSequence").is_ok() {
        return Err(LoadError::Unsupported);
    }
    if deflated_at.is_some() && bits == 1 {
        return Err(LoadError::Unsupported);
    }
    let color = crate::dicom_color::layout(&object)?;
    if color.is_some() && width * height * 3 > MAX_FRAME_SAMPLES {
        return Err(LoadError::Limit);
    }
    let reference = crate::load::text(&object, "FrameOfReferenceUID").filter(|s| !s.is_empty());
    let uid = crate::load::text(&object, "SeriesInstanceUID")
        .filter(|s| !s.is_empty())
        .ok_or(LoadError::Unsupported)?;
    let geometry = if frames == 1
        && matches!(
            crate::load::text(&object, "AnatomicalOrientationType").as_deref(),
            None | Some("BIPED")
        ) {
        crate::load::numbers::<3>(&object, "ImagePositionPatient")
            .zip(crate::load::numbers::<6>(
                &object,
                "ImageOrientationPatient",
            ))
            .zip(crate::load::numbers::<2>(&object, "PixelSpacing"))
            .and_then(|((position, orientation), spacing)| {
                spacing.iter().all(|v| *v > 0.).then(|| Geometry {
                    nifti_transform: None,
                    frame_of_reference: reference.clone(),
                    position,
                    orientation,
                    spacing,
                })
            })
    } else {
        None
    };
    let modality = match crate::load::text(&object, "Modality").as_deref() {
        Some("CT") => "CT",
        Some("MR") => "MR",
        Some("DX") => "DX",
        Some("CR") => "CR",
        Some("XA") => "XA",
        Some("US") => "US",
        _ => "IMAGE",
    };
    let key = format!(
        "{}|{}|{}",
        crate::load::text(&object, "StudyInstanceUID").unwrap_or_default(),
        uid,
        reference.as_deref().unwrap_or_default()
    );
    Ok(Header {
        offsets: OnceLock::new(),
        deflated_at,
        offset: position.get(),
        pixel_length,
        width: width as u32,
        height: height as u32,
        frames: frames as u32,
        native: matches!(
            syntax.as_str(),
            "1.2.840.10008.1.2"
                | "1.2.840.10008.1.2.1"
                | "1.2.840.10008.1.2.2"
                | "1.2.840.10008.1.2.1.99"
        ) && bits != 1,
        bits: bits as u16,
        big_endian: syntax == "1.2.840.10008.1.2.2",
        geometry,
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
    while !bytes.is_empty() {
        elements += 1;
        if elements > 4096 || bytes.len() < 8 || bytes[..2] != [2, 0] {
            return Err(LoadError::Unsupported);
        }
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
