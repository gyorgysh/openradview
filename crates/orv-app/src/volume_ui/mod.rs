// SPDX-License-Identifier: GPL-3.0-or-later
//! Native 3D controls; validation, upload and shaders never block the UI thread.
mod callback;
mod controls;
mod input;
mod sharing;
#[cfg(all(test, feature = "gpu-tests"))]
mod tests;
mod worker;
use callback::{Callback, Gpu};
use eframe::egui::{self, RichText, Vec2};
use orv_core::{
    ViewState,
    volume::Volume,
    volume_view::{Projection, TransferPreset, VolumeView},
};
use std::sync::{Arc, Mutex, mpsc::TryRecvError};
use worker::Upload;
type DisplayRange = (
    orv_core::WindowLevel,
    orv_core::volume_view::TransferFunction,
);

pub struct VolumeSession {
    upload: Option<Upload>,
    pending_frames: Option<crate::frame_source::FrameSource>,
    volume: Option<Arc<Volume>>,
    gpu: Option<Arc<Mutex<Gpu>>>,
    state: Option<VolumeView>,
    display: ViewState,
    error: Option<String>,
    preset: TransferPreset,
    needs_fit: bool,
    prefer_source: bool,
    source_range: Option<DisplayRange>,
    initial_display: DisplayRange,
}
impl VolumeSession {
    pub(crate) fn picture(&self) -> Option<(crate::export::Pixels, [u32; 2])> {
        let (spec, true) = self.shared_view()? else {
            return None;
        };
        Some((
            crate::export::Pixels::Volume(self.volume.as_ref()?.clone()),
            spec.preferred_size?,
        ))
    }
    pub fn start(frames: impl Into<crate::frame_source::FrameSource>) -> Self {
        let frames = frames.into();
        let prefer_source = matches!(&frames,crate::frame_source::FrameSource::Indexed(series) if series.modality!="CT");
        let initial_display = TransferPreset::Bone.values();
        let window = initial_display.0;
        Self {
            upload: None,
            pending_frames: Some(frames),
            prefer_source,
            source_range: None,
            initial_display,
            volume: None,
            gpu: None,
            state: None,
            display: ViewState {
                window,
                ..Default::default()
            },
            error: None,
            preset: TransferPreset::Bone,
            needs_fit: true,
        }
    }
    pub(crate) fn start_upload(
        &mut self,
        device: eframe::egui_wgpu::wgpu::Device,
        queue: eframe::egui_wgpu::wgpu::Queue,
        format: eframe::egui_wgpu::wgpu::TextureFormat,
        ctx: egui::Context,
    ) {
        if let Some(frames) = self.pending_frames.take() {
            self.upload = Some(Upload::start(frames, device, queue, format, ctx));
        }
    }
    pub(crate) fn poll(&mut self) {
        let result = self.upload.as_ref().map(|u| u.receiver.try_recv());
        match result {
            Some(Ok(Ok(ready))) => {
                self.prefer_source |= ready.volume.is_color();
                if self.prefer_source
                    && let Some(range) = &ready.source_range
                {
                    self.initial_display = range.clone();
                }
                if self.state.is_none() {
                    let mut state = VolumeView::fit(&ready.volume);
                    state.projection = Projection::Composite;
                    if self.prefer_source {
                        let Some(range) = ready.source_range.as_ref() else {
                            self.error=Some("The source intensity range exceeds the 3D display limits. Use 2D/MPR.".into());
                            self.upload = None;
                            return;
                        };
                        self.display.window = range.0;
                        state.transfer = range.1.clone();
                        self.initial_display = range.clone();
                    }
                    state.camera.yaw_degrees = 25.;
                    state.camera.pitch_degrees = 15.;
                    self.state = Some(state);
                }
                self.source_range = ready.source_range;
                self.volume = Some(ready.volume);
                self.gpu = Some(Arc::new(Mutex::new(Gpu::new(ready.renderer))));
                self.upload = None;
            }
            Some(Ok(Err(error))) => {
                self.error = Some(error);
                self.upload = None;
            }
            Some(Err(TryRecvError::Disconnected)) => {
                self.error =
                    Some("The volume worker stopped. Return to 2D/MPR and try again.".into());
                self.upload = None;
            }
            _ => (),
        }
    }
    pub fn ui(&mut self, ui: &mut egui::Ui, frame: &eframe::Frame) {
        if self.pending_frames.is_some() {
            if let Some(render) = frame.wgpu_render_state() {
                self.start_upload(
                    render.device.clone(),
                    render.queue.clone(),
                    render.target_format,
                    ui.ctx().clone(),
                );
            } else {
                self.error = Some("3D requires the native GPU renderer".into());
                self.pending_frames = None;
            }
        }
        self.poll();
        let content_width = ui.available_width();
        if let Some(error) = &self.error {
            ui.heading("This volume could not be prepared");
            ui.label(error);
            ui.label("The source images remain available in 2D/MPR.");
            return;
        }
        let (Some(volume), Some(state), Some(gpu)) = (&self.volume, &mut self.state, &self.gpu)
        else {
            ui.add_space(30.);
            ui.label("Checking geometry and uploading image data on a background worker…");
            ui.label("Images stay on this device. Choose 2D/MPR to cancel.");
            return;
        };
        crate::view_context::show(ui, &volume.frames()[0]);
        if controls::show(
            ui,
            state,
            &mut self.display,
            &mut self.preset,
            controls::Context {
                source_range: self.source_range.as_ref(),
                initial: &self.initial_display,
                color: volume.is_color(),
                choices: volume.frames()[0]
                    .scalar
                    .as_ref()
                    .map(|data| &**data.choices()),
                bounds: volume.bounds(),
            },
        ) {
            self.needs_fit = true;
        }
        if ui.is_enabled() && !ui.ctx().egui_wants_keyboard_input() {
            ui.input(|i| {
                if i.key_pressed(egui::Key::R) {
                    self.display.zoom = 1.;
                    self.display.pan = [0.; 2];
                    self.needs_fit = true;
                }
                if i.key_pressed(egui::Key::I) {
                    self.display.invert = !self.display.invert;
                }
            });
        }
        let (rect, response) = ui.allocate_exact_size(
            Vec2::new(content_width, (ui.available_height() - 72.).max(60.)),
            egui::Sense::click_and_drag(),
        );
        let (interacting, fit) = input::interact(ui, &response, state, &mut self.display);
        self.needs_fit |= fit;
        if self.needs_fit {
            if let Err(error) = state
                .camera
                .fit_to_volume(volume, f64::from(rect.width() / rect.height()))
            {
                self.error = Some(error.into());
                return;
            }
            self.needs_fit = false;
        }
        let pixels = rect.size() * ui.ctx().pixels_per_point();
        ui.painter_at(rect)
            .add(eframe::egui_wgpu::Callback::new_paint_callback(
                rect,
                Callback {
                    gpu: gpu.clone(),
                    display: self.display.clone(),
                    state: state.clone(),
                    pixels,
                    interacting,
                    ctx: ui.ctx().clone(),
                },
            ));
        if let Ok(gpu) = gpu.try_lock() {
            if let Some(error) = gpu.error {
                ui.label(error);
            } else {
                let status = if state.quality == orv_core::volume_view::Quality::Interactive
                    && gpu.completed(&self.display, state)
                {
                    "Preview quality".into()
                } else if gpu.refined {
                    "Refined".into()
                } else {
                    format!("Interactive preview · refining {:.0}%", gpu.progress * 100.)
                };
                let [x, y, z] = volume.dimensions();
                ui.label(
                    RichText::new(format!(
                        "{status} · {x} × {y} × {z} source samples · {}%",
                        (self.display.zoom * 100.).round()
                    ))
                    .small()
                    .color(crate::theme::SILVER),
                );
            }
        }
        ui.label(RichText::new("Drag: orbit · Space / right / middle drag: move · Scroll/pinch: zoom · LPS coordinates · Experimental research projection").small().color(crate::theme::SILVER));
    }
}
fn preset_name(preset: TransferPreset) -> &'static str {
    match preset {
        TransferPreset::Bone => "Bone",
        TransferPreset::SoftTissue => "Soft tissue",
        TransferPreset::Lung => "Lung",
    }
}
