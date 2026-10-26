// SPDX-License-Identifier: GPL-3.0-or-later
use super::{LabelUi, Operation};
use crate::annotations::{pixel_at, screen_at};
use eframe::egui;
use orv_core::{ViewState, study::Frame};
impl LabelUi {
    pub fn canvas(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        image: &Frame,
        view: &ViewState,
        selected: usize,
    ) {
        if self.series != selected {
            return;
        }
        let Some(current) = self.current.clone() else {
            return;
        };
        let Some(z) = current.slice_index(image) else {
            return;
        };
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.paint = false;
            self.stroke.clear();
        }
        if self.stroke_slice != Some(z) {
            self.stroke.clear();
            self.stroke_slice = Some(z);
        }
        if self.visible {
            if let Some((slice, texture)) = &self.texture
                && *slice == z
            {
                let w = image.width as f64;
                let h = image.height as f64;
                if let (Some(min), Some(max)) = (
                    screen_at(response.rect, image, view, [-0.5, -0.5]),
                    screen_at(response.rect, image, view, [w - 0.5, h - 0.5]),
                ) {
                    ui.painter_at(response.rect).image(
                        texture.id(),
                        egui::Rect::from_min_max(min, max),
                        egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1., 1.)),
                        egui::Color32::from_white_alpha((self.opacity * 255.) as u8),
                    );
                }
            } else if self.task.is_none() {
                self.start(Operation::Overlay(current.clone(), z), ui.ctx());
            }
        }
        if self.task.is_some()
            || !self.painting(selected)
            || crate::viewport::navigation_override(ui, response)
        {
            return;
        }
        if (response.hovered() || response.dragged_by(egui::PointerButton::Primary))
            && let Some(pos) = response.interact_pointer_pos()
            && let Some(point) = pixel_at(response.rect, image, view, pos)
        {
            let cursor = screen_at(
                response.rect,
                image,
                view,
                [point[0] + self.radius as f64, point[1]],
            )
            .map_or(6., |edge| edge.distance(pos));
            ui.painter_at(response.rect).circle_stroke(
                pos,
                cursor,
                egui::Stroke::new(1., crate::theme::PAPER),
            );
            if ui.input(|i| i.pointer.primary_down())
                && self.stroke.len() < 8192
                && self
                    .stroke
                    .last()
                    .is_none_or(|old| (old[0] - point[0]).hypot(old[1] - point[1]) >= 0.5)
            {
                self.stroke.push(point);
            }
            if response.clicked() && self.stroke.is_empty() {
                self.stroke.push(point);
            }
        }
        if !self.stroke.is_empty() && !ui.input(|i| i.pointer.primary_down()) {
            let points = std::mem::take(&mut self.stroke);
            self.start(
                Operation::Paint(
                    current,
                    z,
                    points,
                    self.radius,
                    if self.erase { 0 } else { self.label },
                ),
                ui.ctx(),
            );
        }
    }
}
