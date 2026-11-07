// SPDX-License-Identifier: GPL-3.0-or-later
//! Explicit workbench stamps over physical source-cell centres, with private preview.
use super::{LabelUi, Operation};
use eframe::egui;
use orv_core::{
    labels::{BrushAction, BrushShape, Labels, PhysicalBrush, PhysicalStroke},
    volume::Volume,
};
use std::sync::Arc;
mod gesture;
pub(super) use gesture::{InputPath, Path};

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Shape {
    #[default]
    Sphere,
    Plane,
}
#[derive(Clone)]
struct Input {
    base: Arc<Labels>,
    brush: PhysicalBrush,
    point: [f64; 3],
    series: usize,
    generation: u64,
}
impl Input {
    fn same(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.base, &other.base)
            && self.brush == other.brush
            && self.point == other.point
            && self.series == other.series
            && self.generation == other.generation
    }
}
pub(super) struct Preview {
    pub stroke: PhysicalStroke,
    pub epoch: u64,
}
pub(super) struct Physical {
    pub open: bool,
    shape: Shape,
    erase: bool,
    radius: f64,
    thickness: f64,
    input: Option<Input>,
    epoch: u64,
    pub pending: bool,
    preview: Option<Preview>,
    path: Option<Arc<Path>>,
    drawing: bool,
    draw_requested: bool,
    gesture: Option<gesture::Capture>,
}
impl Default for Physical {
    fn default() -> Self {
        Self {
            open: false,
            shape: Shape::Sphere,
            erase: false,
            radius: 5.,
            thickness: 1.,
            input: None,
            epoch: 0,
            pending: false,
            preview: None,
            path: None,
            drawing: false,
            draw_requested: false,
            gesture: None,
        }
    }
}
impl Physical {
    fn invalidate(&mut self) {
        self.epoch = self.epoch.wrapping_add(1);
        self.preview = None;
    }
    fn accept(&self, preview: &Preview, map: &Arc<Labels>) -> bool {
        self.epoch == preview.epoch
            && self.input.as_ref().is_some_and(|input| {
                Arc::ptr_eq(&input.base, map)
                    && preview
                        .stroke
                        .is_current(map, &input.brush, self.points(input))
            })
    }
    fn points<'a>(&'a self, input: &'a Input) -> &'a [[f64; 3]] {
        self.path
            .as_ref()
            .map_or_else(|| std::slice::from_ref(&input.point), |path| path.points())
    }
    pub fn completed(&mut self, preview: Preview, map: Option<&Arc<Labels>>) -> bool {
        if map.is_some_and(|map| self.accept(&preview, map)) {
            self.preview = Some(preview);
            true
        } else {
            false
        }
    }
    pub(super) fn displayed<'a>(&'a self, map: &'a Arc<Labels>) -> Option<&'a Arc<Labels>> {
        self.preview
            .as_ref()
            .filter(|preview| self.accept(preview, map))
            .map(|preview| preview.stroke.preview())
    }
}

