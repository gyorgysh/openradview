// SPDX-License-Identifier: GPL-3.0-or-later
//! Explicit local picture export. No networking, automatic saves or UI readback.
#[cfg(test)]
mod tests;
mod worker;
use eframe::egui;
use orv_core::{
    scene::SceneState,
    study::{Frame, Series},
    volume::Volume,
};
use std::sync::Arc;
use worker::Task;

pub(crate) enum Pixels {
    Image(Arc<Frame>),
    Volume(Arc<Volume>),
    Reformat(Arc<Frame>, Arc<Volume>),
}
pub(crate) struct Input {
    pub series: Arc<Series>,
    pub scene: SceneState,
    pub pixels: Pixels,
    pub device: eframe::egui_wgpu::wgpu::Device,
    pub queue: eframe::egui_wgpu::wgpu::Queue,
}

#[derive(Default)]
pub(super) struct PictureExport {
    open: bool,
    input: Option<Input>,
    task: Option<Task>,
    notice: Option<&'static str>,
}
impl PictureExport {
    pub fn is_open(&self) -> bool {
        self.open
    }
    pub fn open(&mut self, input: Input) {
        self.close();
        self.input = Some(input);
        self.open = true;
    }
    pub fn close(&mut self) {
        *self = Self::default();
    }
    pub fn show(&mut self, ctx: &egui::Context) {
        if !self.open {
            return;
        }
        if let Some(result) = self.task.as_ref().and_then(Task::poll) {
            self.notice = Some(result.unwrap_or_else(|message| message));
            self.task = None;
        }
        let mut open = self.open;
        crate::dialogs::window("Export picture", ctx).open(&mut open).default_width(490.).show(ctx, |ui| {
            ui.spacing_mut().item_spacing = egui::vec2(8., 8.);
            ui.label("Save an image-only PNG and its view settings to a new folder on this computer.");
            ui.label("The current pane is captured. Measurements, crosshairs, scales, labels and other app controls are excluded. This is a rendered picture, not the original dataset.");
            ui.label("Identifying text can remain inside the image. The settings contain source fingerprints and geometry, without source paths or patient tags. Review both files before sharing them.");
            if let Some(input) = &self.input {
                let [w,h] = input.scene.output_size;
                ui.weak(format!("{w} × {h} pixels · displayed composition · personal education only"));
            }
            if self.task.is_some() {
                ui.label("Preparing picture and checking the source locally…");
                ui.weak("Large series take longer. Source files are never modified.");
                if ui.button("Cancel export").clicked() { self.close(); }
            } else if let Some(input) = self.input.take() {
                if ui.button("Choose folder and save…").clicked() {
                    self.task = Some(Task::start(input, ctx.clone()));
                    self.notice = None;
                } else { self.input = Some(input); }
            }
            if let Some(notice) = self.notice { ui.label(notice); }
        });
        if !open {
            self.close();
        }
    }
}

impl crate::workspace::Workspace {
    pub(super) fn open_picture_export(&mut self, frame: &eframe::Frame) {
        self.pause_playback();
        let Some((spec, true)) = self.rendered_view() else {
            return;
        };
        let Some(series) = self
            .study
            .as_ref()
            .and_then(|s| s.series.get(self.selected))
        else {
            return;
        };
        let Some(gpu) = frame.wgpu_render_state() else {
            return;
        };
        let captured = match self.mode {
            crate::workspace::ViewMode::Acquired => self
                .current_image()
                .and_then(|(_, f)| Some((Pixels::Image(f), self.picture_size?))),
            crate::workspace::ViewMode::Mpr => self.mpr.as_ref().and_then(|m| m.picture()),
            crate::workspace::ViewMode::Oblique => self.oblique.as_ref().and_then(|m| m.picture()),
            crate::workspace::ViewMode::Volume => self.volume.as_ref().and_then(|m| m.picture()),
        };
        let Some((pixels, output_size)) = captured else {
            return;
        };
        self.export.open(Input {
            series: Arc::new(series.clone()),
            scene: SceneState {
                schema_version: if spec.reconstruction.is_some() { 2 } else { 1 },
                source_sha256: "0".repeat(64), // replaced by worker before rendering/saving
                series_index: self.selected as u32,
                view: spec.view,
                crosshair_mm: spec.point,
                reformat_side: spec.reformat_side,
                output_size,
                reconstruction: spec.reconstruction,
            },
            pixels,
            device: gpu.device.clone(),
            queue: gpu.queue.clone(),
        });
    }
}
