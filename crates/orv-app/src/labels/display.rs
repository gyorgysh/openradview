// SPDX-License-Identifier: GPL-3.0-or-later
use super::{LabelUi, OverlayView, overlay::Mode};
use eframe::egui;
use orv_core::{ViewState, study::Frame, volume::Volume};
use std::sync::Arc;

impl LabelUi {
    pub fn clear_overlays(&mut self) {
        self.overlays.clear();
        self.supplied.clear_overlays();
    }
    pub fn prepare_planes(
        &mut self,
        selected: usize,
        source: &Arc<Volume>,
        images: &[Arc<Frame>],
        view: OverlayView,
        ctx: &egui::Context,
    ) {
        if self.supplied_active {
            self.overlays.clear();
            self.supplied
                .prepare_planes(selected, source, images, view, ctx);
            return;
        }
        self.supplied.clear_overlays();
        if !self.visible
            || selected != self.series
            || self.display.visible_ids[1..].iter().all(|v| !*v)
        {
            self.overlays.clear();
            return;
        }
        let Some(current) = self.current.as_ref() else {
            self.overlays.clear();
            return;
        };
        let labels = self.displayed_manual(current).clone();
        self.overlays
            .prepare(labels, source, images, view, &self.display, ctx);
    }
    pub fn draw_plane(
        &self,
        ui: &egui::Ui,
        response: &egui::Response,
        image: &Arc<Frame>,
        view: &ViewState,
        index: usize,
    ) {
        if self.supplied_active {
            self.supplied.draw_plane(ui, response, image, view, index);
            return;
        }
        if !self.visible {
            return;
        }
        let Some(current) = &self.current else { return };
        let labels = self.displayed_manual(current);
        if let Some(texture) = self.overlays.texture(index, image, labels, &self.display)
            && let Some(mesh) =
                super::canvas::overlay_mesh(response.rect, image, view, texture, self.opacity)
        {
            ui.painter_at(response.rect).add(egui::Shape::mesh(mesh));
        }
        if self.physical_preview_visible() {
            ui.painter_at(response.rect).text(
                response.rect.left_top() + egui::vec2(6., 20.),
                egui::Align2::LEFT_TOP,
                "PHYSICAL PREVIEW ONLY",
                egui::FontId::monospace(10.),
                crate::theme::PAPER,
            );
        }
    }
    pub(super) fn display_controls(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.horizontal(|ui| {
            ui.label("Display");
            egui::ComboBox::from_id_salt("label-display-mode")
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
                        // Accessibility activation need not emit a pointer click for popup auto-close.
                        if ui
                            .selectable_value(&mut self.display.mode, mode, name)
                            .clicked()
                        {
                            ui.close();
                        }
                    }
                });
            if ui.small_button("Show all IDs").clicked() {
                self.display.visible_ids.fill(true);
            }
            if ui.small_button("Hide all IDs").clicked() {
                self.display.visible_ids.fill(false);
            }
        });
        let Some(current) = self.current.as_ref() else {
            return;
        };
        self.counts.update(current, ctx);
        if let Some(counts) = &self.counts.value {
            ui.weak("Volume counts all labeled source cells. Valid and missing source samples stay separate. Hiding an ID only changes the display.");
            let ids: Vec<_> = counts
                .by_id
                .iter()
                .enumerate()
                .skip(1)
                .filter(|(_, count)| count.stored_voxels > 0)
                .map(|(id, _)| id)
                .collect();
            if ids.is_empty() {
                ui.small("No nonzero label IDs. Paint or load labels to inspect regions.");
            }
            // uint8 IDs bound this list to 255 rows. Natural heights also keep
            // keyboard targets and wrapped text correct with the bundled fonts.
            egui::ScrollArea::vertical()
                .id_salt("label-ID-counts")
                .max_height(190.)
                .show(ui, |ui| {
                    for id in ids {
                        let count = counts.by_id[id];
                        let volume = count.stored_voxels as f64 * counts.voxel_volume_mm3;
                        let volume = if !volume.is_finite() {
                            "volume exceeds the numeric range".into()
                        } else if volume > 0. && volume < 0.001 {
                            format!("≈{volume:.3e} mm³")
                        } else {
                            format!("≈{volume:.3} mm³")
                        };
                        ui.horizontal(|ui| {
                            ui.checkbox(&mut self.display.visible_ids[id], format!("ID {id}"));
                            ui.small(format!(
                                "{} {} · {volume}",
                                count.stored_voxels,
                                if count.stored_voxels == 1 {
                                    "voxel"
                                } else {
                                    "voxels"
                                }
                            ));
                        });
                        ui.small(format!(
                            "{} valid · {} missing",
                            count.valid_voxels,
                            count.missing_voxels()
                        ));
                    }
                });
        } else if let Some(error) = &self.counts.error {
            ui.label(error);
            if ui.small_button("Retry label counts").clicked() {
                self.counts = Default::default();
            }
        } else {
            ui.small("Counting source voxels…");
        }
        if let Some(error) = &self.overlays.error {
            ui.label(error);
            if ui.small_button("Retry label overlays").clicked() {
                self.overlays.clear();
            }
        }
    }
}
