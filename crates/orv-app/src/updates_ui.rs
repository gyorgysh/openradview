// SPDX-License-Identifier: GPL-3.0-or-later
use crate::app::OpenRadView;
use eframe::egui;
use orv_updates::Status;

impl OpenRadView {
    pub(super) fn update_settings(&mut self, ui: &mut egui::Ui) {
        ui.label("Update checks contact GitHub and never include scan data. GitHub receives your IP address, app version and platform.");
        let status = self.updates.status();
        egui::CollapsingHeader::new("Updates")
            .default_open(matches!(status, Status::Available | Status::ReadyToRestart))
            .show(ui, |ui| {
                let package_managed = self.updates.package_managed();
                if package_managed {
                    ui.label("Automatic GitHub checks are off for this installation. Download a newer package from GitHub Releases and install it with your package manager. No apt repository is configured by this app.");
                } else {
                    let mut enabled = self.updates.enabled();
                    let preference = ui.add_enabled(
                        self.updates.available(),
                        egui::Checkbox::new(&mut enabled, "Automatically check for updates"),
                    );
                    if preference.changed()
                        && let Err(message) = self.updates.set_enabled(enabled)
                    {
                        ui.label(message);
                    }
                }
                let can_check = self.updates.available()
                    && !status.busy()
                    && status != Status::ReadyToRestart;
                if ui
                    .add_enabled(can_check, egui::Button::new("Check for updates…"))
                    .clicked()
                    && let Err(message) = self.updates.check()
                {
                    ui.label(message);
                }
                ui.label(status.label());
                let detail = self.updates.detail();
                if !detail.is_empty() {
                    ui.label(detail);
                }
                if let Some(info) = self.updates.update_info() {
                    if status == Status::Available {
                        if package_managed {
                            ui.label(format!("OpenRadView {}", info.version));
                        } else {
                            ui.label(format!(
                                "OpenRadView {} · {:.1} MB",
                                info.version,
                                info.bytes as f64 / 1_000_000.
                            ));
                            let can_install = !self.workspace.labels_need_saving()
                                && self.updates.can_install();
                            if ui
                                .add_enabled(
                                    can_install,
                                    egui::Button::new("Download and install update"),
                                )
                                .clicked()
                                && let Err(message) = self.updates.install()
                            {
                                ui.label(message);
                            }
                        }
                    } else if status == Status::Downloading {
                        let progress = self.updates.downloaded_bytes() as f64 / info.bytes as f64;
                        ui.add(egui::ProgressBar::new(progress as f32).show_percentage());
                    }
                }
                if status == Status::ReadyToRestart
                    && ui
                        .add_enabled(
                            !self.workspace.labels_need_saving(),
                            egui::Button::new("Restart now"),
                        )
                        .clicked()
                {
                    self.updates.request_restart();
                }
                if self.workspace.labels_need_saving() {
                    ui.weak("Export or discard local labels before installing or restarting.");
                }
                if !package_managed {
                    ui.weak("When enabled, checks run about once a day. Download, installation and restart require your action. No telemetry or automatic installation.");
                }
                ui.hyperlink_to(if package_managed { "Packages on GitHub" } else { "Downloads on GitHub" }, "https://github.com/gyorgysh/openradview/releases/latest");
            });
        ui.separator();
    }
}
