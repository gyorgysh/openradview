// SPDX-License-Identifier: GPL-3.0-or-later
//! Up to four source workers; extra views stay outside the two-pane file format.
use super::*;
const PAIRS: [[usize; 2]; 6] = [[0, 1], [0, 2], [0, 3], [1, 2], [1, 3], [2, 3]];
pub(super) struct Grid {
    pub columns: usize,
    pub rows: usize,
    pub pane: egui::Vec2,
}
pub(super) fn grid(count: usize, size: egui::Vec2, gap: egui::Vec2) -> Grid {
    let columns = if count == 4 || size.x >= 640. { 2 } else { 1 };
    let rows = count / columns;
    Grid {
        columns,
        rows,
        pane: egui::vec2(
            ((size.x - gap.x * (columns - 1) as f32) / columns as f32).max(1.),
            ((size.y - gap.y * (rows - 1) as f32) / rows as f32).max(1.),
        ),
    }
}
impl ComparisonSession {
    pub fn temporary_panes(&self) -> bool {
        self.panes.len() > 2
    }
    fn add_extra(&mut self, ctx: &egui::Context) -> Result<(), &'static str> {
        if self.panes.len() == 4 {
            return Ok(());
        }
        self.panes
            .try_reserve_exact(2)
            .map_err(|_| "Not enough memory for two extra panes.")?;
        let mut extra = Vec::new();
        extra
            .try_reserve_exact(2)
            .map_err(|_| "Not enough memory for two extra panes.")?;
        for side in 0..2 {
            let study = self.panes[side].study.clone();
            let series = (0..study.series.len())
                .find(|index| {
                    !self.panes.iter().chain(extra.iter()).any(|pane: &Pane| {
                        Arc::ptr_eq(&pane.study, &study) && pane.series == *index
                    })
                })
                .unwrap_or(self.panes[side].series);
            let source = self
                .panes
                .iter()
                .chain(extra.iter())
                .find(|pane| Arc::ptr_eq(&pane.study, &study) && pane.series == series)
                .map(|pane| pane.source.clone())
                .or_else(|| {
                    study
                        .series
                        .get(series)
                        .map(|source| Arc::new(source.clone()))
                })
                .ok_or("The extra source series is unavailable.")?;
            extra.push(Pane::from_source(study, series, source, ctx));
        }
        self.panes.extend(extra);
        Ok(())
    }
    fn set_pair(&mut self, pair: [usize; 2]) -> bool {
        if pair == self.pair
            || pair[0] == pair[1]
            || pair.iter().any(|index| *index >= self.panes.len())
        {
            return false;
        }
        self.pair = pair;
        self.linked = false;
        self.display_links = display_link::Group::default();
        self.notice = None;
        self.reference_status = if self.reference_lines {
            reference_ui::Status::Pending
        } else {
            reference_ui::Status::Off
        };
        true
    }
    pub(super) fn layout_controls(&mut self, ui: &mut egui::Ui) {
        let four = self.temporary_panes();
        if ui
            .button(if four { "Two panes" } else { "Four panes" })
            .clicked()
        {
            if four {
                self.close_extra_question = true;
            } else if let Err(message) = self.add_extra(ui.ctx()) {
                self.file_notice = Some(message);
            }
            ui.ctx().request_repaint();
        }
    }
    pub(super) fn pair_controls(&mut self, ui: &mut egui::Ui) {
        ui.label("Choose which two panes share the link and reference controls.");
        if self.temporary_panes() {
            let mut pair = self.pair;
            ui.horizontal_wrapped(|ui| {
                for choice in PAIRS {
                    ui.selectable_value(
                        &mut pair,
                        choice,
                        format!("{} + {}", choice[0] + 1, choice[1] + 1),
                    );
                }
            });
            if self.set_pair(pair) {
                ui.ctx().request_repaint();
            }
        }
        if !self.pair.contains(&self.active) {
            ui.weak("Select one of the chosen panes to change its links. The other panes stay independent.");
        }
    }
    fn close_extra(&mut self) {
        self.panes.truncate(2);
        self.active = self.active.min(1);
        self.pair = [0, 1];
        self.linked = false;
        self.display_links = display_link::Group::default();
        self.notice = None;
        self.reference_status = if self.reference_lines {
            reference_ui::Status::Pending
        } else {
            reference_ui::Status::Off
        };
        self.close_extra_question = false;
        self.clear_extra_slots = true;
        self.file_notice = None;
    }
    pub(super) fn layout_question(&mut self, ctx: &egui::Context) {
        if !self.close_extra_question {
            return;
        }
        egui::Modal::new(egui::Id::new("close-extra-comparison-panes")).show(ctx, |ui| {
            ui.heading("Close panes 3 and 4?");
            ui.label("Their temporary views and measurements will be discarded. Nothing is saved automatically. Return to the four-pane view to save any needed pictures or illustrations first.");
            ui.horizontal(|ui| {
                if ui.button("Keep four panes").clicked() { self.close_extra_question = false; ctx.request_repaint(); }
                if ui.button("Close panes 3 and 4").clicked() { self.close_extra(); ctx.request_repaint(); }
            });
        });
    }
}

#[cfg(test)]
mod tests;
