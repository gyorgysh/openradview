// SPDX-License-Identifier: GPL-3.0-or-later
use crate::{
    annotations,
    mpr::MprSession,
    mpr_worker::MprSnapshot,
    theme,
    viewport::{self, Tool},
};
use eframe::egui::{self, Rect, Vec2};
use orv_core::study::Frame;
use std::sync::Arc;

pub(super) struct PlanePaint {
    ui: egui::Ui,
    response: egui::Response,
    image: Arc<Frame>,
    index: usize,
}
impl MprSession {
    pub(super) fn prepare_plane(
        &mut self,
        parent: &mut egui::Ui,
        index: usize,
        rect: Rect,
        snapshot: &MprSnapshot,
    ) -> PlanePaint {
        let mut ui = parent.new_child(
            egui::UiBuilder::new()
                .id_salt(("mpr", index))
                .max_rect(rect),
        );
        ui.set_clip_rect(rect);
        let (name, axis) = match index {
            0 => ("AXIAL", 2),
            1 => ("CORONAL", 1),
            _ => ("SAGITTAL", 0),
        };
        let heading = if snapshot.images[index].slab.is_some() {
            format!("{name} · SLAB PROJECTION")
        } else {
            name.into()
        };
        theme::eyebrow(&mut ui, &heading);
        let size = Vec2::new(ui.available_width(), (ui.available_height() - 32.).max(30.));
        let (_, response) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());
        if response.hovered() {
            self.active = index;
        }
        let [min, max] = snapshot.volume.bounds();
        let count = (((max[axis] - min[axis]) / snapshot.volume.step_mm()).ceil() as u32)
            .clamp(1, 8191)
            + 1;
        let step = (max[axis] - min[axis]) / (count - 1) as f64;
        let point = self.target.unwrap_or(snapshot.point);
        let initial = ((point[axis] - min[axis]) / step)
            .round()
            .clamp(0., (count - 1) as f64) as u32;
        self.views[index].slice = initial;
        self.views[index].window = self.window;
        if ui.is_enabled() && self.active == index && !ui.ctx().egui_wants_keyboard_input() {
            ui.input(|i| {
                if i.key_pressed(egui::Key::ArrowDown) {
                    self.views[index].slice = (initial + 1).min(count - 1);
                }
                if i.key_pressed(egui::Key::ArrowUp) {
                    self.views[index].slice = initial.saturating_sub(1);
                }
            });
        }
        viewport::interact(
            &mut ui,
            &response,
            &mut self.views[index],
            self.tool,
            count as usize,
            &mut self.scroll[index],
        );
        self.window = self.views[index].window;
        if initial != self.views[index].slice {
            let mut next = point;
            next[axis] = min[axis] + step * self.views[index].slice as f64;
            self.request(&snapshot.volume, next);
        }
        let image = snapshot.images[index].clone();
        if self.tool == Tool::Crosshair
            && !viewport::navigation_override(&ui, &response)
            && (response.clicked_by(egui::PointerButton::Primary)
                || response.dragged_by(egui::PointerButton::Primary))
            && let Some(pos) = response.interact_pointer_pos()
            && let Some(pixel) =
                annotations::pixel_at(response.rect, &image, &self.views[index], pos)
            && let Some(g) = &image.geometry
            && let Ok(mut world) = g.pixel_to_world(pixel)
            && if snapshot.volume.is_color() {
                snapshot.volume.sample_rgb(world).is_some()
            } else {
                snapshot.volume.sample(world).is_some()
            }
        {
            world[axis] = self.target.unwrap_or(snapshot.point)[axis];
            self.request(&snapshot.volume, world);
        }
        let mut coordinate = self.target.unwrap_or(snapshot.point)[axis];
        let mut changed = false;
        ui.horizontal(|ui| {
            ui.spacing_mut().slider_width = (ui.available_width() - 110.).max(30.);
            changed = ui
                .add(
                    egui::Slider::new(&mut coordinate, min[axis]..=max[axis])
                        .show_value(false)
                        .text("mm"),
                )
                .changed();
            changed |= ui
                .add(
                    egui::DragValue::new(&mut coordinate)
                        .range(min[axis]..=max[axis])
                        .speed(step)
                        .fixed_decimals(1),
                )
                .changed();
        });
        if changed {
            let mut next = self.target.unwrap_or(snapshot.point);
            next[axis] = coordinate;
            self.request(&snapshot.volume, next);
        }
        PlanePaint {
            ui,
            response,
            image,
            index,
        }
    }
    pub(super) fn finish_plane(
        &mut self,
        mut paint: PlanePaint,
        frame: &eframe::Frame,
        snapshot: &MprSnapshot,
    ) {
        let PlanePaint {
            ref mut ui,
            ref response,
            ref image,
            index,
        } = paint;
        let view = &self.views[index];
        self.picture_sizes[index] = Some(crate::rendered::picture_size(
            response.rect.size() * ui.ctx().pixels_per_point(),
        ));
        viewport::draw(ui, frame, response.rect, image.clone(), view, index + 1, "");
        let painter = ui.painter_at(response.rect);
        if self.active_index() == index {
            painter.rect_stroke(
                response.rect,
                0.,
                egui::Stroke::new(1.5, theme::SMOKE),
                egui::StrokeKind::Inside,
            );
        }
        if let Some(g) = &image.geometry
            && let Ok(pixel) = g.world_to_pixel(snapshot.point)
            && let Some(center) = annotations::screen_at(response.rect, image, view, pixel)
        {
            let stroke = egui::Stroke::new(1., theme::SILVER);
            painter.line_segment(
                [
                    egui::pos2(response.rect.left(), center.y),
                    egui::pos2(response.rect.right(), center.y),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    egui::pos2(center.x, response.rect.top()),
                    egui::pos2(center.x, response.rect.bottom()),
                ],
                stroke,
            );
        }
        crate::image_overlay::draw(ui, response, image, view);
        if self.is_settled() {
            self.annotations[index].show(ui, response, image, view, self.tool);
        }
        self.annotations[index].status_overlay(ui, response.rect, self.tool);
    }
}
