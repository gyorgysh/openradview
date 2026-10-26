// SPDX-License-Identifier: GPL-3.0-or-later
use crate::{brand, theme};
use eframe::egui::{self, Align, Color32, Layout, RichText, Vec2};
use std::time::Instant;

pub struct OpenRadView {
    accepted: bool,
    understood: bool,
    reduce_motion: bool,
    started: Instant,
    wordmark: Option<egui::TextureHandle>,
    privacy_open: bool,
    help_open: bool,
    licenses: crate::licenses::Licenses,
    pub(super) workspace: crate::workspace::Workspace,
    #[cfg(feature = "updates")]
    pub(super) updates: orv_updates::Updater,
    #[cfg(feature = "updates")]
    automatic_updates: bool,
    #[cfg(feature = "updates")]
    quit_after_update: bool,
    #[cfg(feature = "qa")]
    diagnostics: Option<crate::qa::Recorder>,
}

impl OpenRadView {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::install(&cc.egui_ctx);
        Self {
            accepted: false,
            understood: false,
            reduce_motion: system_reduce_motion(),
            started: Instant::now(),
            wordmark: brand::texture(&cc.egui_ctx, brand::WORDMARK),
            privacy_open: false,
            help_open: false,
            licenses: Default::default(),
            workspace: Default::default(),
            #[cfg(feature = "updates")]
            updates: {
                let repaint = cc.egui_ctx.clone();
                orv_updates::Updater::new(
                    std::sync::Arc::new(move || repaint.request_repaint()),
                    env!("CARGO_PKG_VERSION"),
                )
            },
            #[cfg(feature = "updates")]
            automatic_updates: true,
            #[cfg(feature = "updates")]
            quit_after_update: false,
            #[cfg(feature = "qa")]
            diagnostics: None,
        }
    }

    #[cfg(feature = "qa")]
    pub fn with_diagnostics(mut self, recorder: crate::qa::Recorder) -> Self {
        self.diagnostics = Some(recorder);
        self
    }

    fn agreement(&mut self, ui: &mut egui::Ui, viewport_height: f32) {
        ui.vertical_centered(|ui| {
            ui.add_space((viewport_height - 530.0).max(20.0) / 2.0);
            brand::symbol(
                ui,
                72.0,
                self.started.elapsed().as_secs_f32(),
                self.reduce_motion,
            );
            ui.add_space(28.0);
            ui.heading("View your own scans");
            ui.add_space(8.0);
            ui.label(
                RichText::new("Open local images and learn how they are displayed.")
                    .color(theme::SILVER),
            );
            ui.add_space(28.0);
            ui.allocate_ui_with_layout(
                Vec2::new(540.0, 240.0),
                Layout::top_down(Align::LEFT),
                |ui| {
                    theme::eyebrow(ui, "BEFORE YOU BEGIN");
                    ui.label(crate::copy::PURPOSE);
                    ui.label(crate::copy::LIMITS);
                    ui.label(crate::copy::CARE);
                    ui.add_space(6.0);
                    ui.checkbox(&mut self.understood, "I have read the research-use notice.");
                    ui.checkbox(&mut self.reduce_motion, "Reduce motion");
                    #[cfg(feature = "updates")]
                    if self.updates.package_managed() {
                        ui.weak("Updates are managed outside this app. Check GitHub manually in Privacy → Updates.");
                    } else if self.updates.available() && !self.updates.preference_chosen() {
                        ui.checkbox(
                            &mut self.automatic_updates,
                            "Automatically check GitHub for updates",
                        );
                        ui.weak("No scan data is sent. Change this anytime in Privacy → Updates.");
                    }
                    if ui
                        .add_enabled(
                            self.understood,
                            egui::Button::new("Open viewer  →").min_size(Vec2::new(540.0, 46.0)),
                        )
                        .clicked()
                    {
                        #[cfg(feature = "updates")]
                        if self.updates.available()
                            && !self.updates.package_managed()
                            && !self.updates.preference_chosen()
                            && let Err(message) = self.updates.set_enabled(self.automatic_updates)
                        {
                            // Viewing remains usable with a read-only settings
                            // folder; the updater keeps background checks off.
                            ui.label(message);
                        }
                        self.accepted = true;
                        self.started = Instant::now();
                    }
                },
            );
            ui.add_space(26.0);
            theme::eyebrow(
                ui,
                if cfg!(feature = "updates") {
                    "LOCAL IMAGES  /  NO TELEMETRY  /  NO ACCOUNT"
                } else {
                    "OFFLINE BY DEFAULT  /  NO TELEMETRY  /  NO ACCOUNT"
                },
            );
        });
    }
}

fn system_reduce_motion() -> bool {
    #[cfg(target_os = "macos")]
    {
        objc2_app_kit::NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion()
    }
    #[cfg(not(target_os = "macos"))]
    {
        true
    }
}

