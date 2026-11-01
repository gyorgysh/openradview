// SPDX-License-Identifier: GPL-3.0-or-later
//! Read-only supplied masks have no manual-label edits, undo or automatic attachment.
pub(in crate::labels) mod overlay;
use super::{
    OverlayView,
    jobs::Worker,
    overlay::{Mode, Overlays},
};
use eframe::egui;
use orv_core::{
    ViewState,
    segments::Segments,
    study::{Frame, Series},
    volume::Volume,
};
use std::sync::Arc;

pub(super) struct Supplied {
    current: Option<Arc<Segments>>,
    series: usize,
    worker: Option<Worker<Arc<Series>, Option<Arc<Segments>>>>,
    pending: Option<(u64, usize)>,
    overlays: Overlays<Segments>,
    display: overlay::Display,
    visible: bool,
    opacity: f32,
    message: String,
}
impl Default for Supplied {
    fn default() -> Self {
        Self {
            current: None,
            series: 0,
            worker: None,
            pending: None,
            overlays: Default::default(),
            display: Default::default(),
            visible: true,
            opacity: 0.5,
            message: String::new(),
        }
    }
}
impl Supplied {
    pub(super) fn illustration_masks(
        &self,
        selected: usize,
    ) -> Result<Option<super::CapturedMasks>, &'static str> {
        if !self.visible || self.series != selected || self.display.visible == 0 {
            return Ok(None);
        }
        let Some(map) = &self.current else {
            return Ok(None);
        };
        if self.pending.is_some() {
            return Err("Wait for mask loading to finish, then reopen export");
        }
        super::CapturedMasks::supplied(map.clone(), self.display.clone(), self.opacity).map(Some)
    }
    pub fn poll(&mut self) {
        let Some((generation, result)) = self.worker.as_ref().and_then(Worker::take) else {
            return;
        };
        self.finish(generation, result);
    }
    fn finish(&mut self, generation: u64, result: Result<Option<Arc<Segments>>, String>) {
        let Some((expected, series)) = self.pending else {
            return;
        };
        if generation != expected {
            return;
        }
        self.pending = None;
        match result {
            Ok(Some(masks)) => {
                self.current = Some(masks);
                self.series = series;
                self.display = Default::default();
                self.visible = true;
                self.overlays.clear();
                self.message = "Supplied masks loaded in memory. Inspect their alignment with the source images.".into();
            }
            Ok(None) => self.message = "Cancelled; supplied masks unchanged.".into(),
            Err(error) => self.message = error,
        }
    }
    fn start(&mut self, series: Arc<Series>, selected: usize, ctx: &egui::Context) {
        if self.worker.is_none() {
            match Worker::start(ctx.clone(), "orv-supplied-mask-import", read) {
                Ok(worker) => self.worker = Some(worker),
                Err(error) => {
                    self.message = error;
                    return;
                }
            }
        }
        if let Some(worker) = &self.worker {
            self.pending = Some((worker.submit(series), selected));
            self.message.clear();
        }
    }
    fn cancel(&mut self) {
        self.pending = None;
        if let Some(worker) = &self.worker {
            worker.clear();
        }
    }
    pub fn clear_overlays(&mut self) {
        self.overlays.clear();
    }
    pub fn controls(
        &mut self,
        ui: &mut egui::Ui,
        selected: usize,
        series: Option<Arc<Series>>,
        ctx: &egui::Context,
    ) {
        ui.label("Inspect supplied masks on the selected DICOM series. No model is run.");
        if self.pending.is_some() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Reading masks locally…");
                if ui.button("Cancel mask import").clicked() {
                    self.cancel();
                    self.message = "Cancelled; supplied masks unchanged.".into();
                }
            });
        }
        ui.add_enabled_ui(self.pending.is_none(), |ui| {
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(
                        series.is_some(),
                        egui::Button::new("Load matching DICOM masks…"),
                    )
                    .clicked()
                    && let Some(series) = series
                {
                    self.start(series, selected, ctx);
                }
                if self.current.is_some() && ui.button("Remove masks").clicked() {
                    self.current = None;
                    self.overlays.clear();
                    self.message = "Supplied masks removed. Manual labels are unchanged.".into();
                }
            })
        });
        if let Some(current) = &self.current {
            if selected != self.series {
                self.overlays.clear();
                ui.label(format!("These masks belong to series {} in the list. Return there to inspect them, or load a matching set for the selected series.", self.series + 1));
            } else {
                ui.horizontal(|ui| {
                    ui.checkbox(&mut self.visible, "Show supplied overlay");
                    ui.add(egui::Slider::new(&mut self.opacity, 0.05..=0.9).text("Opacity"));
                });
                ui.horizontal_wrapped(|ui| {
                    ui.label("Display");
                    egui::ComboBox::from_id_salt("supplied-mask-style")
                        .selected_text(match self.display.mode {
                            Mode::Fill => "Fill",
                            Mode::Outline => "Outline",
                            Mode::Hatched => "Hatched",
                        })
                        .show_ui(ui, |ui| {
                            for (mode, name) in [
                                (Mode::Fill, "Fill"),
                                (Mode::Outline, "Outline"),
                                (Mode::Hatched, "Hatched"),
                            ] {
                                if ui
                                    .selectable_value(&mut self.display.mode, mode, name)
                                    .clicked()
                                {
                                    ui.close();
                                }
                            }
                        });
                    if ui.small_button("Show all").clicked() {
                        self.display.visible = u64::MAX;
                    }
                    if ui.small_button("Hide all").clicked() {
                        self.display.visible = 0;
                    }
                });
                let source = current.source_arc();
                let volume = source.voxel_volume_mm3();
                let depth = source.dimensions()[2];
                ui.label(format!("{} overlapping source cells. Segment volumes share these cells; do not add them.", current.overlap_cells()));
                egui::ScrollArea::vertical()
                    .id_salt("supplied-mask-counts")
                    .max_height(240.)
                    .show(ui, |ui| {
                        for (index, info) in current.infos().enumerate() {
                            let flag = 1u64 << index;
                            let mut visible = self.display.visible & flag != 0;
                            let total = volume * info.positive_cells as f64;
                            let amount =
                                if !total.is_finite() || !volume.is_finite() || volume <= 0. {
                                    "volume unavailable".into()
                                } else if total > 0. && total < 0.001 {
                                    format!("≈{total:.3e} mm³")
                                } else {
                                    format!("≈{total:.3} mm³")
                                };
                            ui.horizontal_wrapped(|ui| {
                                if ui
                                    .checkbox(&mut visible, format!("ID {}", info.number))
                                    .changed()
                                {
                                    if visible {
                                        self.display.visible |= flag;
                                    } else {
                                        self.display.visible &= !flag;
                                    }
                                }
                                ui.small(format!("{} voxels · {amount}", info.positive_cells));
                            });
                            ui.small(format!(
                                "{} valid · {} missing · {}/{depth} source planes supplied",
                                info.valid_source_cells,
                                info.positive_cells - info.valid_source_cells,
                                info.supplied_planes
                            ));
                        }
                    });
                ui.weak("Crosshatching marks overlapping visible masks. Hide an ID to inspect it separately. Counts include stored positives on missing samples; omitted planes are unknown, not supplied empty masks.");
            }
        }
        if let Some(error) = &self.overlays.error {
            ui.label(error);
            if ui.small_button("Retry supplied overlay").clicked() {
                self.overlays.clear();
            }
        }
        if !self.message.is_empty() {
            ui.separator();
            ui.label(&self.message);
        }
        ui.separator();
        ui.small("Read-only on source images and thin MPR/oblique planes. Slabs, 3D and agent snapshots exclude masks. Nothing is saved automatically.");
        ui.collapsing("Supported mask inputs", |ui| {
            ui.small("Choose native uncompressed little-endian BINARY DICOM SEG. Source references and plane geometry must match the selected loaded DICOM series; no resampling is performed.");
            ui.small("Up to 64 segments and 4096 mask/source planes, 32 million source voxels and 4 million pixels per plane. Other encodings and differing grids are unsupported. Closing the source releases this set.");
        });
    }
    pub fn prepare_planes(
        &mut self,
        selected: usize,
        source: &Arc<Volume>,
        images: &[Arc<Frame>],
        view: OverlayView,
        ctx: &egui::Context,
    ) {
        if !self.visible || selected != self.series || self.display.visible == 0 {
            self.overlays.clear();
            return;
        }
        let Some(current) = &self.current else {
            self.overlays.clear();
            return;
        };
        self.overlays
            .prepare(current.clone(), source, images, view, &self.display, ctx);
    }
    pub fn draw_plane(
        &self,
        ui: &egui::Ui,
        response: &egui::Response,
        image: &Arc<Frame>,
        view: &ViewState,
        index: usize,
    ) {
        if !self.visible {
            return;
        }
        let Some(current) = &self.current else {
            return;
        };
        if let Some(texture) = self.overlays.texture(index, image, current, &self.display)
            && let Some(mesh) =
                super::canvas::overlay_mesh(response.rect, image, view, texture, self.opacity)
        {
            ui.painter_at(response.rect).add(egui::Shape::mesh(mesh));
        }
    }
    pub fn canvas(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        image: &Arc<Frame>,
        view: &ViewState,
        selected: usize,
    ) {
        let Some(current) = &self.current else {
            return;
        };
        let source = current.source_arc();
        self.prepare_planes(
            selected,
            &source,
            std::slice::from_ref(image),
            OverlayView::Source,
            ui.ctx(),
        );
        self.draw_plane(ui, response, image, view, 0);
    }
}

