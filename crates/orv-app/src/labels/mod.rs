// SPDX-License-Identifier: GPL-3.0-or-later
//! Source-grid user labels, deliberately excluded from renderer/MCP snapshots.
mod canvas;
mod counts;
mod display;
mod illustration;
mod local_volume;
use crate::jobs;
pub(crate) use illustration::{CapturedMasks, MaskDescription};
pub(crate) use local_volume::{RegionMap, RegionMaps, RegionSelection};
mod overlay;
mod physical;
pub(crate) use physical::PhysicalContext;
mod regions;
mod supplied;
pub(crate) use canvas::overlay_mesh;
pub(crate) use overlay::OverlayView;
#[cfg(test)]
mod tests;
mod worker;
use eframe::egui;
use orv_core::{
    labels::{LabelObjectId, LabelRevision, Labels},
    study::Series,
};
use std::{collections::VecDeque, sync::Arc};
use worker::{Operation, Output, Task};

const HISTORY_STEPS: usize = 16;

pub struct LabelUi {
    pub open: bool,
    pub dirty: bool,
    pub series: usize,
    pub(super) current: Option<Arc<Labels>>,
    undo: VecDeque<Arc<Labels>>,
    redo: VecDeque<Arc<Labels>>,
    saved: Option<(LabelObjectId, LabelRevision)>,
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
    overlays: overlay::Overlays,
    counts: counts::Counts,
    display: overlay::Display,
    regions: regions::Regions,
    physical: physical::Physical,
    supplied: supplied::Supplied,
    supplied_active: bool,
    stroke: Vec<[f64; 2]>,
    pub discard_question: bool,
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
            redo: VecDeque::new(),
            saved: None,
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
            overlays: Default::default(),
            counts: Default::default(),
            display: Default::default(),
            regions: regions::Regions::new(),
            physical: Default::default(),
            supplied: Default::default(),
            supplied_active: false,
            stroke: Vec::new(),
            discard_question: false,
            stroke_slice: None,
        }
    }
}
impl LabelUi {
    pub fn poll(&mut self) {
        self.supplied.poll();
        let result = self.task.as_ref().and_then(Task::poll);
        let Some(result) = result else { return };
        self.task = None;
        self.physical.pending = false;
        match result {
            Ok(Output::Start(labels)) => {
                self.current = Some(labels);
                self.mark_saved();
                self.message =
                    "Blank labels ready. Choose a source-image brush or a physical workbench edit."
                        .into();
            }
            Ok(Output::Change(labels)) => {
                self.commit_change(labels);
            }
            Ok(Output::Preview(preview)) => {
                if self
                    .current
                    .as_ref()
                    .is_some_and(|map| self.regions.accept(&preview, map))
                {
                    self.regions.preview = Some(*preview);
                    // The proposal section owns its status; it vanishes when inputs change.
                    self.message.clear();
                } else {
                    self.message = "Selection inputs changed. Request a new preview.".into();
                }
            }
            Ok(Output::Physical(preview)) => {
                self.message = if self.physical.completed(*preview, self.current.as_ref()) {
                    String::new()
                } else {
                    "Physical edit inputs changed. Request a new preview.".into()
                };
            }
            Ok(Output::Saved) => {
                self.mark_saved();
                self.message =
                    "Label map exported. Source images and identifiers were not included.".into();
            }
            Ok(Output::Cancelled) => {
                self.message = "Cancelled; labels unchanged.".into();
            }
            Err(error) => self.message = error,
        }
    }
    fn commit_change(&mut self, labels: Arc<Labels>) {
        if self.current.as_ref().is_some_and(|current| {
            current.object_id() == labels.object_id() && current.revision() == labels.revision()
        }) {
            self.message = "No label cells changed.".into();
            return;
        }
        if let Some(old) = self.current.replace(labels) {
            self.undo.push_back(old);
        }
        while self.undo.len() > HISTORY_STEPS {
            self.undo.pop_front();
        }
        self.redo.clear();
        self.changed_revision();
        self.message = "Labels changed in memory. Export to keep them.".into();
    }
    fn mark_saved(&mut self) {
        self.saved = self
            .current
            .as_ref()
            .map(|map| (map.object_id().clone(), map.revision().clone()));
        self.dirty = false;
    }
    fn changed_revision(&mut self) {
        self.invalidate_physical();
        self.regions.invalidate();
        self.regions.picking = false;
        self.stroke.clear();
        self.stroke_slice = None;
        self.overlays.clear();
        self.dirty = self.current.as_ref().is_some_and(|map| {
            self.saved.as_ref().is_none_or(|(object, revision)| {
                object != map.object_id() || revision != map.revision()
            })
        });
    }
    fn undo_edit(&mut self) {
        if self.busy() {
            return;
        }
        if let Some(previous) = self.undo.pop_back() {
            if let Some(current) = self.current.replace(previous) {
                self.redo.push_back(current);
            }
            self.changed_revision();
            self.message = "Edit undone. Redo is available until the next change.".into();
        }
    }
    fn redo_edit(&mut self) {
        if self.busy() {
            return;
        }
        if let Some(next) = self.redo.pop_back() {
            if let Some(current) = self.current.replace(next) {
                self.undo.push_back(current);
            }
            self.changed_revision();
            self.message = "Edit restored.".into();
        }
    }
    fn start(&mut self, op: Operation, ctx: &egui::Context) {
        if self.task.is_none() {
            if !matches!(&op, Operation::Physical(..)) {
                self.invalidate_physical();
            }
            if !matches!(&op, Operation::Preview(..)) {
                self.regions.invalidate();
            }
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
    pub fn discard_manual(&mut self) {
        let supplied = std::mem::take(&mut self.supplied);
        let supplied_active = self.supplied_active;
        *self = Self {
            supplied,
            supplied_active,
            ..Default::default()
        };
    }
    pub fn stop_painting(&mut self) {
        self.end_physical_tools();
        self.paint = false;
        self.stroke.clear();
        self.regions.picking = false;
        self.regions.invalidate();
    }
    pub fn painting(&self, selected: usize) -> bool {
        !self.supplied_active
            && (self.paint || self.regions.picking)
            && self.current.is_some()
            && self.series == selected
            && !self.open
    }
    pub fn window(
        &mut self,
        ctx: &egui::Context,
        selected: usize,
        series: Option<Arc<Series>>,
        slice: Option<usize>,
        demo_mask: Option<&'static [u8]>,
    ) {
        self.poll();
        if let Some(map) = &self.current {
            self.regions.sync(
                map,
                self.label,
                if selected == self.series { slice } else { None },
            );
        }
        let mut open = self.open;
        crate::dialogs::window("Labels and supplied masks",ctx).open(&mut open).default_width(500.).default_height(620.).vscroll(true).show(ctx,|ui| {
            let previous_group = self.supplied_active;
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.supplied_active, false, "Manual labels");
                ui.selectable_value(&mut self.supplied_active, true, "Supplied masks");
            });
            if previous_group != self.supplied_active {
                self.stop_painting();
                self.clear_overlays();
            }
            ui.separator();
            if self.supplied_active {
                self.supplied.controls(ui, selected, series, demo_mask, ctx);
                return;
            }
            ui.label("Draw or load your own labels. No model or anatomical inference is used.");
            ui.label("Labels appear on source images and thin MPR/oblique planes. In 3D, choose Region in 3D for one opaque ID over a scalar material view. Painting stays on the source grid. Slabs and agent snapshots exclude labels. Keep a matching copy of the original images.");
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
                if slice.is_some() {
                ui.horizontal(|ui| {if ui.checkbox(&mut self.paint,"Brush enabled").changed() && self.paint {self.visible=true;self.regions.picking=false;self.regions.invalidate();}ui.checkbox(&mut self.erase,"Erase");});
                ui.label("Close this panel to paint. Escape turns the brush off. Right-drag pans; wheel zooms.");
                ui.horizontal(|ui| {ui.add(egui::DragValue::new(&mut self.label).range(1..=255).prefix("Label "));ui.add(egui::DragValue::new(&mut self.radius).range(1..=64).prefix("Radius ").suffix(" px"));});
                } else {ui.weak("Return to 2D image for painting, threshold fill or connected-region previews.");}
                ui.horizontal(|ui| {
                    if ui.add_enabled(!self.undo.is_empty(),egui::Button::new("Undo last edit")).clicked() {self.undo_edit();}
                    if ui.add_enabled(!self.redo.is_empty(),egui::Button::new("Redo edit")).clicked() {self.redo_edit();}
                });
                if slice.is_some() {ui.collapsing("Threshold fill (scalar images)",|ui| {
                    ui.label("Replace this label within a value range; preserve other labels. Values follow the selected source representation.");
                    ui.horizontal(|ui| {ui.add(egui::DragValue::new(&mut self.range[0]).prefix("From "));ui.add(egui::DragValue::new(&mut self.range[1]).prefix("To "));});
                    ui.checkbox(&mut self.all_slices,"All source slices");
                    if ui.add_enabled(slice.is_some() && self.current.as_ref().is_some_and(|l|!l.source().is_color()),egui::Button::new("Apply threshold")).clicked() && let Some(current)=self.current.clone(){self.start(Operation::Threshold(current,if self.all_slices{None}else{slice},self.range,self.label),ctx);}
                });}
                self.region_controls(ui,slice,ctx);
                self.display_controls(ui,ctx);
                ui.separator();
                ui.label("Import requires matching dimensions, affine and spatial reference. Matching numbers do not prove the same anatomy; verify the overlay yourself. Import does not register or resample the map.");
                ui.horizontal(|ui| {
                    if ui.button("Load label map…").clicked() && let Some(current)=self.current.clone(){self.start(Operation::Import(current),ctx);}
                    if ui.button("Export labels…").clicked() && let Some(current)=self.current.clone(){self.start(Operation::Export(current),ctx);}
                });
                ui.label("Export: NIfTI-2 uint8, IDs 0–255. Zero is background. Up to 16 edits can be undone and redone while the map is open. A new change clears redo. Nothing is saved automatically.");
            });
            if !self.message.is_empty(){ui.separator();ui.label(&self.message);}
        });
        self.open = open && !self.regions.picking;
    }
}
