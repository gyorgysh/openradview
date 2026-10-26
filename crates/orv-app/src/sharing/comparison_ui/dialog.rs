// SPDX-License-Identifier: GPL-3.0-or-later
use super::{ComparisonUi, Draft};
use eframe::egui::{self, RichText};

impl ComparisonUi {
    pub(in crate::sharing) fn ui(&mut self, ctx: &egui::Context) {
        let mut cancel = false;
        let mut allow = false;
        if let Some(draft) = &mut self.draft {
            let response =
                egui::Modal::new(egui::Id::new(("compare-consent", draft.id))).show(ctx, |ui| {
                    ui.set_width((ctx.content_rect().width() - 80.).clamp(300., 680.));
                    cancel |= crate::dialogs::heading(ui, "Review agent comparison");
                    ui.label("Each recipient gets the same frozen image for this session.");
                    ui.separator();
                    egui::ScrollArea::vertical()
                        .id_salt(("compare-review", draft.id))
                        .auto_shrink([false, false])
                        .max_height((ctx.content_rect().height() - 300.).clamp(120., 500.))
                        .show(ui, |ui| draft.content(ui));
                    ui.separator();
                    ui.label(crate::copy::AGENT_PRIVACY);
                    ui.separator();
                    ui.horizontal_wrapped(|ui| {
                        cancel |= ui.button("Keep private").clicked();
                        allow = ui
                            .add_enabled(
                                draft.can_allow(),
                                egui::Button::new("Allow comparison for this session"),
                            )
                            .clicked();
                    });
                });
            cancel |= response.should_close();
        }
        if cancel {
            self.draft = None;
        } else if allow
            && let Err(error) = self.allow(ctx)
            && let Some(draft) = &mut self.draft
        {
            draft.error = Some(error);
        }
        if self.draft.is_none() {
            self.results(ctx);
        }
    }
}
impl Draft {
    fn content(&mut self, ui: &mut egui::Ui) {
        ui.label("Name two to four agents/applications you configure yourself. Labels identify token holders; OpenRadView does not verify their accounts or models.");
        let mut changed = false;
        for (i, label) in self.recipients.iter_mut().enumerate() {
            ui.label(format!("Recipient {}", i + 1));
            changed |= ui
                .add(
                    egui::TextEdit::singleline(label)
                        .hint_text("Your configured agent / application")
                        .char_limit(128)
                        .desired_width(f32::INFINITY),
                )
                .changed();
        }
        ui.horizontal_wrapped(|ui| {
            if self.recipients.len() < 4 && ui.button("Add recipient").clicked() {
                self.recipients.push(String::new());
                changed = true;
            }
            if self.recipients.len() > 2 && ui.button("Remove last recipient").clicked() {
                self.recipients.pop();
                changed = true;
            }
        });
        if changed {
            self.reviewed = false;
            self.risk = false;
        }
        ui.label("After consent, each recipient gets a separate 127.0.0.1 endpoint and temporary token. Connect it in your own local MCP-capable agent, then ask it to read the snapshot and submit experimental AI commentary. No provider is contacted or launched by OpenRadView.");
        ui.label("Allowed tools: read this frozen PNG and manifest; submit one final reply. No browsing other images, view control, file access, measurements or access to other recipients' replies.");
        if let Some(error) = self.error {
            ui.label(error);
        }
        if let Some(preview) = &self.preview {
            let size = preview.texture.size_vec2();
            ui.label(format!(
                "Exact shared PNG: {} × {} · no UI text or annotations",
                size.x as u32, size.y as u32
            ));
            ui.checkbox(&mut self.zoom, "Show preview at full pixel size");
            if self.zoom {
                egui::ScrollArea::both()
                    .id_salt(("compare-pixels", self.id))
                    .min_scrolled_height(270.)
                    .max_height(270.)
                    .show(ui, |ui| {
                        ui.image((preview.texture.id(), size));
                    });
            } else {
                let scale = (250. / size.y).min(ui.available_width() / size.x).min(1.);
                ui.image((preview.texture.id(), size * scale));
            }
            ui.label("Shared fields: source fingerprint, series index, exact SceneState, output size, geometry, renderer version, sampling budgets where applicable, and scene/PNG hashes. No raw DICOM tags, names, dates, original UIDs, descriptions or file paths.");
            egui::CollapsingHeader::new("Exact shared scene and geometry (JSON)").show(ui, |ui| {
                ui.add(egui::Label::new(RichText::new(&preview.json).small().monospace()).wrap());
            });
        } else if self.error.is_none() {
            ui.label("Preparing the frozen image and metadata locally…");
        }
        ui.add_space(8.);
        ui.label("Each agent may send the approved image and metadata to its provider. Tag removal cannot remove identifiers burned into pixels.");
        ui.label("Replies are untrusted experimental AI commentary, not medical interpretation. OpenRadView is for personal education/research, not diagnosis, treatment or clinical decisions.");
        ui.add_enabled_ui(self.preview.is_some(),|ui|{
            ui.checkbox(&mut self.reviewed,"I reviewed this exact image, manifest, all recipients and their limited tools.");
            ui.checkbox(&mut self.risk,"I accept the burned-in identifier and onward-sharing risks for every named recipient.");
        });
    }
}
