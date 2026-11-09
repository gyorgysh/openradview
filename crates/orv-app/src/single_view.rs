// SPDX-License-Identifier: GPL-3.0-or-later
use crate::{
    controls, theme,
    viewport::{self, Tool},
    workspace::Workspace,
};
use eframe::egui::{self, RichText, Vec2};

impl Workspace {
    pub(super) fn image_ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.poll_image(ui.ctx());
        let timing = self.playback_metadata();
        let before_input = self.view.clone();
        let before_tool = self.tool;
        let content_width = ui.available_width();
        let Some(series) = self
            .study
            .as_ref()
            .and_then(|s| s.series.get(self.selected))
        else {
            return;
        };
        let color = self.current_image().is_some_and(|(_, f)| f.rgba.is_some());
        if color && self.tool == Tool::Window {
            self.tool = Tool::Pan;
        }
        let count = series.frame_count();
        if count == 0 {
            return;
        }
        crate::actions::tool_input(ui, &mut self.tool, !color, false, true);
        self.view.slice = crate::actions::navigate(ui, self.view.slice, count as u32 - 1);
        if crate::actions::pressed(ui, crate::actions::Action::Fit) {
            self.view.zoom = 1.;
            self.view.pan = [0.; 2];
        }
        if crate::actions::pressed(ui, crate::actions::Action::Invert) {
            self.view.invert = !self.view.invert;
        }
        let mut inspect = false;
        if let Some((index, image)) = self.current_image() {
            let mut displayed_view = self.view.clone();
            displayed_view.slice = index as u32;
            self.annotations.bind_image(&image, &displayed_view);
            crate::view_context::show(ui, &image);
        }
        if controls::tools(ui, &mut self.tool, false, !color, |ui| {
            controls::orientation(ui, &mut self.view.transform);
            if ui
                .selectable_label(self.labels.painting(self.selected), "Labels…")
                .clicked()
            {
                self.labels.open = true;
            }
            inspect = ui.button("Image info…").clicked();
            self.annotations.controls(ui);
            crate::toolbar::label(
                ui,
                egui::RichText::new(format!("{:.0}%", self.view.zoom * 100.0))
                    .small()
                    .color(theme::SILVER),
            );
        }) {
            self.view.zoom = 1.0;
            self.view.pan = [0.0, 0.0];
        }
        if inspect && let Some((index, image)) = self.current_image() {
            self.playback.pause();
            self.image_info.open(
                image,
                std::sync::Arc::new(series.clone()),
                index,
                self.view.clone(),
                ui.ctx().clone(),
            );
        }
        let original = self
            .current_image()
            .map(|(_, f)| f.default_window)
            .or_else(|| {
                series
                    .source
                    .as_ref()
                    .and_then(|s| s.original_window(self.view.slice as usize))
            })
            .unwrap_or(self.view.window);
        let before = self.view.window;
        // In short windows these rows collapse so the image keeps its room.
        let current = self.current_image().map(|(_, frame)| frame);
        let description = current.as_ref().and_then(|image| {
            image.color.as_ref().map(|data| {
                format!(
                    "RGB · {}-bit codes · {}",
                    data.info.bits,
                    data.info.presentation()
                )
            })
        });
        let monochrome1 = current.as_ref().is_some_and(|image| image.monochrome1);
        let layers = current.clone().filter(|image| image.presentation.is_some());
        let mut applied = false;
        controls::display_panel(ui, "image-display-controls", |ui| {
            if color {
                ui.horizontal_wrapped(|ui| {
                    ui.label(description.unwrap_or_else(|| "RGB color".into()));
                    ui.checkbox(
                        &mut self.view.invert,
                        crate::actions::label(ui.ctx(), crate::actions::Action::Invert),
                    );
                    if ui.button("Reset display").clicked() {
                        self.view.invert = false;
                    }
                });
            } else {
                let choices = current
                    .as_ref()
                    .and_then(|frame| frame.scalar.as_ref())
                    .map(|data| &**data.choices());
                controls::window(
                    ui,
                    &mut self.view.window,
                    &mut self.view.invert,
                    original,
                    choices,
                );
            }
            applied = crate::toolbar::row(ui, |ui| {
                let applied = (!color)
                    && self
                        .presentation
                        .controls(ui, series, &mut self.view, monochrome1);
                if let Some(image) = &layers {
                    self.source_layers.controls(ui, image);
                }
                applied
            })
            .inner;
        });
        if self.view.window != before {
            self.initialize_window = false;
        }
        if applied || self.presentation.follow(&mut self.view) {
            self.initialize_window = false;
            ui.ctx().request_repaint();
        }
        if self.labels.painting(self.selected) {
            ui.horizontal(|ui| {
                ui.weak(self.labels.pointer_hint());
                if ui.small_button("Stop label tool").clicked() {
                    self.labels.stop_painting();
                }
            });
        }
        if self.labels.region_preview_visible() {
            ui.weak("Region edit preview only · Open Labels… to Apply or Discard");
        }
        // Short windows drop the gesture hint (also in Help) to keep image room.
        let compact = ui.available_height() < 420.;
        let footer = if count > 1
            && timing
                .as_ref()
                .is_some_and(|m| !matches!(m, orv_core::acquisition::CineMetadata::Absent))
        {
            140.
        } else {
            62.
        } - if compact { 22. } else { 0. };
        let size = Vec2::new(content_width, (ui.available_height() - footer).max(60.0));
        let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());
        let painting = self.labels.painting(self.selected);
        if !painting
            || viewport::navigation_override(ui, &response)
            || (!ui.input(|i| i.pointer.primary_down()) && !response.double_clicked())
        {
            viewport::interact(
                ui,
                &response,
                &mut self.view,
                if painting { Tool::Zoom } else { self.tool },
                count,
                &mut self.scroll,
            );
        }
        let displayed = self.current_image();
        if let Some((index, image)) = &displayed {
            self.picture_size = Some(crate::rendered::picture_size(
                rect.size() * ui.ctx().pixels_per_point(),
            ));
            let mut rendered_view = self.view.clone();
            rendered_view.slice = *index as u32;
            self.last_image_view = Some(rendered_view.clone());
            viewport::draw(
                ui,
                frame,
                rect,
                image.clone(),
                &rendered_view,
                0,
                if series.modality == "NIFTI" {
                    "NIFTI SOURCE GRID · RESEARCH ONLY"
                } else {
                    "AS ACQUIRED · RESEARCH ONLY"
                },
            );
            self.source_layers
                .draw(ui, &response, image, &rendered_view);
            crate::image_overlay::draw(ui, &response, image, &rendered_view);
            if *index == self.view.slice as usize {
                if !painting {
                    self.annotations
                        .show(ui, &response, image, &rendered_view, self.tool);
                }
                self.labels
                    .canvas(ui, &response, image, &rendered_view, self.selected);
            }
        } else {
            ui.painter().rect_filled(rect, 0., theme::INK);
        }
        let pending = displayed
            .as_ref()
            .is_none_or(|(i, _)| *i != self.view.slice as usize);
        if pending {
            let message = self
                .decode_error
                .as_ref()
                .filter(|(i, _)| *i == self.view.slice as usize)
                .map_or_else(
                    || format!("Loading image {} locally…", self.view.slice + 1),
                    |(_, e)| e.to_string(),
                );
            ui.put(
                egui::Rect::from_min_size(
                    rect.min + egui::vec2(12., 12.),
                    egui::vec2((rect.width() - 24.).max(20.), 56.),
                ),
                egui::Label::new(RichText::new(message).color(theme::SILVER)).wrap(),
            );
        }
        if !pending {
            self.annotations.status_overlay(ui, rect, self.tool);
        }
        let failed = self
            .decode_error
            .as_ref()
            .is_some_and(|(i, _)| *i == self.view.slice as usize);
        if failed {
            self.playback.pause();
        }
        let foreground = ui.input(|i| i.focused && i.viewport().minimized != Some(true));
        let allowed = ui.is_enabled()
            && foreground
            && self.loading.is_none()
            && !self.labels.open
            && !painting
            && !self.image_info.is_open()
            && !failed;
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new(displayed.as_ref().map_or_else(
                    || "Loading…".into(),
                    |(_, image)| format!("{} × {}", image.width, image.height),
                ))
                .monospace()
                .size(11.0)
                .color(theme::SILVER),
            );
            controls::image_number(ui, &mut self.view.slice, count);
            if self.view != before_input
                || self.tool != before_tool
                || (response.hovered() && ui.input(|i| i.pointer.any_down()))
                || self.labels.open
                || self.labels.painting(self.selected)
                || self.image_info.is_open()
                || !ui.is_enabled()
                || ui.input(|i| {
                    !i.focused
                        || i.viewport().minimized == Some(true)
                        || i.key_pressed(egui::Key::Escape)
                })
            {
                self.playback.pause();
            }
            if count > 1 {
                self.playback
                    .buttons(ui, &mut self.view.slice, count, allowed);
            }
        });
        if count > 1
            && let Some(enabled) = self.playback.source_controls(ui, timing.as_ref(), allowed)
        {
            self.set_source_playback(enabled);
        }
        let was_running = self.playback.running;
        match self.playback.tick(
            std::time::Instant::now(),
            self.view.slice,
            displayed.as_ref().map(|(i, _)| *i),
            count,
        ) {
            crate::playback::Step::Wait(delay) => ui.ctx().request_repaint_after(delay),
            crate::playback::Step::Advance(index) => {
                self.view.slice = index;
                ui.ctx().request_repaint();
            }
            crate::playback::Step::Idle => (),
        }
        if was_running != self.playback.running {
            ui.ctx().request_repaint();
        }
        if self.labels.painting(self.selected) {
            ui.weak(self.labels.pointer_controls());
        } else if !compact {
            controls::navigation_hint(ui, self.tool);
        }
        self.request_image(ui.ctx());
    }
}
