// SPDX-License-Identifier: GPL-3.0-or-later
use super::Playback;
use eframe::egui;
use orv_core::acquisition::CineMetadata;

impl Playback {
    pub fn buttons(&mut self, ui: &mut egui::Ui, target: &mut u32, count: usize, allowed: bool) {
        if ui
            .add_enabled(
                allowed,
                egui::Button::new(if self.running { "Pause" } else { "Play" }),
            )
            .clicked()
        {
            if self.running {
                self.pause();
            } else if let Some(index) = self.start_at(*target, count) {
                *target = index;
            }
        }
        ui.add_enabled_ui(allowed && !self.source_mode, |ui| {
            let rate = ui.add(egui::DragValue::new(&mut self.fps).range(1..=30).suffix(" fps"))
                .on_hover_text("Manual browsing speed. Slower decoding reduces playback speed; images are not skipped.");
            ui.ctx().accesskit_node_builder(rate.id, |node| node.set_label("Manual playback rate"));
            let repeat = ui.checkbox(&mut self.repeat, "Loop");
            if rate.changed() || repeat.changed() { self.pause(); }
        });
    }
    pub fn source_controls(
        &self,
        ui: &mut egui::Ui,
        metadata: Option<&CineMetadata>,
        allowed: bool,
    ) -> Option<bool> {
        let mut change = None;
        if let Some(metadata) = metadata.filter(|m| !matches!(m, CineMetadata::Absent)) {
            ui.horizontal_wrapped(|ui| {
                let mut enabled = self.source_mode;
                if ui.add_enabled(allowed && self.source_playable(), egui::Checkbox::new(&mut enabled, "Source intervals"))
                    .on_hover_text("Use validated original-frame intervals and trim. Stops at the trim end; manual fps and Loop are inactive. Changing mode ends existing sharing approval.")
                    .changed() { change = Some(enabled); }
                match metadata {
                    CineMetadata::Available(timing) if self.source_playable() => {
                        let [first, last] = timing.trim();
                        ui.small(format!("Images {}–{} · {} · stops at end", first+1, last+1, if timing.variable() { "variable intervals" } else { "fixed interval" }));
                    },
                    CineMetadata::Available(_) => { ui.small("Source trim contains one image; there is no interval to play."); },
                    CineMetadata::Unavailable(error) => { ui.small(error.text()); },
                    CineMetadata::Absent => (),
                }
            });
        }
        if self.source_mode {
            if let Some(pace) = &self.pace {
                ui.small(format!(
                    "Last ready step: {:.1} ms · source {:.1} ms · image wait {:.1} ms",
                    pace.elapsed.as_secs_f64() * 1000.,
                    pace.interval.as_secs_f64() * 1000.,
                    pace.image_wait.as_secs_f64() * 1000.
                ));
            }
            if self.running && self.shown.is_none() {
                ui.small("Waiting for the requested image; timing resumes when ready. No images skipped.");
            }
        }
        if let Some(notice) = self.notice {
            ui.small(notice);
        }
        change
    }
}
