// SPDX-License-Identifier: GPL-3.0-or-later
use super::{LabelUi, Operation};
use eframe::egui;
use orv_core::labels::{LabelScope, Labels, RegionOperation, SelectionRequest, SelectionSummary};
use std::sync::{Arc, Weak};

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Choice {
    #[default]
    Grow,
    Keep,
    RemoveSmall,
}
impl Choice {
    fn name(self) -> &'static str {
        match self {
            Self::Grow => "Grow from seed",
            Self::Keep => "Keep seeded component",
            Self::RemoveSmall => "Remove small islands",
        }
    }
}
pub(super) struct Preview {
    pub base: Arc<Labels>,
    pub labels: Arc<Labels>,
    pub request: SelectionRequest,
    pub summary: SelectionSummary,
    pub epoch: u64,
}
#[derive(Default)]
pub(super) struct Regions {
    choice: Choice,
    volume: bool,
    range: [f32; 2],
    minimum: usize,
    seed: Option<[usize; 3]>,
    typed_seed: [usize; 2],
    pub picking: bool,
    pub preview: Option<Preview>,
    pub epoch: u64,
    pub seeded: super::seeded::Seeded,
    pub seed_picking: Option<orv_core::labels::SeedClass>,
    inputs: Option<(Weak<Labels>, Option<SelectionRequest>)>,
}
impl Regions {
    pub fn new() -> Self {
        Self {
            range: [0., 100.],
            minimum: 10,
            typed_seed: [1, 1],
            ..Default::default()
        }
    }
    pub fn invalidate(&mut self) {
        self.epoch = self.epoch.wrapping_add(1);
        self.preview = None;
        self.seeded.invalidate();
    }
    fn request(&self, label: u8, slice: Option<usize>) -> Option<SelectionRequest> {
        let z = slice?;
        let scope = if self.volume {
            LabelScope::Volume
        } else {
            LabelScope::Slice(z)
        };
        let operation = match self.choice {
            Choice::Grow | Choice::Keep => {
                let seed = self.seed?;
                if !self.volume && seed[2] != z {
                    return None;
                }
                if self.choice == Choice::Grow {
                    RegionOperation::Grow {
                        seed,
                        range: self.range,
                    }
                } else {
                    RegionOperation::KeepComponent { seed }
                }
            }
            Choice::RemoveSmall => RegionOperation::RemoveSmall {
                minimum_voxels: self.minimum,
            },
        };
        Some(SelectionRequest {
            scope,
            label,
            operation,
        })
    }
    pub fn sync(&mut self, map: &Arc<Labels>, label: u8, slice: Option<usize>) -> bool {
        let key = Arc::downgrade(map);
        let request = self.request(label, slice);
        if self
            .inputs
            .as_ref()
            .is_none_or(|(old, input)| !old.ptr_eq(&key) || *input != request)
        {
            self.invalidate();
            self.inputs = Some((key, request));
        }
        let cancelled = self.seeded.sync(map, label, slice);
        if cancelled {
            self.picking = false;
            self.seed_picking = None;
            self.invalidate();
        }
        cancelled
    }
    pub fn pick(&mut self, seed: [usize; 3]) {
        if let Some(class) = self.seed_picking.take() {
            self.seeded.add(seed, class);
            self.picking = false;
            self.invalidate();
            return;
        }
        self.seed = Some(seed);
        self.typed_seed = [seed[0] + 1, seed[1] + 1];
        self.picking = false;
        self.invalidate();
    }
    pub fn accept(&self, preview: &Preview, map: &Arc<Labels>) -> bool {
        preview.epoch == self.epoch
            && Arc::ptr_eq(&preview.base, map)
            && self
                .inputs
                .as_ref()
                .is_some_and(|(_, request)| *request == Some(preview.request))
    }
    pub fn displayed<'a>(&'a self, map: &'a Arc<Labels>) -> &'a Arc<Labels> {
        if let Some(preview) = self
            .seeded
            .preview
            .as_ref()
            .filter(|p| self.seeded.accept(p, map))
        {
            return &preview.labels;
        }
        self.preview
            .as_ref()
            .filter(|p| self.accept(p, map))
            .map_or(map, |p| &p.labels)
    }
}
impl LabelUi {
    pub(super) fn region_controls(
        &mut self,
        ui: &mut egui::Ui,
        slice: Option<usize>,
        ctx: &egui::Context,
    ) {
        let Some(map) = self.current.clone() else {
            return;
        };
        self.sync_regions(&map, self.label, slice);
        ui.collapsing("Connected regions", |ui| {
            ui.label("Use source values and neighboring cells; this does not recognize shapes, organs or disease. Preview first, then explicitly apply one undoable edit.");
            egui::ComboBox::from_id_salt("region-operation").selected_text(self.regions.choice.name()).show_ui(ui, |ui| {
                for choice in [Choice::Grow, Choice::Keep, Choice::RemoveSmall] {
                    if ui.selectable_value(&mut self.regions.choice, choice, choice.name()).clicked() { ui.close(); }
                }
            });
            ui.checkbox(&mut self.regions.volume, "Whole source volume (6 neighbors)");
            if !self.regions.volume { ui.small("Current source image only (4 neighbors). Diagonal contacts do not connect."); }
            else { ui.small("Face neighbors only, including adjacent source slices. Diagonal contacts do not connect."); }
            ui.small("Missing source samples and other label IDs are barriers. Cleanup preserves stored labels on missing samples.");
            ui.add_enabled_ui(slice.is_some(), |ui| {
                match self.regions.choice {
                    Choice::Grow => {
                        ui.label("Add connected background cells in this inclusive scalar interval to the selected label. Existing cells of that label can connect the region; they are retained.");
                        ui.horizontal(|ui| {
                            ui.add(egui::DragValue::new(&mut self.regions.range[0]).prefix("Minimum "));
                            ui.add(egui::DragValue::new(&mut self.regions.range[1]).prefix("Maximum "));
                        });
                    }
                    Choice::Keep => { ui.label("Keep the seeded component of the selected label; remove its other valid cells within scope."); }
                    Choice::RemoveSmall => {
                        ui.label("Remove components of the selected label with fewer cells than this cutoff. Equal-size components stay.");
                        ui.add(egui::DragValue::new(&mut self.regions.minimum).range(1..=orv_core::labels::MAX_VOXELS).prefix("Minimum cells "));
                    }
                }
                if self.regions.choice != Choice::RemoveSmall {
                    if ui.button("Pick seed on source image").clicked() {
                        self.paint = false; self.clear_brush();
                        self.regions.seed_picking = None;
                        self.regions.invalidate(); self.regions.picking = true; self.open = false;
                    }
                    ui.small("Click once on the source image; this panel reopens. Escape cancels. Or enter a column and row below (1-based).");
                    let [w,h,_] = map.source().dimensions();
                    ui.horizontal(|ui| {
                        ui.add(egui::DragValue::new(&mut self.regions.typed_seed[0]).range(1..=w).prefix("Column "));
                        ui.add(egui::DragValue::new(&mut self.regions.typed_seed[1]).range(1..=h).prefix("Row "));
                    });
                    if ui.button("Use typed seed on this image").clicked() && let Some(z) = slice {
                        let [x,y] = self.regions.typed_seed;
                        self.regions.pick([x.saturating_sub(1),y.saturating_sub(1),z]);
                    }
                    if let Some([x,y,z]) = self.regions.seed { ui.small(format!("Seed: column {}, row {}, source-grid image {}",x+1,y+1,z+1)); }
                    else { ui.small("Choose a seed before previewing."); }
                }
            });
            self.sync_regions(&map, self.label, slice);
            let request = self.regions.request(self.label, slice);
            let scalar_allowed = self.regions.choice != Choice::Grow || !map.source().is_color();
            if !scalar_allowed { ui.small("Grow from seed requires scalar source images."); }
            if ui.add_enabled(request.is_some() && scalar_allowed, egui::Button::new("Preview connected edit")).clicked() && let Some(request) = request {
                self.paint = false;
                self.regions.invalidate();
                self.visible = true;
                self.display.visible_ids[self.label as usize] = true;
                self.start(Operation::Preview(map.clone(), request, self.regions.epoch), ctx);
            }
            if let Some(preview) = &self.regions.preview {
                let verb = if matches!(preview.request.operation, RegionOperation::Grow { .. }) { "add" } else { "remove" };
                ui.label(format!("PREVIEW ONLY · {verb} {} cells in label {}. Region: {} cells; {} component(s).", preview.summary.changed_voxels, preview.request.label, preview.summary.region_voxels, preview.summary.components));
                ui.weak("The overlay shows the proposed map. Counts below and exports use the applied map. Nothing is saved by previewing.");
                let changes = preview.summary.changed_voxels > 0;
                ui.horizontal(|ui| {
                    if ui.add_enabled(changes, egui::Button::new("Apply connected edit")).clicked() { self.apply_region_preview(slice); }
                    if ui.button("Discard preview").clicked() { self.regions.invalidate(); }
                });
            }
        });
        self.sync_regions(&map, self.label, slice);
    }
    pub(super) fn apply_region_preview(&mut self, slice: Option<usize>) {
        let Some(map) = self.current.clone() else {
            return;
        };
        self.sync_regions(&map, self.label, slice);
        if let Some(preview) = self.regions.preview.take()
            && self.regions.accept(&preview, &map)
            && preview.summary.changed_voxels > 0
        {
            self.commit_change(preview.labels);
        }
    }
    pub fn pointer_hint(&self) -> &'static str {
        if self.regions.picking {
            match self.regions.seed_picking {
                Some(orv_core::labels::SeedClass::Include) => {
                    "Draw an include hint or click once · Escape to cancel"
                }
                Some(orv_core::labels::SeedClass::Exclude) => {
                    "Draw an exclude hint or click once · Escape to cancel"
                }
                None => "Choose a source seed · click once · Escape to stop",
            }
        } else {
            "Label brush active · Escape to stop"
        }
    }
    pub fn pointer_controls(&self) -> &'static str {
        if self.regions.picking && self.regions.seed_picking.is_some() {
            "Hint: draw or click · Wheel / pinch: zoom · Right / Space drag: move · ↑↓: images · Escape: cancel"
        } else if self.regions.picking {
            "Seed: click once · Wheel / pinch: zoom · Right / Space drag: move · ↑↓: images · Escape: stop"
        } else {
            "Brush: draw labels · Wheel / pinch: zoom · Right / Space drag: move · ↑↓: images · Escape: stop"
        }
    }
    pub fn region_preview_visible(&self) -> bool {
        self.current.as_ref().is_some_and(|map| {
            self.regions
                .preview
                .as_ref()
                .is_some_and(|p| self.regions.accept(p, map))
                || self
                    .regions
                    .seeded
                    .preview
                    .as_ref()
                    .is_some_and(|p| self.regions.seeded.accept(p, map))
        })
    }
}

#[cfg(test)]
mod tests;