fn read(series: Arc<Series>, cancel: &dyn Fn() -> bool) -> Result<Option<Arc<Segments>>, String> {
    if cancel() {
        return Ok(None);
    }
    let Some(path) = rfd::FileDialog::new()
        .set_title("Load masks for the selected DICOM source")
        .add_filter("DICOM segmentation", &["dcm", "seg"])
        .pick_file()
    else {
        return Ok(None);
    };
    orv_core::index::read_binary_segmentation(&path, &series, cancel)
        .map(|masks| Some(Arc::new(masks)))
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn masks() -> Arc<Segments> {
        Arc::new(
            Segments::from_binary(
                crate::labels::tests::labels().source_arc(),
                &[9],
                &[(9, 1)],
                &[255; 8],
                None,
                || false,
            )
            .unwrap(),
        )
    }
    #[test]
    fn illustration_capture_uses_only_the_selected_supplied_ids_and_waits_for_replacement() {
        let masks = masks();
        let mut state = Supplied {
            current: Some(masks.clone()),
            series: 3,
            ..Default::default()
        };
        let captured = state.illustration_masks(3).unwrap().unwrap();
        assert_eq!(captured.description().visible_ids, [9]);
        let json = serde_json::to_value(captured.description()).unwrap();
        assert_eq!(json["group"], "supplied");
        assert!(
            captured
                .prepare(&masks.source_arc().frames()[1], 8, &|| false)
                .unwrap()
                .image
                .pixels
                .iter()
                .all(|p| p.a() != 0)
        );
        assert!(state.illustration_masks(2).unwrap().is_none());
        state.pending = Some((1, 3));
        assert!(state.illustration_masks(3).is_err());
        state.pending = None;
        state.display.visible = 0;
        assert!(state.illustration_masks(3).unwrap().is_none());
    }
    #[test]
    fn failed_replacement_and_cancelled_or_stale_completion_preserve_the_previous_set() {
        let masks = masks();
        let mut state = Supplied {
            current: Some(masks.clone()),
            series: 4,
            pending: Some((7, 6)),
            ..Default::default()
        };
        state.finish(7, Err("Generated import failure".into()));
        assert!(Arc::ptr_eq(state.current.as_ref().unwrap(), &masks));
        assert_eq!(state.series, 4);
        assert!(state.pending.is_none());
        state.pending = Some((8, 6));
        state.cancel();
        state.finish(8, Ok(Some(masks.clone())));
        assert_eq!(state.series, 4);
        state.pending = Some((9, 6));
        state.finish(8, Ok(Some(masks.clone())));
        assert_eq!(state.pending, Some((9, 6)));
        assert_eq!(state.series, 4);
        state.finish(9, Ok(Some(masks.clone())));
        assert_eq!(state.series, 6);
        assert!(state.pending.is_none());
        assert!(Arc::ptr_eq(state.current.as_ref().unwrap(), &masks));
    }
    #[test]
    fn manual_discard_preserves_supplied_masks_and_source_closure_releases_them() {
        let masks = masks();
        let retained = Arc::downgrade(&masks);
        let manual = crate::labels::tests::labels();
        let mut labels = crate::labels::LabelUi {
            current: Some(manual),
            dirty: true,
            supplied_active: true,
            supplied: Supplied {
                current: Some(masks.clone()),
                ..Default::default()
            },
            ..Default::default()
        };
        labels.discard_manual();
        assert!(labels.current.is_none());
        assert!(!labels.dirty);
        assert!(labels.supplied_active);
        assert!(Arc::ptr_eq(
            labels.supplied.current.as_ref().unwrap(),
            &masks
        ));
        drop(masks);
        let mut workspace = crate::workspace::Workspace::default();
        std::mem::swap(&mut workspace.labels, &mut labels);
        workspace.close();
        assert!(retained.upgrade().is_none());
    }
}
