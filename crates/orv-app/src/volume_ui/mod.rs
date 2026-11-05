// SPDX-License-Identifier: GPL-3.0-or-later
//! Native 3D controls; validation, upload and shaders never block the UI thread.
mod callback;
mod controls;
mod input;
mod material;
mod regions;
mod sharing;
#[cfg(all(test, feature = "gpu-tests"))]
mod tests;
mod workbench;
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
    pub(super) material_enabled: bool,
    material_editor: Option<material::MaterialEditor>,
    material_error: Option<&'static str>,
    regions: regions::Regions,
    settings_open: bool,
    cursor_visible: bool,
}
impl VolumeSession {
    pub(crate) fn controls_open(&self) -> bool {
        self.settings_open
            || self
                .material_editor
                .as_ref()
                .is_some_and(|editor| editor.open)
    }
    pub(crate) fn local_scene(&self) -> bool {
        self.material_enabled || self.regions.enabled()
    }
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
        let mut session = Self::new(prefer_source);
        session.pending_frames = Some(frames);
        session
    }
    fn new(prefer_source: bool) -> Self {
        let initial_display = TransferPreset::Bone.values();
        let window = initial_display.0;
        Self {
            upload: None,
            pending_frames: None,
            prefer_source,
            source_range: None,
            initial_display,
            material_enabled: false,
            material_editor: None,
            material_error: None,
            regions: Default::default(),
            settings_open: false,
            cursor_visible: true,
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
                self.regions.bind(ready.source, ready.format);
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
                let mut gpu = Gpu::new(ready.renderer);
                if let Some(renderer) = ready.material_renderer {
                    match renderer.and_then(|renderer| {
                        material::MaterialEditor::new(&self.initial_display)
                            .map(|editor| (renderer, editor))
                    }) {
                        Ok((renderer, editor)) => {
                            gpu.material_renderer = Some(renderer);
                            self.material_editor = Some(editor);
                        }
                        Err(error) => self.material_error = Some(error),
                    }
                }
                self.gpu = Some(Arc::new(Mutex::new(gpu)));
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
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        frame: &eframe::Frame,
        labels: &mut crate::labels::LabelUi,
        selected: usize,
    ) {
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
        let Some(volume) = &self.volume else {
            ui.add_space(30.);
            ui.label("Checking geometry and uploading image data on a background worker…");
            ui.label("Images stay on this device. Choose 2D/MPR to cancel.");
            return;
        };
        crate::view_context::show(ui, &volume.frames()[0]);
        self.show_controls(ui, labels, selected, true);
        if let Some(editor) = &mut self.material_editor {
            editor.window(ui.ctx());
        }
        let Some(volume) = self.volume.as_ref() else {
            return;
        };
        let region_selection = self.regions.selection(labels, selected, volume);
        if crate::actions::pressed(ui, crate::actions::Action::Fit) {
            self.display.zoom = 1.;
            self.display.pan = [0.; 2];
            self.needs_fit = true;
        }
        if !self.material_enabled && crate::actions::pressed(ui, crate::actions::Action::Invert) {
            self.display.invert = !self.display.invert;
        }
        let compact = ui.ctx().content_rect().height() < 700.;
        let bottom_space = if compact { 32. } else { 72. };
        let (rect, response) = ui.allocate_exact_size(
            Vec2::new(
                content_width,
                (ui.available_height() - bottom_space).max(60.),
            ),
            egui::Sense::click_and_drag(),
        );
        self.paint_viewport(ui, rect, &response, region_selection, false);
        if !compact {
            ui.label(RichText::new("Drag: orbit · Space / right / middle drag: move · Scroll/pinch: zoom · LPS coordinates · Experimental research projection").small().color(crate::theme::SILVER));
        }
    }
    fn paint_viewport(
        &mut self,
        ui: &mut egui::Ui,
        rect: egui::Rect,
        response: &egui::Response,
        region_selection: Option<crate::labels::RegionSelection>,
        embedded: bool,
    ) -> Option<(ViewState, VolumeView, [u32; 2])> {
        let (Some(volume), Some(state), Some(gpu)) = (&self.volume, &mut self.state, &self.gpu)
        else {
            return None;
        };
        let (interacting, fit) = input::interact(ui, response, state, &mut self.display);
        self.needs_fit |= fit;
        if self.needs_fit {
            if let Err(error) = state
                .camera
                .fit_to_volume(volume, f64::from(rect.width() / rect.height()))
            {
                self.error = Some(error.into());
                return None;
            }
            self.needs_fit = false;
        }
        let pixels = rect.size() * ui.ctx().pixels_per_point();
        let material = self
            .material_enabled
            .then(|| self.material_editor.as_ref().map(|editor| editor.style()))
            .flatten();
        let region_mode = self.regions.enabled();
        let painted = if let (Some(selection), Some(style)) = (region_selection, material.as_ref())
        {
            self.regions.show(
                ui,
                rect,
                regions::Scene {
                    selection,
                    display: self.display.clone(),
                    state: state.clone(),
                    style: style.clone(),
                },
                interacting,
            )
        } else {
            false
        };
        if !painted {
            ui.painter_at(rect)
                .add(eframe::egui_wgpu::Callback::new_paint_callback(
                    rect,
                    Callback {
                        gpu: gpu.clone(),
                        display: self.display.clone(),
                        state: state.clone(),
                        pixels,
                        interacting,
                        material: material.clone(),
                        ctx: ui.ctx().clone(),
                    },
                ));
        }
        let mut shown = if painted {
            self.regions.displayed()
        } else {
            None
        };
        if !painted && let Ok(gpu) = gpu.try_lock() {
            shown = gpu.displayed().filter(|(display, camera, size)| {
                *display == self.display
                    && *camera == *state
                    && *size
                        == callback::resolution(
                            pixels,
                            if gpu.refined {
                                if material.is_some() { 2048. } else { 768. }
                            } else if material.is_some() {
                                512.
                            } else {
                                256.
                            },
                        )
            });
        }
        if !region_mode && let Ok(gpu) = gpu.try_lock() {
            if let Some(error) = gpu.error {
                ui.label(error);
            } else {
                let status = if state.quality == orv_core::volume_view::Quality::Interactive
                    && gpu.completed_style(&self.display, state, material.as_ref())
                {
                    "Preview quality".into()
                } else if gpu.refined {
                    "Refined".into()
                } else {
                    format!("Interactive preview · refining {:.0}%", gpu.progress * 100.)
                };
                let [x, y, z] = volume.dimensions();
                let style = if self.material_enabled {
                    "Material · local · "
                } else {
                    ""
                };
                ui.label(
                    RichText::new(if embedded {
                        format!("{style}{status}")
                    } else {
                        format!(
                            "{style}{status} · {x} × {y} × {z} source samples · {}%",
                            (self.display.zoom * 100.).round()
                        )
                    })
                    .small()
                    .color(crate::theme::SILVER),
                );
            }
        }
        shown
    }
    fn show_controls(
        &mut self,
        ui: &mut egui::Ui,
        labels: &mut crate::labels::LabelUi,
        selected: usize,
        shortcuts: bool,
    ) {
        let (Some(volume), Some(state)) = (&self.volume, &mut self.state) else {
            return;
        };
        let was_material = self.material_enabled;
        if self.material_enabled {
            state.projection = Projection::Composite;
            if let Some(editor) = &mut self.material_editor {
                self.needs_fit |= editor.controls(
                    ui,
                    state,
                    &mut self.display,
                    volume.bounds(),
                    &mut self.material_enabled,
                    shortcuts,
                );
            }
        } else if controls::show(
            ui,
            state,
            &mut self.display,
            &mut self.preset,
            controls::Context {
                shortcuts,
                source_range: self.source_range.as_ref(),
                initial: &self.initial_display,
                color: volume.is_color(),
                choices: volume.frames()[0]
                    .scalar
                    .as_ref()
                    .map(|data| &**data.choices()),
                bounds: volume.bounds(),
                material_selection: self
                    .material_editor
                    .as_ref()
                    .map(|_| &mut self.material_enabled),
            },
        ) {
            self.needs_fit = true;
        }
        if was_material && !self.material_enabled {
            self.regions.disable();
        }
        if self.material_editor.is_some() {
            self.regions
                .controls(ui, labels, selected, volume, &mut self.material_enabled);
        } else {
            self.regions.disable();
        }
        if self.material_enabled {
            state.projection = Projection::Composite;
        }
        if let Some(error) = self.material_error {
            ui.label(format!("Material view unavailable: {error}"));
        }
    }
}
fn preset_name(preset: TransferPreset) -> &'static str {
    match preset {
        TransferPreset::Bone => "Bone",
        TransferPreset::SoftTissue => "Soft tissue",
        TransferPreset::Lung => "Lung",
    }
}
