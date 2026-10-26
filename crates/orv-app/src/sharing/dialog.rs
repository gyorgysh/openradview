// SPDX-License-Identifier: GPL-3.0-or-later
use super::{Draft, Sharing, Transport};
use eframe::egui::{self, RichText};
use orv_core::Plane;
use orv_mcp::http::ServerStatus;

#[derive(Clone, Copy)]
enum Action {
    Cancel,
    Allow,
    Revoke,
}

impl Sharing {
    pub(super) fn ui(&mut self, ctx: &egui::Context) {
        let mut action = None;
        if let Some(draft) = &mut self.draft {
            let response = egui::Modal::new(egui::Id::new(("agent-consent", draft.review_id)))
                .show(ctx, |ui| {
                    let width = (ctx.content_rect().width() - 80.0).clamp(300.0, 680.0);
                    ui.set_width(width);
                    if crate::dialogs::heading(
                        ui,
                        if draft.existing {
                            "Review the shared view"
                        } else {
                            "Allow agent access?"
                        },
                    ) {
                        action = Some(Action::Cancel);
                    }
                    ui.label(
                        RichText::new("Review access for this session.")
                            .color(crate::theme::SILVER),
                    );
                    ui.separator();
                    if self.live.is_some() && !draft.existing {
                        ui.label("Allow replaces the current agent connection. Closing this request keeps the existing connection active.");
                    }
                    egui::ScrollArea::vertical()
                        .id_salt(("agent-review-scroll", draft.review_id))
                        .auto_shrink([false, false])
                        .max_height((ctx.content_rect().height() - 300.0).clamp(120.0, 500.0))
                        .show(ui, |ui| draft.content(ui));
                    ui.separator();
                    ui.label(crate::copy::AGENT_PRIVACY);
                    ui.separator();
                    ui.horizontal_wrapped(|ui| {
                        if ui
                            .button(if draft.existing {
                                "Cancel review"
                            } else {
                                "Keep private"
                            })
                            .clicked()
                        {
                            action = Some(Action::Cancel);
                        }
                        if draft.existing && ui.button("Revoke access").clicked() {
                            action = Some(Action::Revoke);
                        }
                        let label = if draft.existing && draft.full_series {
                            "Done"
                        } else if draft.existing {
                            "Approve this exact view"
                        } else {
                            "Allow for this session"
                        };
                        if ui
                            .add_enabled(draft.can_confirm(), egui::Button::new(label))
                            .clicked()
                        {
                            draft.reviewed = true;
                            draft.risk = true;
                            action = Some(Action::Allow);
                        }
                    });
                });
            if response.should_close() && action.is_none() {
                action = Some(Action::Cancel);
            }
        }
        match action {
            Some(Action::Cancel) => {
                if let Some(id) = self.draft.as_ref().and_then(|d| d.request_id)
                    && let Some(broker) = &self.broker
                {
                    broker.deny(id);
                }
                self.draft = None;
            }
            Some(Action::Revoke) => self.revoke(),
            Some(Action::Allow) => {
                if let Err(error) = self.allow(ctx)
                    && let Some(draft) = &mut self.draft
                {
                    draft.error = Some(error);
                }
            }
            None => (),
        }
        if self.draft.is_none() {
            self.connection_ui(ctx);
            self.comparison.ui(ctx);
        }
    }

