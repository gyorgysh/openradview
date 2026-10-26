// SPDX-License-Identifier: GPL-3.0-or-later
//! Source-grid user labels, deliberately excluded from renderer/MCP snapshots.
mod canvas;
#[cfg(test)]
mod tests;
mod worker;
use eframe::egui;
use orv_core::{labels::Labels, study::Series};
use std::{collections::VecDeque, sync::Arc};
use worker::{Operation, Output, Task};

pub struct LabelUi {
    pub open: bool,
    pub dirty: bool,
    pub series: usize,
    pub(super) current: Option<Arc<Labels>>,
    undo: VecDeque<Arc<Labels>>,
    task: Option<Task>,
    message: String,
    visible: bool,
    opacity: f32,
    paint: bool,
    erase: bool,
    label: u8,
    radius: u32,
    range: [f32; 2],
    all_slices: bool,
    texture: Option<(usize, egui::TextureHandle)>,
    stroke: Vec<[f64; 2]>,
    pub discard_question: bool,
    pending_overlay: Option<(usize, egui::ColorImage)>,
    stroke_slice: Option<usize>,
}
impl Default for LabelUi {
    fn default() -> Self {
        Self {
            open: false,
            dirty: false,
            series: 0,
            current: None,
            undo: VecDeque::new(),
            task: None,
            message: String::new(),
            visible: true,
            opacity: 0.4,
            paint: false,
            erase: false,
            label: 1,
            radius: 3,
            range: [0., 100.],
            all_slices: false,
            texture: None,
            stroke: Vec::new(),
            discard_question: false,
            pending_overlay: None,
            stroke_slice: None,
        }
    }
}
impl LabelUi {
    pub fn poll(&mut self) {
        let result = self.task.as_ref().and_then(Task::poll);
        let Some(result) = result else { return };
        self.task = None;
        match result {
            Ok(Output::Start(labels)) => {
                self.current = Some(labels);
                self.message = "Blank labels ready. Paint on the 2D source image.".into();
            }
            Ok(Output::Change(labels)) => {
                if let Some(old) = self.current.replace(labels) {
                    self.undo.push_back(old);
                }
                while self.undo.len() > 2 {
                    self.undo.pop_front();
                }
                self.texture = None;
                self.dirty = true;
                self.message = "Labels changed in memory. Export to keep them.".into();
            }
            Ok(Output::Saved) => {
                self.dirty = false;
                self.message =
                    "Label map exported. Source images and identifiers were not included.".into();
            }
            Ok(Output::Cancelled) => {
                self.message = "Cancelled; labels unchanged.".into();
            }
            Ok(Output::Overlay(z, image)) => {
                self.pending_overlay = Some((z, image));
            }
            Err(error) => self.message = error,
        }
    }
    fn start(&mut self, op: Operation, ctx: &egui::Context) {
        if self.task.is_none() {
            self.stroke.clear();
            self.task = Some(Task::start(op, ctx.clone()));
        }
    }
    pub fn busy(&self) -> bool {
        self.task.is_some()
    }
    pub fn cancel(&self) {
        if let Some(task) = &self.task {
            task.cancel();
        }
    }
    pub fn stop_painting(&mut self) {
        self.paint = false;
        self.stroke.clear();
    }
    pub fn painting(&self, selected: usize) -> bool {
        self.paint && self.current.is_some() && self.series == selected && !self.open
    }
    pub fn window(
        &mut self,
        ctx: &egui::Context,
        selected: usize,
        series: Option<Arc<Series>>,
        slice: Option<usize>,
    ) {
        self.poll();
        if let Some((z, image)) = self.pending_overlay.take() {
            self.texture = Some((
                z,
                ctx.load_texture("user-label-overlay", image, egui::TextureOptions::NEAREST),
            ));
        }
        let mut open = self.open;
        crate::dialogs::window("Local labels",ctx).open(&mut open).default_width(440.).show(ctx,|ui| {
            ui.label("Draw or load your own labels. No model or anatomical inference is used.");
            ui.label("Labels appear only on the 2D source grid and are excluded from agent snapshots. Keep a matching copy of the original images.");
            if self.task.is_some() {ui.horizontal(|ui| {ui.spinner();ui.label("Working locally…");if ui.button("Cancel").clicked() && let Some(task)=&self.task{task.cancel();}});}
            ui.add_enabled_ui(self.task.is_none(),|ui| {
                if self.current.is_none() {
                    ui.label("Requires a regular volume, up to 32 million voxels and 4 million pixels per slice.");
                    if ui.add_enabled(series.is_some(),egui::Button::new("Create blank labels")).clicked() && let Some(series)=series {self.series=selected;self.start(Operation::Start(series),ctx);}
                    return;
                }
                if ui.button("Remove this label map…").clicked() {self.discard_question=true;}
                if selected!=self.series {ui.label(format!("Labels belong to series {} in the list. Return there to edit or export them, or remove this map.",self.series+1));return;}
                ui.horizontal(|ui| {ui.checkbox(&mut self.visible,"Show overlay");ui.add(egui::Slider::new(&mut self.opacity,0.05..=0.9).text("Opacity"));});
                ui.horizontal(|ui| {if ui.checkbox(&mut self.paint,"Brush enabled").changed() && self.paint {self.visible=true;}ui.checkbox(&mut self.erase,"Erase");});
                ui.label("Close this panel to paint. Escape turns the brush off. Right-drag pans; wheel zooms.");
                ui.horizontal(|ui| {ui.add(egui::DragValue::new(&mut self.label).range(1..=255).prefix("Label "));ui.add(egui::DragValue::new(&mut self.radius).range(1..=64).prefix("Radius ").suffix(" px"));});
                if ui.add_enabled(!self.undo.is_empty(),egui::Button::new("Undo last edit")).clicked() && let Some(previous)=self.undo.pop_back() {self.current=Some(previous);self.texture=None;self.dirty=true;}
                ui.collapsing("Threshold fill (scalar images)",|ui| {
                    ui.label("Replace this label within a value range; preserve other labels. Values follow the selected source representation.");
                    ui.horizontal(|ui| {ui.add(egui::DragValue::new(&mut self.range[0]).prefix("From "));ui.add(egui::DragValue::new(&mut self.range[1]).prefix("To "));});
                    ui.checkbox(&mut self.all_slices,"All source slices");
                    if ui.add_enabled(slice.is_some() && self.current.as_ref().is_some_and(|l|!l.source().is_color()),egui::Button::new("Apply threshold")).clicked() && let Some(current)=self.current.clone(){self.start(Operation::Threshold(current,if self.all_slices{None}else{slice},self.range,self.label),ctx);}
                });
                ui.separator();
                ui.label("Import requires matching dimensions, affine and spatial reference. Matching numbers do not prove the same anatomy; verify the overlay yourself. No registration or resampling is performed.");
                ui.horizontal(|ui| {
                    if ui.button("Load label map…").clicked() && let Some(current)=self.current.clone(){self.start(Operation::Import(current),ctx);}
                    if ui.button("Export labels…").clicked() && let Some(current)=self.current.clone(){self.start(Operation::Export(current),ctx);}
                });
                ui.label("Export: NIfTI-2 uint8, IDs 0–255. Zero is background. Two previous edits can be undone. Nothing is saved automatically.");
            });
            if !self.message.is_empty(){ui.separator();ui.label(&self.message);}
        });
        self.open = open;
    }
}
