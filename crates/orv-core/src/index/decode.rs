// SPDX-License-Identifier: GPL-3.0-or-later
use super::{
    header::Header,
    source::{ReadState, Source},
};
use crate::{WindowLevel, load::LoadError, study::Frame};
use dicom_core::{DataElement, DicomValue, PrimitiveValue, Tag, VR};
use dicom_pixeldata::{ConvertOptions, PixelDecoder, VoiLutOption};
use std::{io::Cursor, sync::Arc};

pub(super) const MAX_ENCODED_OBJECT: u64 = 64 * 1024 * 1024;

pub(super) fn decode(
    source: &Source,
    header: &Header,
    frame: u32,
    state: &mut ReadState,
    cancelled: &impl Fn() -> bool,
) -> Result<Arc<Frame>, LoadError> {
    if frame >= header.frames {
        return Err(LoadError::Unsupported);
    }
    if let super::header::Format::Nifti(nifti) = &header.format {
        return nifti.decode(source, header, frame, state, cancelled);
    }
    let object = if header.native {
        let bytes_per_frame = u64::from(header.width)
            * u64::from(header.height)
            * u64::from(header.bits / 8)
            * crate::dicom_color::native_samples(header.dicom()?)?;
        let offset = header
            .offset
            .checked_add(
                bytes_per_frame
                    .checked_mul(u64::from(frame))
                    .ok_or(LoadError::Limit)?,
            )
            .ok_or(LoadError::Limit)?;
        let bytes = source.range(offset, bytes_per_frame as usize, state, cancelled)?;
        let mut object = frame_object(header.dicom()?, frame)?;
        let value = match header.bits {
            8 => PrimitiveValue::U8(bytes.into()),
            16 => PrimitiveValue::U16(
                bytes
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|b| {
                        if header.big_endian {
                            u16::from_be_bytes([b[0], b[1]])
                        } else {
                            u16::from_le_bytes([b[0], b[1]])
                        }
                    })
                    .collect(),
            ),
            32 => PrimitiveValue::U32(
                bytes
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|b| {
                        if header.big_endian {
                            u32::from_be_bytes([b[0], b[1], b[2], b[3]])
                        } else {
                            u32::from_le_bytes([b[0], b[1], b[2], b[3]])
                        }
                    })
                    .collect(),
            ),
            _ => return Err(LoadError::Unsupported),
        };
        object.put(DataElement::new(
            Tag(0x7fe0, 0x0010),
            if header.bits == 8 { VR::OB } else { VR::OW },
            value,
        ));
        object
    } else if header.pixel_length.is_none() {
        match super::encapsulated::fragments(source, header, frame, state, cancelled) {
            Ok(fragments) => {
                let mut object = frame_object(header.dicom()?, frame)?;
                object.put(DataElement::new(
                    Tag(0x7fe0, 0x0010),
                    VR::OB,
                    DicomValue::PixelSequence(dicom_core::value::PixelFragmentSequence::new(
                        Vec::new(),
                        fragments,
                    )),
                ));
                object
            }
            Err(LoadError::Unsupported) if source.length <= MAX_ENCODED_OBJECT => {
                let bytes = source.range(0, source.length as usize, state, cancelled)?;
                dicom_object::from_reader(Cursor::new(bytes)).map_err(|_| LoadError::Unsupported)?
            }
            Err(e) => return Err(e),
        }
    } else {
        if source.length > MAX_ENCODED_OBJECT {
            return Err(LoadError::Limit);
        }
        let bytes = source.range(0, source.length as usize, state, cancelled)?;
        dicom_object::from_reader(Cursor::new(bytes)).map_err(|_| LoadError::Unsupported)?
    };
    if cancelled() {
        return Err(LoadError::Cancelled);
    }
    crate::dicom_codec::validate(&object)?;
    let mode = crate::dicom_color::layout(&object)?;
    let padding = crate::dicom_padding::Padding::read(&object)?;
    let decoded = if mode == Some(crate::dicom_color::Color::PackedYbr) {
        None
    } else {
        Some(
            object
                .decode_pixel_data_frame(
                    if crate::load::integer(&object, "NumberOfFrames").unwrap_or(1) == 1 {
                        0
                    } else {
                        frame
                    },
                )
                .map_err(|_| LoadError::Unsupported)?,
        )
    };
    if decoded.as_ref().is_some_and(|decoded| {
        decoded.rows() != header.height
            || decoded.columns() != header.width
            || decoded.number_of_frames() != 1
    }) {
        return Err(LoadError::Unsupported);
    }
    if let Some(color) = mode {
        let presentation =
            crate::dicom_color::present(&object, decoded.as_ref(), color, 0, padding, cancelled)?;
        return Ok(Arc::new(Frame {
            validity: None,
            color: Some(presentation.source),
            nifti: None,
            width: header.width,
            height: header.height,
            pixels: Arc::from([]),
            rgba: Some(presentation.rgba),
            geometry: header.geometry_for(frame).cloned(),
            instance: header.instance,
            frame_index: frame,
            monochrome1: false,
            default_window: WindowLevel {
                width: 256.,
                center: 128.,
            },
        }));
    }
    let options = ConvertOptions::new().with_voi_lut(VoiLutOption::Identity);
    let decoded = decoded.ok_or(LoadError::Unsupported)?;
    let pixels: Vec<f32> = decoded
        .to_vec_with_options(&options)
        .map_err(|_| LoadError::Unsupported)?;
    if pixels.len() as u64 != u64::from(header.width) * u64::from(header.height)
        || pixels.iter().any(|v| !v.is_finite())
    {
        return Err(LoadError::Unsupported);
    }
    let validity = padding
        .map(|p| p.mask(&decoded, 0, cancelled))
        .transpose()?
        .flatten();
    let window = crate::load::number(&object, "WindowWidth")
        .zip(crate::load::number(&object, "WindowCenter"))
        .map(|(width, center)| WindowLevel {
            width: width as f32,
            center: center as f32,
        })
        .filter(|w| w.is_valid())
        .unwrap_or_else(|| crate::dicom_padding::automatic_window(&pixels, validity.as_deref()));
    Ok(Arc::new(Frame {
        validity,
        color: None,
        nifti: None,
        rgba: None,
        width: header.width,
        height: header.height,
        pixels: pixels.into(),
        geometry: header.geometry_for(frame).cloned(),
        instance: header.instance,
        frame_index: frame,
        monochrome1: crate::load::text(&object, "PhotometricInterpretation").as_deref()
            == Some("MONOCHROME1"),
        default_window: window,
    }))
}

fn frame_object(
    source: &dicom_object::DefaultDicomObject,
    frame: u32,
) -> Result<dicom_object::DefaultDicomObject, LoadError> {
    // Only the selected functional group is cloned, not every frame's metadata.
    let mut object = dicom_object::InMemDicomObject::from_element_iter(
        source
            .iter()
            .filter(|element| element.header().tag != Tag(0x5200, 0x9230))
            .cloned(),
    )
    .with_exact_meta(source.meta().clone());
    object.put_str(Tag(0x0028, 0x0008), VR::IS, "1");
    // Preserve the selected frame's enhanced rescale metadata. Geometry is
    // validated independently from the full shared/per-frame source metadata.
    if let Ok(element) = source.element(Tag(0x5200, 0x9230)) {
        let selected = element
            .value()
            .items()
            .and_then(|items| items.get(frame as usize))
            .cloned()
            .ok_or(LoadError::Unsupported)?;
        object.put(DataElement::new(
            Tag(0x5200, 0x9230),
            VR::SQ,
            DicomValue::new_sequence(vec![selected], dicom_core::Length::UNDEFINED),
        ));
    }
    Ok(object)
}