impl eframe::App for OpenRadView {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        #[cfg(feature = "qa")]
        let measured_start = Instant::now();
        let ctx = ui.ctx().clone();
        let external_dialog_was_open = self.help_open || self.privacy_open || self.licenses.open;
        #[cfg(feature = "updates")]
        {
            if ctx.input(|i| i.viewport().close_requested())
                && !self.workspace.labels_need_saving()
                && !self.updates.request_close()
            {
                self.quit_after_update = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            }
            if self.quit_after_update
                && !self.updates.status().busy()
                && self.updates.request_close()
            {
                self.quit_after_update = false;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
        self.workspace.protect_labels_on_quit(&ctx);
        let animation_time = if self.reduce_motion { 0.0 } else { 0.2 };
        if ctx.global_style().animation_time != animation_time {
            ctx.style_mut_of(egui::Theme::Dark, |style| {
                style.animation_time = animation_time;
            });
        }
        egui::Frame::new()
            .fill(theme::INK)
            .inner_margin(16.0)
            .show(ui, |ui| {
                #[cfg(target_os = "macos")]
                {
                    // Real native traffic lights live above the content. The
                    // remaining black title band is draggable without stealing
                    // pointer input from image canvases or controls.
                    let (rect, _) = ui.allocate_exact_size(
                        Vec2::new(ui.available_width(), 24.0),
                        egui::Sense::hover(),
                    );
                    let drag_rect =
                        egui::Rect::from_min_max(rect.min + egui::vec2(80.0, 0.0), rect.max);
                    if ui
                        .interact(
                            drag_rect,
                            ui.id().with("native-title-drag"),
                            egui::Sense::drag(),
                        )
                        .drag_started()
                    {
                        ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                    }
                }
                ui.horizontal(|ui| {
                    brand::lockup(
                        ui,
                        self.wordmark.as_ref(),
                        self.started.elapsed().as_secs_f32(),
                        self.reduce_motion,
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        #[cfg(feature = "updates")]
                        if matches!(
                            self.updates.status(),
                            orv_updates::Status::Available | orv_updates::Status::ReadyToRestart
                        ) && ui.button("Update available").clicked()
                        {
                            self.privacy_open = true;
                        }
                        if ui.button("Privacy").clicked() {
                            self.privacy_open = true;
                        }
                        if ui.button("Help").clicked() {
                            self.help_open = true;
                        }
                        ui.label(
                            RichText::new(if self.workspace.sharing_active() {
                                "AGENT ACCESS ON"
                            } else if cfg!(feature = "updates") {
                                "LOCAL IMAGES"
                            } else {
                                "OFFLINE"
                            })
                            .monospace()
                            .size(10.0)
                            .color(theme::SILVER),
                        );
                    });
                });
                ui.add_space(12.0);
                ui.separator();
                let height = (ui.available_height() - 40.0).max(0.0);
                if self.help_open || self.privacy_open || self.licenses.open {
                    self.workspace.pause_playback();
                }
                ui.allocate_ui(Vec2::new(ui.available_width(), height), |ui| {
                    if self.accepted {
                        self.workspace.ui(
                            ui,
                            frame,
                            self.started.elapsed().as_secs_f32(),
                            self.reduce_motion,
                            self.help_open || self.privacy_open || self.licenses.open,
                        );
                    } else {
                        let viewport_height = ui.available_height();
                        egui::ScrollArea::vertical()
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                self.agreement(ui, viewport_height);
                            });
                    }
                });
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(concat!("RESEARCH PREVIEW  /  ", env!("CARGO_PKG_VERSION")))
                            .monospace()
                            .size(10.0)
                            .color(theme::SILVER),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(
                            RichText::new("Not for diagnosis or clinical decisions")
                                .size(12.0)
                                .color(theme::SILVER),
                        );
                    });
                });
            });
        if crate::help::show(&ctx, &mut self.help_open) {
            self.help_open = false;
            self.licenses.open = true;
        }
        if self.licenses.show(&ctx) {
            self.help_open = true;
        }
        let mut privacy_open = self.privacy_open;
        crate::dialogs::window("Privacy", &ctx).open(&mut privacy_open)
            .collapsible(false).resizable(false).default_width(470.0).default_height(480.0).show(&ctx, |ui| {
                ui.label("OpenRadView has no telemetry or app account.");
                #[cfg(feature="updates")]
                self.update_settings(ui);
                #[cfg(not(feature="updates"))]
                ui.label("This build has no automatic web connections or update framework.");
                ui.label("The app does not save a study history. This acknowledgement applies to the current run.");
                ui.label(crate::copy::AGENT_PRIVACY);
                if self.workspace.sharing_active() {
                    ui.label("Agent access is enabled for this session. Configured recipients can read only their approved scope. Their agents may send shared data off this device. Revoke access in the workspace to end the connections.");
                } else {
                    ui.label("Agent data access is off. Local connection requests wait for your explicit approval in this app.");
                }
                ui.add_space(8.0);
                ui.label(RichText::new("Personal education only. Not for decisions about care.").color(theme::SILVER));
                ui.add_space(12.);
            });
        self.privacy_open = privacy_open;
        if external_dialog_was_open && !(self.help_open || self.privacy_open || self.licenses.open)
        {
            ctx.request_repaint();
        }
        #[cfg(feature = "updates")]
        if !self.workspace.labels_need_saving() {
            self.updates.finish_relaunch(|| self.workspace.close());
        }
        #[cfg(feature = "qa")]
        if let Some(diagnostics) = &mut self.diagnostics {
            diagnostics.frame(measured_start.elapsed(), frame.info().cpu_usage);
        }
    }

    #[cfg(feature = "qa")]
    fn raw_input_hook(&mut self, _: &egui::Context, raw: &mut egui::RawInput) {
        if let Some(diagnostics) = &mut self.diagnostics {
            diagnostics.input(!raw.events.is_empty());
        }
    }

    #[cfg(feature = "qa")]
    fn on_exit(&mut self) {
        if let Some(diagnostics) = self.diagnostics.take()
            && diagnostics.finish().is_err()
        {
            eprintln!("The requested local QA timing report could not be written");
        }
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        Color32::from_gray(10).to_normalized_gamma_f32()
    }
}
