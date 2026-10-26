// SPDX-License-Identifier: GPL-3.0-or-later
use super::{Sharing, client_config};
use eframe::egui;
use orv_mcp::http::ServerStatus;

impl Sharing {
    /// Returns advanced manual-review/comparison actions for the workspace.
    pub(super) fn panel(&mut self, ctx: &egui::Context) -> (bool, bool) {
        let mut open = self.panel_open;
        let (mut manual, mut compare, mut revoke, mut review) = (false, false, false, false);
        let mut enabled = !self.requests_disabled;
        crate::dialogs::window("Agents", ctx).open(&mut open).default_width(540.).default_height(480.).show(ctx, |ui| {
            if let Some(live) = &self.live {
                ui.heading(format!("Connected: {}", live.recipient));
                ui.label(if live.full_series { "This session covers the selected series and its approved tools." } else { "This session covers only exact reviewed views and approved tools." });
                ui.horizontal_wrapped(|ui| {
                    revoke = ui.button("Revoke access").clicked();
                    review = ui.button("Review shared view").clicked();
                });
            } else if self.comparison.is_active() {
                ui.label("A frozen comparison is shared with your approved recipients.");
                if ui.button("Show commentary").clicked() { self.comparison.show_results(); }
                revoke = ui.button("Revoke access").clicked();
            } else {
                ui.heading("Connect your own agent");
                ui.label("Copy the setup prompt to your local agent. Open a series, ask it to request access, then approve that session here.");
            }
            ui.add_space(8.);
            ui.label("Requests alone cannot read your images. You choose what to share before each session.");
            if self.requests_disabled { ui.weak("Connection requests are off. Enable them under Advanced."); }
            if let Some(broker) = &self.broker {
                match broker.status() {
                    ServerStatus::Listening { .. } => { ui.weak("Ready for local connection requests"); }
                    ServerStatus::Starting => { ui.weak("Starting local connection requests…"); }
                    ServerStatus::Failed(e) => { ui.label(e); }
                    ServerStatus::Stopped => { ui.weak("Connection requests stopped"); }
                }
            }
            if let Some(path) = client_config::companion() {
                ui.horizontal_wrapped(|ui| {
                    if ui.button("Copy setup prompt").clicked() && let Some(text) = client_config::setup_prompt(&path) { ctx.copy_text(text); }
                });
                ui.weak("Install in Applications first. Setup contains no session token. Refresh the agent's MCP connection after setup.");
            } else {
                ui.label("The MCP companion is missing. Use the MCP application bundle for the copyable connector setup.");
            }
            ui.add_space(8.);
            ui.label("Your agent may send approved data to its provider. Review its retention and training settings before allowing access.");
            egui::CollapsingHeader::new("Advanced").show(ui, |ui| {
                ui.checkbox(&mut enabled, "Allow agents to request a session");
                ui.weak("Client names are self-reported, not verified accounts. Turning requests off also revokes access.");
                if let Some(path) = client_config::companion() {
                    ui.horizontal_wrapped(|ui| {
                        if ui.button("Copy MCP JSON").clicked() && let Some(text) = client_config::reusable_json(&path) { ctx.copy_text(text); }
                        if ui.button("Copy Codex TOML").clicked() && let Some(text) = client_config::reusable_codex(&path) { ctx.copy_text(text); }
                    });
                }
                ui.label("The normal session starts with all supported tools for one selected series. Customize those permissions in the approval dialog.");
                egui::CollapsingHeader::new("Tools and endpoints").show(ui, |ui| {
                    ui.label("request_access: ask for desktop approval; call again after the user responds.");
                    ui.label("open_study / list_series: approved scope and current scene. render_snapshot: image-only PNG and reproducible scene manifest. set_view: supported window, invert, pan/zoom, slice or derived-view settings. get_tags: allowlisted geometry. measure / roi_stats: research plane measurements.");
                    ui.monospace("openradview-mcp instances --json");
                    ui.label("list_instances discovers local apps; request_access selects instance_id when several run. No fixed port is required.");
                    if let Some(broker) = &self.broker && let ServerStatus::Listening { port } = broker.status() { ui.monospace(format!("http://127.0.0.1:{port}/request")); }
                    ui.label("POST/GET approval protocol only. The connector receives a random-port /mcp endpoint with an ephemeral bearer token after Allow. No arbitrary paths or access to other series.");
                });
                if self.live.is_some() && ui.button("Direct session configuration…").clicked() { self.connection_open = true; }
                ui.add_enabled_ui(!self.is_active(), |ui| {
                    manual = ui.button("Manually review a connection…").clicked();
                    compare = ui.button("Compare agents using a frozen image…").clicked();
                });
            });
            if let Some(notice) = &self.notice { ui.label(notice); }
            if let Some(notice) = &self.comparison.notice { ui.label(notice); }
        });
        self.panel_open = open;
        if enabled == self.requests_disabled {
            self.requests_disabled = !enabled;
            self.broker_attempted = false;
            self.broker = None;
            if !enabled {
                self.revoke();
            }
        }
        if revoke {
            self.revoke();
        }
        if review {
            self.review(ctx);
            self.panel_open = false;
        }
        if manual || compare {
            self.panel_open = false;
        }
        (manual, compare)
    }
}
