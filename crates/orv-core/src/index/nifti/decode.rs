// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use crate::{nifti_view::ComplexView, study::Frame};
use std::sync::Arc;
impl Nifti {
    pub fn decode(
        &self,
        source: &Source,
        header: &Header,
        frame: u32,
        state: &mut ReadState,
        cancelled: &impl Fn() -> bool,
    ) -> Result<Arc<Frame>, LoadError> {
        let size = header.width as usize * header.height as usize * (header.bits / 8) as usize;
        let bytes = source.range(
            header.offset + size as u64 * frame as u64,
            size,
            state,
            cancelled,
        )?;
        let mut pixels = Vec::with_capacity(header.width as usize * header.height as usize);
        for (index, chunk) in bytes.chunks_exact((header.bits / 8) as usize).enumerate() {
            if index.is_multiple_of(16384) && cancelled() {
                return Err(LoadError::Cancelled);
            }
            let raw = |part: &[u8]| {
                let mut value = [0u8; 8];
                for (i, b) in part.iter().enumerate() {
                    value[if header.big_endian {
                        part.len() - 1 - i
                    } else {
                        i
                    }] = *b;
                }
                u64::from_le_bytes(value)
            };
            let scale = |v: f64| v * self.slope + self.intercept;
            let value = if matches!(self.datatype, 32 | 1792) {
                let (a, b) = chunk.split_at(chunk.len() / 2);
                let float = |v: &[u8]| {
                    if self.datatype == 32 {
                        f32::from_bits(raw(v) as u32) as f64
                    } else {
                        f64::from_bits(raw(v))
                    }
                };
                // NIfTI specifies the same scaling for both real/imaginary parts.
                let (re, im) = (scale(float(a)), scale(float(b)));
                if !re.is_finite() || !im.is_finite() {
                    return Err(LoadError::Unsupported);
                }
                match self.view.complex {
                    ComplexView::Magnitude => re.hypot(im),
                    ComplexView::PhaseRadians => im.atan2(re),
                    ComplexView::Real => re,
                    ComplexView::Imaginary => im,
                    ComplexView::Scalar => return Err(LoadError::Unsupported),
                }
            } else {
                let v = raw(chunk);
                scale(match self.datatype {
                    2 => v as u8 as f64,
                    256 => v as i8 as f64,
                    4 => v as i16 as f64,
                    512 => v as u16 as f64,
                    8 => v as i32 as f64,
                    768 => v as u32 as f64,
                    1024 => v as i64 as f64,
                    1280 => v as f64,
                    16 => f32::from_bits(v as u32) as f64,
                    64 => f64::from_bits(v),
                    _ => return Err(LoadError::Unsupported),
                })
            } as f32;
            if !value.is_finite() {
                return Err(LoadError::Unsupported);
            }
            pixels.push(value);
        }
        let min = pixels.iter().copied().fold(f32::INFINITY, f32::min);
        let max = pixels.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let window = self
            .window
            .unwrap_or_else(|| WindowLevel::source_range(min, max));
        if !window.is_valid() {
            return Err(LoadError::Unsupported);
        }
        let geometry = self.frame_geometry(header, frame);
        Ok(Arc::new(Frame {
            scalar: header.scalar.get(frame).cloned(),
            validity: None,
            color: None,
            nifti: Some(self.view.clone()),
            rgba: None,
            width: header.width,
            height: header.height,
            pixels: pixels.into(),
            geometry,
            instance: 0,
            frame_index: frame,
            monochrome1: false,
            default_window: window,
        }))
    }
}