    fn connection_ui(&mut self, ctx: &egui::Context) {
        let Some(live) = &self.live else {
            return;
        };
        let mut revoke = false;
        crate::dialogs::window("Agent connection", ctx).open(&mut self.connection_open).default_width(550.0)
            .resizable(false).collapsible(false).show(ctx, |ui| {
                ui.label(format!("Recipient: {}", live.recipient));
                ui.label(live.transport.label());
                ui.label(if live.full_series { "Scope: all images and derived views in the selected series." }
                    else { "Scope: exact reviewed views only. New views require review." });
                ui.label("The recipient label is supplied by you; its account is not verified here. Anyone with this token can use the allowed tools.");
                match live.server.status() {
                    ServerStatus::Starting => { ui.label("Starting local access…"); }
                    ServerStatus::Listening { port } => {
                        let endpoint = format!("http://127.0.0.1:{port}/mcp");
                        ui.monospace(&endpoint);
                        ui.label("Configuration contains a session token. Give it only to this recipient. It becomes invalid when you revoke, close the study or quit.");
                        if live.transport == Transport::Http {
                            if ui.button("Copy HTTP configuration").clicked() {
                                let config = serde_json::json!({"mcpServers":{"OpenRadView":{
                                    "url":endpoint, "headers":{"Authorization":format!("Bearer {}", live.server.token_for_configuration())}
                                }}});
                                if let Ok(text) = serde_json::to_string_pretty(&config) { ctx.copy_text(text); }
                            }
                            if ui.button("Copy Codex TOML").clicked()
                                && let Some(text) = super::client_config::codex("openradview", &endpoint, live.server.token_for_configuration(), None) {
                                ctx.copy_text(text);
                            }
                        } else {
                            let companion = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join(
                                if cfg!(windows) { "openradview-mcp.exe" } else { "openradview-mcp" })));
                            let exists = companion.as_ref().is_some_and(|p| p.is_file());
                            if !exists { ui.label("The stdio companion is missing. Use an MCP-enabled application bundle."); }
                            if ui.add_enabled(exists, egui::Button::new("Copy stdio configuration")).clicked()
                                && let Some(ref companion) = companion {
                                let config = serde_json::json!({"mcpServers":{"OpenRadView":{
                                    "command":companion, "args":["stdio","--url",endpoint],
                                    "env":{"OPENRADVIEW_TOKEN":live.server.token_for_configuration()}
                                }}});
                                if let Ok(text) = serde_json::to_string_pretty(&config) { ctx.copy_text(text); }
                            }
                            if ui.add_enabled(exists, egui::Button::new("Copy Codex TOML")).clicked()
                                && let Some(companion) = companion.as_deref()
                                && let Some(text) = super::client_config::codex("openradview", &endpoint, live.server.token_for_configuration(), Some(companion)) {
                                ctx.copy_text(text);
                            }
                        }
                        ui.label("Paste JSON into your client's private MCP settings, or Codex TOML into ~/.codex/config.toml. Refresh/restart that client's MCP connection. Keep this app/session open; no provider key is needed here.");
                    }
                    ServerStatus::Failed(error) => { ui.label(error); }
                    ServerStatus::Stopped => { ui.label("Access stopped."); }
                }
                ui.add_space(8.0);
                ui.label("Agent connections use loopback. Your configured agent may send approved data off this device. Tag removal does not remove identifying text inside pixels.");
                if ui.button("Revoke access").clicked() { revoke = true; }
            });
        if revoke {
            self.revoke();
        }
    }
}

