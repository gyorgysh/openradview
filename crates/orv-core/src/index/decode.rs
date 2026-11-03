// SPDX-License-Identifier: GPL-3.0-or-later
use super::{
    header::Header,
    source::{ReadState, Source},
};
use crate::{WindowLevel, load::LoadError, study::Frame};
use dicom_core::{DataElement, DicomValue, Tag, VR};
use dicom_pixeldata::PixelDecoder;
use std::{io::Cursor, sync::Arc};

mod native;

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
        native::object(source, header, frame, state, cancelled)?
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
    let source_frame = if crate::load::integer(&object, "NumberOfFrames").unwrap_or(1) == 1 {
        0
    } else {
        frame
    };
    let decoded = if mode == Some(crate::dicom_color::Color::PackedYbr) {
        None
    } else {
        Some(
            object
                .decode_pixel_data_frame(source_frame)
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
            slab: None,
            scalar: None,
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
                function: Default::default(),
                width: 256.,
                center: 128.,
            },
        }));
    }
    let scalar = header
        .scalar
        .get(frame)
        .cloned()
        .ok_or(LoadError::Unsupported)?;
    let decoded = decoded.ok_or(LoadError::Unsupported)?;
    let pixels = crate::dicom_voi::pixels(&object, &decoded, 0, source_frame, &scalar, cancelled)?;
    if pixels.len() as u64 != u64::from(header.width) * u64::from(header.height)
        || pixels.iter().any(|v| !v.is_finite())
    {
        return Err(LoadError::Unsupported);
    }
    let validity = crate::dicom_voi::mask(
        &object,
        &decoded,
        0,
        source_frame,
        &scalar,
        padding,
        cancelled,
    )?;
    let window = scalar
        .choices()
        .original()
        .unwrap_or_else(|| crate::dicom_padding::automatic_window(&pixels, validity.as_deref()));
    Ok(Arc::new(Frame {
        slab: None,
        scalar: Some(scalar),
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
    // Immutable modality and display tables live in Header. Source values are
    // mapped exactly once after codec decoding, before spatial interpolation.
    // Avoid copying those tables or unselected functional groups for every slice.
    let mut object = dicom_object::InMemDicomObject::from_element_iter(
        source
            .iter()
            .filter(|element| {
                !matches!(
                    element.header().tag,
                    Tag(0x5200, 0x9229 | 0x9230) | Tag(0x0028, 0x3000 | 0x3010)
                )
            })
            .cloned(),
    )
    .with_exact_meta(source.meta().clone());
    object.put_str(Tag(0x0028, 0x0008), VR::IS, "1");
    // Preserve the selected frame's enhanced rescale metadata. Geometry is
    // validated independently from the full shared/per-frame source metadata.
    for (tag, index) in [
        (Tag(0x5200, 0x9229), 0),
        (Tag(0x5200, 0x9230), frame as usize),
    ] {
        let Ok(element) = source.element(tag) else {
            continue;
        };
        let selected = element
            .items()
            .and_then(|items| items.get(index))
            .ok_or(LoadError::Unsupported)?;
        let selected = dicom_object::InMemDicomObject::from_element_iter(
            selected
                .iter()
                .filter(|element| element.header().tag != Tag(0x0028, 0x9132))
                .cloned(),
        );
        object.put(DataElement::new(
            tag,
            VR::SQ,
            DicomValue::new_sequence(vec![selected], dicom_core::Length::UNDEFINED),
        ));
    }
    Ok(object)
}
