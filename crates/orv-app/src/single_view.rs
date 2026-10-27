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
        if ui.is_enabled() && !ui.ctx().egui_wants_keyboard_input() {
            ui.input(|i| {
                if i.key_pressed(egui::Key::ArrowDown) {
                    self.view.slice = (self.view.slice + 1).min(count as u32 - 1);
                }
                if i.key_pressed(egui::Key::ArrowUp) {
                    self.view.slice = self.view.slice.saturating_sub(1);
                }
                if i.key_pressed(egui::Key::R) {
                    self.view.zoom = 1.0;
                    self.view.pan = [0.0, 0.0];
                }
                if i.key_pressed(egui::Key::I) {
                    self.view.invert = !self.view.invert;
                }
                if !color && i.key_pressed(egui::Key::W) {
                    self.tool = Tool::Window;
                }
                if i.key_pressed(egui::Key::P) {
                    self.tool = Tool::Pan;
                }
                if i.key_pressed(egui::Key::Z) {
                    self.tool = Tool::Zoom;
                }
                if i.key_pressed(egui::Key::S) {
                    self.tool = Tool::Browse;
                }
            });
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
            self.image_info
                .open(image, index, self.view.clone(), ui.ctx().clone());
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
        if color {
            let description = self.current_image().and_then(|(_, image)| {
                image.color.as_ref().map(|data| {
                    format!(
                        "RGB · {}-bit codes · {}",
                        data.info.bits,
                        data.info.presentation()
                    )
                })
            });
            ui.horizontal_wrapped(|ui| {
                ui.label(description.unwrap_or_else(|| "RGB color".into()));
                ui.checkbox(&mut self.view.invert, "Invert [I]");
                if ui.button("Reset display").clicked() {
                    self.view.invert = false;
                }
            });
        } else {
            let current = self.current_image().map(|(_, frame)| frame);
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
        if self.view.window != before {
            self.initialize_window = false;
        }
        if self.labels.painting(self.selected) {
            ui.horizontal(|ui| {
                ui.weak("Label brush active · Escape to stop");
                if ui.small_button("Stop brush").clicked() {
                    self.labels.stop_painting();
                }
            });
        }
        let size = Vec2::new(content_width, (ui.available_height() - 62.0).max(60.0));
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
            ui.add(
                egui::Slider::new(&mut self.view.slice, 0..=count as u32 - 1)
                    .text("Image")
                    .custom_formatter(|v, _| format!("{} / {}", v as u32 + 1, count)),
            );
            if self.view != before_input || self.tool != before_tool
                || (response.hovered() && ui.input(|i| i.pointer.any_down()))
                || self.labels.open || self.labels.painting(self.selected)
                || self.image_info.is_open() || !ui.is_enabled()
                || ui.input(|i| !i.focused || i.viewport().minimized == Some(true) || i.key_pressed(egui::Key::Escape))
            {
                self.playback.pause();
            }
            if count > 1 {
                let allowed = ui.is_enabled() && self.loading.is_none() && !self.labels.open && !painting && !self.image_info.is_open();
                if ui.add_enabled(allowed, egui::Button::new(if self.playback.running { "Pause" } else { "Play" })).clicked() {
                    if self.playback.running {
                        self.playback.pause();
                    } else {
                        if self.view.slice as usize == count - 1 { self.view.slice = 0; }
                        self.playback.start();
                    }
                }
                ui.add(egui::DragValue::new(&mut self.playback.fps).range(1..=30).suffix(" fps"))
                    .on_hover_text("Requested browsing speed. Slower decoding reduces playback speed; images are not skipped. This is not acquisition timing.");
                ui.checkbox(&mut self.playback.repeat, "Loop");
            }
        });
        if self
            .decode_error
            .as_ref()
            .is_some_and(|(i, _)| *i == self.view.slice as usize)
        {
            self.playback.pause();
        }
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
        if self.labels.painting(self.selected) {
            ui.weak("Brush: draw labels · Wheel / pinch: zoom · Right / Space drag: move · ↑↓: images · Escape: stop");
        } else {
            controls::navigation_hint(ui, self.tool);
        }
        self.request_image(ui.ctx());
    }
}