impl Draft {
    fn content(&mut self, ui: &mut egui::Ui) {
        if self.request_id.is_some() || self.existing {
            ui.label(RichText::new(format!("Recipient: {}", self.recipient)).strong());
            ui.weak("Client name is self-reported, not a verified account.");
        } else {
            ui.label("Agent / application receiving access");
            ui.add(
                egui::TextEdit::singleline(&mut self.recipient)
                    .hint_text("Your local agent's name")
                    .char_limit(128)
                    .desired_width(f32::INFINITY),
            );
        }
        ui.label(if self.full_series {
            "Scope: all images and supported derived views in this selected series. Other series and files stay private."
        } else {
            "Scope: exact reviewed views only. Different images require another review. Other series and files stay private."
        });
        let tools = [
            (self.tools.snapshots, "PNG snapshots"),
            (self.tools.view_control, "display control"),
            (self.tools.metadata, "geometry"),
            (self.tools.measurements, "measurements / ROI"),
        ]
        .into_iter()
        .filter_map(|(on, name)| on.then_some(name))
        .collect::<Vec<_>>()
        .join(", ");
        ui.label(format!("Allowed tools: {tools}."));
        if let Some(error) = self.error {
            ui.label(error);
        }
        if let Some(preview) = &self.preview {
            let source = preview.data.source.description();
            ui.label(format!(
                "{} · selected series {} · {} source images",
                source.modality,
                source.series_index + 1,
                source.image_count
            ));
            let size = preview.texture.size_vec2();
            let scale = (200. / size.y).min(ui.available_width() / size.x).min(1.);
            ui.image((preview.texture.id(), size * scale));
            ui.weak("Image-only preview; no UI annotations. Geometry and a reproducible view manifest accompany snapshots.");
        } else if self.error.is_none() {
            ui.label("Preparing the preview and checking the selected series locally…");
        }
        if !self.existing {
            egui::CollapsingHeader::new("Advanced permissions").show(ui, |ui| self.permissions(ui));
        }
        egui::CollapsingHeader::new("Preview details and metadata").show(ui, |ui| {
            if self.tools.measurements {
                ui.label("ROI statistics read scalar samples across the approved image before PNG windowing/downsampling, including outside its zoomed/panned crop. Physical measurements require valid plane geometry and are unavailable on 3D/MIP projections.");
            }
            if self.full_series && !self.existing && let Some(preview) = &self.preview {
                let count = preview.data.source.description().image_count as u32;
                if ui.add(egui::Slider::new(&mut self.browser_slice, 0..=count.saturating_sub(1)).text("Source image preview")).changed() {
                    let mut spec = self.spec.clone();
                    spec.view.plane = Plane::Acquired;
                    spec.reconstruction = None;
                    spec.reformat_side = 512;
                    spec.view.slice = self.browser_slice;
                    spec.point = None;
                    self.request(spec);
                }
            }
            if let Some(preview) = &self.preview {
                ui.label(format!("Shared PNG: {} × {} pixels", self.size[0], self.size[1]));
                ui.checkbox(&mut self.zoom, "Show full pixel size");
                if self.zoom {
                    egui::ScrollArea::both().id_salt("exact-pixels").min_scrolled_height(270.).max_height(270.).show(ui, |ui| {
                        ui.image((preview.texture.id(), preview.texture.size_vec2()));
                    });
                }
                ui.label("Fields: source fingerprint, series index/count/modality, complete scene/display state, output size, geometry and image/view hashes; projections include camera, transfer function and sampling budgets. No names, dates, original UIDs, descriptions or file paths are copied. Resampling approximates the source.");
                egui::CollapsingHeader::new("Exact scene and geometry (JSON)").show(ui, |ui| {
                    ui.add(egui::Label::new(RichText::new(&preview.json).monospace().small()).wrap());
                });
            }
        });
        ui.add_space(6.);
        ui.label("Your agent may send approved data off this device. Removing tags cannot remove names burned into pixels.");
        ui.weak("Allow confirms the recipient, preview, permissions and sharing risks. This session ends on revoke, changing/closing the study or series, or quitting. Experimental AI commentary only; not for diagnosis.");
    }
    fn permissions(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.checkbox(&mut self.tools.snapshots, "PNG + view manifest");
            ui.checkbox(&mut self.tools.view_control, "View control");
            ui.checkbox(&mut self.tools.metadata, "Geometry query");
            ui.checkbox(&mut self.tools.measurements, "Measurements / ROI");
        });
        if ui
            .checkbox(
                &mut self.full_series,
                "Every image and supported view in this selected series",
            )
            .changed()
        {
            self.request(self.initial_spec.clone());
        }
        ui.label("Turn off series exploration to require review of each new exact view. Tool switches never grant other series or arbitrary files.");
        if self.request_id.is_none() {
            egui::ComboBox::from_id_salt("agent-transport")
                .selected_text(self.transport.label())
                .show_ui(ui, |ui| {
                    for transport in [Transport::Http, Transport::Stdio] {
                        ui.selectable_value(&mut self.transport, transport, transport.label());
                    }
                });
        }
        ui.weak("A random local port and token are created only after Allow. The reusable connector collects them privately for this requester.");
    }
}
