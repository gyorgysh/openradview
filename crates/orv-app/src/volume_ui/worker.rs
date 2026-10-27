// SPDX-License-Identifier: GPL-3.0-or-later
use eframe::{egui, egui_wgpu::wgpu};
use orv_core::volume::Volume;
use orv_render::volume::VolumeRenderer;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::{Receiver, sync_channel},
};

pub(super) struct Ready {
    pub volume: Arc<Volume>,
    pub renderer: VolumeRenderer,
    pub source_range: Option<super::DisplayRange>,
}
pub(super) struct Upload {
    pub receiver: Receiver<Result<Ready, String>>,
    cancel: Arc<AtomicBool>,
}
impl Upload {
    pub fn start(
        frames: impl Into<crate::frame_source::FrameSource>,
        device: wgpu::Device,
        queue: wgpu::Queue,
        format: wgpu::TextureFormat,
        ctx: egui::Context,
    ) -> Self {
        let frames = frames.into();
        let (tx, receiver) = sync_channel(1);
        let cancel = Arc::new(AtomicBool::new(false));
        let worker = cancel.clone();
        let fallback = tx.clone();
        let repaint = ctx.clone();
        let spawn = std::thread::Builder::new()
            .name("orv-volume-upload".into())
            .spawn(move || {
                let result = (|| {
                    orv_render::volume::check_limits(&device.limits()).map_err(str::to_owned)?;
                    let volume = frames.volume(|| worker.load(Ordering::Acquire))?;
                    let source_range = source_range(&volume, || worker.load(Ordering::Acquire))?;
                    let renderer =
                        VolumeRenderer::new(&device, &queue, volume.clone(), format, || {
                            worker.load(Ordering::Acquire)
                        })
                        .map_err(str::to_owned)?;
                    Ok(Ready {
                        volume,
                        renderer,
                        source_range,
                    })
                })();
                if !worker.load(Ordering::Acquire) {
                    let _ = tx.send(result);
                    ctx.request_repaint();
                }
            });
        if spawn.is_err() {
            let _ = fallback.send(Err("Could not start the volume worker".into()));
            repaint.request_repaint();
        }
        Self { receiver, cancel }
    }
}
impl Drop for Upload {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}

// Runs on the upload worker: a neutral initial presentation, not tissue inference.
fn source_range(
    volume: &Volume,
    cancelled: impl Fn() -> bool,
) -> Result<Option<super::DisplayRange>, String> {
    use orv_core::{
        WindowLevel,
        volume_view::{TransferFunction, TransferShape},
    };
    if volume.is_color() {
        return Ok(Some((
            WindowLevel {
                function: Default::default(),
                width: 256.,
                center: 128.,
            },
            TransferFunction {
                low: 0.,
                high: 255.,
                opacity_per_mm: 0.05,
                shape: TransferShape::Ramp,
            },
        )));
    }
    let (mut min, mut max) = (f32::INFINITY, f32::NEG_INFINITY);
    for frame in volume.frames() {
        for (block_index, block) in frame.pixels.chunks(16384).enumerate() {
            if cancelled() {
                return Err("Cancelled".into());
            }
            for (offset, value) in block.iter().enumerate() {
                if frame.validity.is_some() && !frame.has_sample(block_index * 16384 + offset) {
                    continue;
                }
                min = min.min(*value);
                max = max.max(*value);
            }
        }
    }
    if !min.is_finite() || !max.is_finite() {
        return Ok(None);
    }
    let window = WindowLevel::source_range(min, max);
    let transfer = TransferFunction {
        low: min,
        high: if max > min {
            max
        } else {
            min + 1f32.max(min.abs() * f32::EPSILON * 4.)
        },
        opacity_per_mm: 0.05,
        shape: TransferShape::Ramp,
    };
    Ok((window.is_valid() && transfer.validate().is_ok()).then_some((window, transfer)))
}

#[cfg(test)]
mod range_tests {
    use super::*;
    use orv_core::{
        WindowLevel,
        study::{Frame, Geometry},
    };
    #[test]
    fn automatic_3d_source_range_ignores_declared_padding() {
        for (padding, scale) in [(-32768., 1.), (32767., 1.), (-32768., 0.025)] {
            let frames = (0..2)
                .map(|z| {
                    Arc::new(Frame {
                        scalar: None,
                        validity: Some([0, 1, 1, 1].into()),
                        color: None,
                        nifti: None,
                        rgba: None,
                        width: 2,
                        height: 2,
                        pixels: vec![padding, 10. * scale, 20. * scale, 30. * scale].into(),
                        geometry: Some(Geometry {
                            nifti_transform: None,
                            frame_of_reference: Some("synthetic-range".into()),
                            position: [0., 0., f64::from(z)],
                            orientation: [1., 0., 0., 0., 1., 0.],
                            spacing: [1.; 2],
                        }),
                        instance: z,
                        frame_index: 0,
                        monochrome1: false,
                        default_window: WindowLevel::BONE,
                    })
                })
                .collect();
            let volume = Volume::new(frames).unwrap();
            let (window, transfer) = source_range(&volume, || false).unwrap().unwrap();
            assert_eq!((window.width, window.center), (20. * scale, 20. * scale));
            assert_eq!((transfer.low, transfer.high), (10. * scale, 30. * scale));
            for (value, expected) in [(10., 0.), (20., 0.5), (30., 1.)] {
                assert_eq!(window.normalize(value * scale), expected);
                assert_eq!(transfer.weight(value * scale), expected);
            }
        }
    }
}
