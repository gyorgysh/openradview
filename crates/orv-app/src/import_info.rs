// SPDX-License-Identifier: GPL-3.0-or-later
use eframe::egui;
use orv_core::import_report::{ImportReport, SkipReason};

pub(crate) fn summary(ui: &mut egui::Ui, report: &ImportReport) {
    if report.skipped_files() == 0 && report.directories() == 0 {
        return;
    }
    let title = if report.directories() > 0 {
        format!("File-set import · {} skipped", report.skipped_files())
    } else {
        format!("{} skipped inputs", report.skipped_files())
    };
    egui::CollapsingHeader::new(title)
        .show(ui, |ui| {
        ui.small(format!("{} {} indexed", report.indexed_files(), if report.indexed_files() == 1 { "file" } else { "files" }));
            if report.directories() > 0 {
                ui.small(format!("{} media {} read", report.directories(), if report.directories() == 1 { "index" } else { "indexes" }));
                ui.weak("Only listed image records are opened. Open folder scans files directly when a media index is incomplete.");
            }
            for reason in SkipReason::ALL {
                let count = report.count(reason);
                if count > 0 {
                    ui.small(format!("{count} · {}", reason.label()));
                }
            }
            ui.weak("Indexed images are decoded as you browse. This summary clears when the source closes.");
        });
}