/// A completed thin MPR context. Pending navigation has no edit authority.
pub(crate) struct PhysicalContext<'a> {
    pub source: &'a Arc<Volume>,
    pub selected: usize,
    pub point: [f64; 3],
    pub plane: usize,
    pub generation: u64,
    pub ready: bool,
}
impl LabelUi {
    pub fn open_physical_tools(&mut self) {
        self.paint = false;
        self.stroke.clear();
        self.regions.seeded.cancel_stroke();
        self.regions.picking = false;
        self.regions.seed_picking = None;
        self.regions.invalidate();
        self.open = false;
        self.visible = true;
        self.physical.open = true;
    }
    pub(crate) fn select_manual_for_physical(&mut self) {
        self.supplied_active = false;
    }
    pub fn physical_dialog_open(&self) -> bool {
        self.physical.open
    }
    pub fn end_physical_tools(&mut self) {
        self.invalidate_physical();
        self.physical.open = false;
        self.physical.input = None;
        self.physical.drawing = false;
        self.physical.draw_requested = false;
    }
    pub(super) fn invalidate_physical(&mut self) {
        self.physical.path = None;
        self.physical.gesture = None;
        self.invalidate_physical_proposal();
    }
    fn invalidate_physical_proposal(&mut self) {
        if self.physical.pending {
            self.cancel();
        }
        self.physical.invalidate();
    }
    pub fn sync_physical(&mut self, context: &PhysicalContext<'_>) {
        let input = self
            .current
            .as_ref()
            .filter(|map| {
                context.ready
                    && self.series == context.selected
                    && !self.supplied_active
                    && !self.open
                    && (!self.busy() || self.physical.pending)
                    && Arc::ptr_eq(&map.source_arc(), context.source)
            })
            .map(|map| Input {
                base: map.clone(),
                point: context.point,
                series: context.selected,
                generation: context.generation,
                brush: PhysicalBrush {
                    label: self.label,
                    action: if self.physical.erase {
                        BrushAction::Erase
                    } else {
                        BrushAction::Add
                    },
                    radius_mm: self.physical.radius,
                    shape: if self.physical.shape == Shape::Sphere {
                        BrushShape::Sphere
                    } else {
                        let axis = [2, 1, 0][context.plane.min(2)];
                        BrushShape::Plane {
                            origin_mm: context.point,
                            normal: std::array::from_fn(
                                |index| if axis == index { 1. } else { 0. },
                            ),
                            thickness_mm: self.physical.thickness,
                        }
                    },
                    clip: None,
                    // Background has no overlay visibility; hiding all displayed
                    // IDs must not turn empty source cells into a protected ID.
                    protected_ids: std::array::from_fn(|id| {
                        id != 0 && !self.display.visible_ids[id]
                    }),
                },
            });
        let unchanged = match (&self.physical.input, &input) {
            (Some(old), Some(new)) => old.same(new),
            (None, None) => true,
            _ => false,
        };
        if !unchanged {
            self.invalidate_physical();
            self.physical.input = input;
        }
    }
    pub fn physical_preview_visible(&self) -> bool {
        self.current
            .as_ref()
            .is_some_and(|map| self.physical.displayed(map).is_some())
    }
    pub(super) fn displayed_manual<'a>(&'a self, map: &'a Arc<Labels>) -> &'a Arc<Labels> {
        self.physical
            .displayed(map)
            .unwrap_or_else(|| self.regions.displayed(map))
    }
    fn preview_physical(&mut self, ctx: &egui::Context) {
        self.invalidate_physical();
        self.submit_physical(ctx);
    }
    fn submit_physical(&mut self, ctx: &egui::Context) {
        if self.busy() {
            return;
        }
        let Some(input) = self.physical.input.clone() else {
            return;
        };
        if input.brush.protected_ids[self.label as usize] {
            return;
        }
        self.invalidate_physical_proposal();
        self.visible = true;
        self.physical.pending = true;
        self.start(
            Operation::Physical(
                input.base,
                Box::new(input.brush),
                self.physical.path.clone().map_or(
                    gesture::InputPath::Point(input.point),
                    gesture::InputPath::Trace,
                ),
                self.physical.epoch,
            ),
            ctx,
        );
    }
    fn apply_physical(&mut self) {
        if self.busy() {
            return;
        }
        let Some(map) = self.current.clone() else {
            return;
        };
        let Some(preview) = self.physical.preview.take() else {
            return;
        };
        if !self.physical.accept(&preview, &map) {
            self.message = "Physical edit inputs changed. Request a new preview.".into();
            return;
        }
        let Some(input) = &self.physical.input else {
            return;
        };
        match preview
            .stroke
            .apply(&map, &input.brush, self.physical.points(input), || false)
        {
            Ok(labels) => self.commit_change(labels),
            Err(error) => self.message = error.into(),
        }
    }
    pub fn physical_window(&mut self, ctx: &egui::Context, context: &PhysicalContext<'_>) {
        self.sync_physical(context);
        let mut open = self.physical.open;
        let mut draw = false;
        crate::dialogs::escape(ctx, &mut open);
        crate::dialogs::window("Physical label edit", ctx).open(&mut open).default_width(440.).show(ctx, |ui| {
            ui.label("Preview a sphere or a thick disk at the linked MPR point, or sweep it along a drawn MPR path. This explicit volume tool can reach hidden cells; it is not a visible-surface brush.");
            if self.current.is_none() {
                ui.label("Create manual labels for this volume. Supplied masks remain read-only.");
                if ui.add_enabled(!self.busy(), egui::Button::new("Create blank workbench labels")).clicked() {
                    self.series = context.selected;
                    self.start(Operation::StartVolume(context.source.clone()), ctx);
                }
            } else if self.series != context.selected || self.current.as_ref().is_some_and(|map| !Arc::ptr_eq(&map.source_arc(), context.source)) {
                ui.label("Manual labels belong to another source. Return to that source or remove the map in Labels… first.");
            } else {
                ui.add_enabled_ui(!self.busy(), |ui| {
                    crate::toolbar::row(ui, |ui| {
                        ui.selectable_value(&mut self.physical.shape, Shape::Sphere, "Sphere");
                        ui.selectable_value(&mut self.physical.shape, Shape::Plane, "Plane disk");
                        ui.checkbox(&mut self.physical.erase, "Erase selected ID");
                    });
                    crate::toolbar::row(ui, |ui| {
                        crate::toolbar::number(ui, "Label ID", egui::DragValue::new(&mut self.label).range(1..=255));
                        crate::toolbar::number(ui, "Radius (mm)", egui::DragValue::new(&mut self.physical.radius).range(0.001..=256.).speed(0.1).max_decimals(3));
                    });
                    if self.physical.shape == Shape::Plane {
                        ui.label(format!("Disk lies in the active {} plane.", ["axial", "coronal", "sagittal"][context.plane.min(2)]));
                        crate::toolbar::number(ui, "Thickness (mm)", egui::DragValue::new(&mut self.physical.thickness).range(0.001..=512.).speed(0.1).max_decimals(3));
                    }
                    ui.small(format!("Centre: L {:.3}, P {:.3}, S {:.3} mm. Move the linked point or use Point coordinates to position it.", context.point[0], context.point[1], context.point[2]));
                    ui.small("Cell centres determine membership. Missing source samples, hidden IDs and other labels are preserved. This brush uses the source extent, independently of 3D display clipping.");
                    self.sync_physical(context);
                    if !context.ready { ui.label("Wait for completed thin MPR planes. Slab projections cannot position this tool."); }
                    if !self.display.visible_ids[self.label as usize] { ui.label("The selected ID is hidden and protected. Show it in Labels… before editing."); }
                    if let Some(path) = &self.physical.path {
                        ui.small(format!("Drawn path: {} input points joined by a continuous physical sweep.", path.points().len()));
                    }
                    let point_action = if self.physical.path.is_some() { "Preview linked point instead" } else { "Preview physical edit" };
                    if ui.add_enabled(self.physical.input.is_some() && self.display.visible_ids[self.label as usize], egui::Button::new(point_action)).clicked() { self.preview_physical(ctx); }
                    if ui.add_enabled(self.physical.input.is_some() && self.display.visible_ids[self.label as usize], egui::Button::new("Draw physical path on MPR")).clicked() { draw = true; }
                    ui.small("Draw across one thin MPR plane, then release to preview the continuous physical footprint. Right / Space drag navigates; Escape stops drawing.");
                    if let Some(preview) = &self.physical.preview {
                        let summary = preview.stroke.summary();
                        ui.label(format!("PREVIEW ONLY · {} cells would change in {} tiles.", summary.changed_voxels, summary.changed_tiles));
                        ui.small("The planes and manual 3D region show this proposal. Counts and export use the applied map. Apply adds one undo step; nothing is saved automatically.");
                        ui.horizontal_wrapped(|ui| {
                            if ui.button("Apply physical edit").clicked() { self.apply_physical(); }
                            if ui.button("Discard physical preview").clicked() { self.invalidate_physical(); }
                        });
                    }
                    ui.horizontal_wrapped(|ui| {
                        if ui.add_enabled(!self.undo.is_empty(), egui::Button::new("Undo physical / label edit")).clicked() { self.undo_edit(); }
                        if ui.add_enabled(!self.redo.is_empty(), egui::Button::new("Redo label edit")).clicked() { self.redo_edit(); }
                    });
                });
            }
            if self.busy() { ui.horizontal(|ui| { ui.spinner(); ui.label("Working locally…"); if ui.button("Cancel label work").clicked() { self.cancel(); } }); }
            if !self.message.is_empty() { ui.label(&self.message); }
            ui.small("Moving the MPR point, changing edit inputs or leaving this layout discards the proposal. Closing this window keeps it available for inspection.");
            crate::dialogs::reveal_focus(ui);
        });
        self.physical.open = open && !draw;
        if draw {
            self.invalidate_physical();
            self.physical.drawing = true;
            self.physical.draw_requested = true;
        }
        self.sync_physical(context);
    }
}

#[cfg(test)]
mod tests;
