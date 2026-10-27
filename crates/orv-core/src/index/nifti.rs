// SPDX-License-Identifier: GPL-3.0-or-later
//! Bounded NIfTI-1/2 volumes. Nonspatial axes are independent, named subseries.
mod decode;
mod fields;
mod geometry;
use super::{
    header::{Format, Header, MAX_FRAME_SAMPLES},
    source::{ReadState, Source},
};
use crate::{
    WindowLevel,
    load::LoadError,
    nifti_view::{ComplexView, NiftiView},
    scalar::{Rescale, ScalarData, ScalarInfo, Storage, Unit, VoiChoices},
};
use fields::Fields;
use std::sync::OnceLock;

pub(super) struct Nifti {
    datatype: u16,
    slope: f64,
    intercept: f64,
    step: [f64; 3],
    pub window: Option<WindowLevel>,
    pub view: NiftiView,
    pub metadata_bytes: u64,
}
/// Reads only the fixed header. The data may reside in a separate paired source.
pub(super) fn read_headers(
    source: &Source,
    data_length: u64,
    paired: bool,
    id: u64,
    cancelled: &impl Fn() -> bool,
) -> Result<Vec<Header>, LoadError> {
    let prefix = source.range(0, 4, &mut ReadState::default(), cancelled)?;
    let (big, version, size) = match prefix.as_slice() {
        [92, 1, 0, 0] => (false, 1, 348),
        [0, 0, 1, 92] => (true, 1, 348),
        [28, 2, 0, 0] => (false, 2, 540),
        [0, 0, 2, 28] => (true, 2, 540),
        _ => return Err(LoadError::Unsupported),
    };
    let bytes = source.range(0, size, &mut ReadState::default(), cancelled)?;
    let magic: &[u8] = match (version, paired) {
        (1, false) => b"n+1\0",
        (1, true) => b"ni1\0",
        (2, false) => b"n+2\0\r\n\x1a\n",
        _ => b"ni2\0\r\n\x1a\n",
    };
    let at = if version == 1 { 344 } else { 4 };
    if &bytes[at..at + magic.len()] != magic {
        return Err(LoadError::Unsupported);
    }
    let f = Fields {
        bytes: &bytes,
        big,
        version,
    };
    let ndim = f.dim(0)?;
    if !(2..=7).contains(&ndim) {
        return Err(LoadError::Unsupported);
    }
    let mut dims = [1u64; 7];
    for axis in 1..=ndim as usize {
        dims[axis - 1] = f.dim(axis)?;
    }
    if dims.contains(&0)
        || dims[0] > 8192
        || dims[1] > 8192
        || dims[0] * dims[1] > MAX_FRAME_SAMPLES
        || dims[2] > 1_000_000
    {
        return Err(LoadError::Limit);
    }
    let datatype = f.u16(70);
    let bits = match datatype {
        2 | 256 => 8,
        4 | 512 => 16,
        8 | 16 | 768 => 32,
        32 | 64 | 1024 | 1280 => 64,
        1792 => 128,
        _ => return Err(LoadError::Unsupported),
    };
    if f.u16(72) != bits {
        return Err(LoadError::Unsupported);
    }
    let offset = f.offset()?;
    if offset > 8 * 1024 * 1024 || (!paired && offset < size as u64 + 4) {
        return Err(LoadError::Limit);
    }
    let voxels = dims
        .iter()
        .try_fold(1u64, |a, b| a.checked_mul(*b))
        .ok_or(LoadError::Limit)?;
    let total = voxels
        .checked_mul(u64::from(bits / 8))
        .ok_or(LoadError::Limit)?;
    if offset
        .checked_add(total)
        .is_none_or(|end| end > data_length)
    {
        return Err(LoadError::Unsupported);
    }
    let volume_count = dims[3..].iter().product::<u64>();
    let views: &[ComplexView] = if matches!(datatype, 32 | 1792) {
        &[
            ComplexView::Magnitude,
            ComplexView::PhaseRadians,
            ComplexView::Real,
            ComplexView::Imaginary,
        ]
    } else {
        &[ComplexView::Scalar]
    };
    if volume_count * views.len() as u64 > 4096
        || dims[2] * volume_count * views.len() as u64 > 1_000_000
    {
        return Err(LoadError::Limit);
    }
    let (slope, intercept) = if f.f32(112) == 0. {
        (1., 0.)
    } else {
        (f.f32(112), f.f32(116))
    };
    if !slope.is_finite() || !intercept.is_finite() {
        return Err(LoadError::Unsupported);
    }
    let window = (views.len() == 1 && f.f32(124) > f.f32(128))
        .then(|| WindowLevel::source_range(f.f32(128) as f32, f.f32(124) as f32))
        .filter(|w| w.is_valid());
    let (geometry, step) = geometry::geometry(&f, id).unwrap_or((None, [0.; 3]));
    let length = dims[0] * dims[1] * dims[2] * u64::from(bits / 8);
    let mut headers = Vec::new();
    for volume in 0..volume_count {
        let mut n = volume;
        let indices = std::array::from_fn(|i| {
            let v = n % dims[i + 3];
            n /= dims[i + 3];
            v as u32
        });
        for view in views {
            let info = ScalarInfo {
                value_type: crate::scalar::ValueType::F32,
                storage: match datatype {
                    2 | 512 | 768 | 1280 => Storage::UnsignedInteger,
                    256 | 4 | 8 | 1024 => Storage::SignedInteger,
                    16 | 64 => Storage::Float,
                    _ => Storage::Complex,
                },
                stored_bits: bits,
                unit: if *view == ComplexView::PhaseRadians {
                    Unit::Radians
                } else {
                    Unit::Unknown
                },
                rescale: Some(Rescale { slope, intercept }),
                resampled: false,
            };
            let choices = VoiChoices::new(window.into_iter().collect(), Vec::new())
                .map_err(|_| LoadError::Unsupported)?;
            let scalar = ScalarData::new(info, choices).map_err(|_| LoadError::Unsupported)?;
            headers.push(Header {
                format: Format::Nifti(Nifti {
                    datatype,
                    slope,
                    intercept,
                    step,
                    window,
                    view: NiftiView {
                        version,
                        indices,
                        dimensions: std::array::from_fn(|i| dims[i + 3] as u32),
                        complex: *view,
                    },
                    metadata_bytes: if headers.is_empty() { size as u64 } else { 0 },
                }),
                offsets: OnceLock::new(),
                deflated_at: None,
                offset: offset + volume * length,
                pixel_length: Some(length),
                width: dims[0] as u32,
                height: dims[1] as u32,
                frames: dims[2] as u32,
                native: true,
                bits,
                big_endian: big,
                geometry: geometry.clone(),
                frame_spatial: Vec::new(),
                acquisition: None,
                scalar: crate::dicom_voi::Presentations::single(scalar),
                key: format!("nifti-{id}-{volume}-{view:?}"),
                modality: "NIFTI",
                series_number: (headers.len() + 1) as i32,
                instance: 0,
            });
        }
    }
    Ok(headers)
}
