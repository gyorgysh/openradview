// SPDX-License-Identifier: GPL-3.0-or-later
//! Bounded embedded RGB input profile conversion; never fetch or expose profile text.
use crate::{load::LoadError, scene::hex};
use dicom_core::Tag;
use dicom_object::DefaultDicomObject;
use moxcms::{ColorProfile, Layout, ParsingOptions, RenderingIntent, TransformOptions};
use sha2::{Digest, Sha256};
pub(super) fn to_srgb(
    object: &DefaultDicomObject,
    rgb: &[u16],
    bits: u16,
    cancelled: &impl Fn() -> bool,
) -> Result<(Vec<u8>, Option<String>), LoadError> {
    let maximum = (1u32 << bits) - 1;
    let bytes = object
        .element(Tag(0x0028, 0x2000))
        .ok()
        .map(|e| e.to_bytes().map_err(|_| LoadError::Unsupported))
        .transpose()?;
    let profile = if let Some(bytes) = &bytes {
        if bytes.len() < 132
            || bytes.len() > 1024 * 1024
            || &bytes[12..16] != b"scnr"
            || &bytes[16..20] != b"RGB "
            || !matches!(&bytes[20..24], b"XYZ " | b"Lab ")
            || !matches!(bytes[8], 2 | 4)
            || &bytes[36..40] != b"acsp"
        {
            return Err(LoadError::Unsupported);
        }
        let length =
            u32::from_be_bytes(bytes[..4].try_into().map_err(|_| LoadError::Unsupported)?) as usize;
        if length < 132
            || length > bytes.len()
            || bytes.len() - length > 1
            || bytes[length..].iter().any(|b| *b != 0)
        {
            return Err(LoadError::Unsupported);
        }
        if cancelled() {
            return Err(LoadError::Cancelled);
        }
        let profile = ColorProfile::new_from_slice_with_options(
            &bytes[..length],
            ParsingOptions {
                max_profile_size: 1024 * 1024 + 1,
                max_allowed_clut_size: 1024 * 1024,
                max_allowed_trc_size: 65536,
            },
        )
        .map_err(|_| LoadError::Unsupported)?;
        let options = TransformOptions {
            rendering_intent: RenderingIntent::RelativeColorimetric,
            ..Default::default()
        };
        let transform = profile
            .create_transform_16bit(Layout::Rgb, &ColorProfile::new_srgb(), Layout::Rgb, options)
            .map_err(|_| LoadError::Unsupported)?;
        Some((transform, hex(&Sha256::digest(&bytes[..length]))))
    } else {
        None
    };
    let mut rgba = Vec::with_capacity(rgb.len() / 3 * 4);
    for chunk in rgb.chunks(16384 * 3) {
        if cancelled() {
            return Err(LoadError::Cancelled);
        }
        let mut output = Vec::new();
        let samples = if let Some((transform, _)) = &profile {
            let input: Vec<u16> = chunk
                .iter()
                .map(|v| ((*v as u32 * 65535 + maximum / 2) / maximum) as u16)
                .collect();
            output.resize(chunk.len(), 0);
            transform
                .transform(&input, &mut output)
                .map_err(|_| LoadError::Unsupported)?;
            output.as_slice()
        } else {
            chunk
        };
        let max = if profile.is_some() { 65535 } else { maximum };
        for pixel in samples.as_chunks::<3>().0 {
            rgba.extend(
                pixel
                    .iter()
                    .map(|v| ((*v as u32 * 255 + max / 2) / max) as u8),
            );
            rgba.push(255);
        }
    }
    Ok((rgba, profile.map(|(_, hash)| hash)))
}
