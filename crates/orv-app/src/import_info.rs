// SPDX-License-Identifier: GPL-3.0-or-later
use eframe::egui;
use orv_core::import_report::{ImportReport, SkipReason};

pub(crate) fn summary(ui: &mut egui::Ui, report: &ImportReport) {
    if report.skipped_files() == 0 {
        return;
    }
    egui::CollapsingHeader::new(format!("{} skipped inputs", report.skipped_files()))
        .show(ui, |ui| {
            ui.small(format!("{} files indexed", report.indexed_files()));
            for reason in SkipReason::ALL {
                let count = report.count(reason);
                if count > 0 {
                    ui.small(format!("{count} · {}", reason.label()));
                }
            }
            ui.weak("Indexed images are decoded as you browse. This summary clears when the source closes.");
        });
}
