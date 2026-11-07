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
    #[cfg(test)]
    pub(super) slider_rect: Rect,
}
#[cfg(test)]
impl PlanePaint {
    pub(super) fn image_rect(&self) -> Rect {
        self.response.rect
    }
}
impl MprSession {
    pub(super) fn prepare_plane(
        &mut self,
        parent: &mut egui::Ui,
        index: usize,
        rect: Rect,
        snapshot: &MprSnapshot,
        labels: &mut crate::labels::LabelUi,
        selected: usize,
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
        if labels
            .physical_drawing_plane()
            .is_none_or(|plane| plane == index)
            && response.hovered()
            && ui.input(|input| {
                input.pointer.delta() != egui::Vec2::ZERO || input.pointer.any_pressed()
            })
        {
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
        if self.active == index {
            if crate::actions::pressed(&ui, crate::actions::Action::Next) {
                self.views[index].slice = initial.saturating_add(1).min(count - 1);
            }
            if crate::actions::pressed(&ui, crate::actions::Action::Previous) {
                self.views[index].slice = initial.saturating_sub(1);
            }
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
        // The release can finish a physical path in this same input pass.
        // Keep that gesture out of crosshair navigation even after it finishes.
        let physical_drawing = labels.physical_drawing();
        if physical_drawing {
            labels.sync_physical(&self.physical_context(snapshot, selected));
            labels.physical_pointer(&ui, &response, &image, &self.views[index], index);
        }
        if !physical_drawing
            && self.tool == Tool::Crosshair
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
        #[cfg(test)]
        let mut slider_rect = Rect::NOTHING;
        ui.horizontal(|ui| {
            ui.spacing_mut().slider_width = (ui.available_width() - 110.).max(30.);
            let track_width = ui.spacing().slider_width;
            let slider = ui.add(
                egui::Slider::new(&mut coordinate, min[axis]..=max[axis])
                    .show_value(false)
                    .text("mm"),
            );
            changed = slider.changed();
            if !changed && let Some(fraction) = quick_slider_release(ui, &slider, track_width) {
                coordinate = min[axis] + (max[axis] - min[axis]) * f64::from(fraction);
                changed = true;
            }
            #[cfg(test)]
            {
                slider_rect = slider.rect;
            }
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
            #[cfg(test)]
            slider_rect,
        }
    }
    pub(super) fn finish_plane(
        &mut self,
        mut paint: PlanePaint,
        frame: &eframe::Frame,
        snapshot: &MprSnapshot,
        labels: &crate::labels::LabelUi,
    ) {
        let PlanePaint {
            ref mut ui,
            ref response,
            ref image,
            index,
            #[cfg(test)]
                slider_rect: _,
        } = paint;
        let view = &self.views[index];
        self.picture_sizes[index] = Some(crate::rendered::picture_size(
            response.rect.size() * ui.ctx().pixels_per_point(),
        ));
        let display = snapshot.gpu_images[index].as_ref().map_or_else(
            || viewport::SliceImage::Cpu(image.clone()),
            |gpu| viewport::SliceImage::Gpu(gpu.clone()),
        );
        viewport::draw(ui, frame, response.rect, display, view, index + 1, "");
        labels.draw_plane(ui, response, image, view, index);
        labels.physical_pointer_guide(ui, response, image, view);
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
        if self.is_settled() && !labels.physical_drawing() {
            self.annotations[index].show(ui, response, image, view, self.tool);
            self.annotations[index].status_overlay(ui, response.rect, self.tool);
        } else {
            self.annotations[index].cancel_interaction(ui.ctx());
        }
    }
}

// A complete drag can arrive between two UI passes. In that case the slider
// may never acquire the pointer. The press must begin on its visible rail.
fn quick_slider_release(ui: &egui::Ui, slider: &egui::Response, track_width: f32) -> Option<f32> {
    use egui::{Event, PointerButton};

    if !ui.is_enabled() || slider.interact_pointer_pos().is_some() {
        return None;
    }
    let track = Rect::from_min_size(
        slider.rect.min,
        Vec2::new(track_width, slider.rect.height()),
    );
    let (start, end, definitely_dragged) = ui.input(|input| {
        if input.key_pressed(egui::Key::Escape) {
            return None;
        }
        let mut start = None;
        let mut end = None;
        let mut moved = false;
        for event in &input.events {
            match event {
                Event::PointerButton {
                    pos,
                    button: PointerButton::Primary,
                    pressed: true,
                    modifiers,
                } if *modifiers == egui::Modifiers::NONE && start.is_none() => start = Some(*pos),
                Event::PointerMoved(_) if start.is_some() && end.is_none() => moved = true,
                Event::PointerButton {
                    pos,
                    button: PointerButton::Primary,
                    pressed: false,
                    modifiers,
                } if *modifiers == egui::Modifiers::NONE && start.is_some() && end.is_none() => {
                    end = Some(*pos)
                }
                Event::PointerButton { .. } | Event::PointerGone | Event::WindowFocused(false) => {
                    return None;
                }
                _ => {}
            }
        }
        moved.then_some((start?, end?, !input.pointer.could_any_button_be_click()))
    })?;
    if !track.contains(start)
        || (!track.contains(end) && !definitely_dragged)
        || !ui.clip_rect().contains(start)
        || ui.ctx().layer_id_at(start) != Some(slider.layer_id)
        || ui.ctx().layer_id_at(end) != Some(slider.layer_id)
    {
        return None;
    }
    let radius = track.height() * 0.4;
    let radius = match ui.visuals().handle_shape {
        egui::style::HandleShape::Circle => radius,
        egui::style::HandleShape::Rect { aspect_ratio } => radius * aspect_ratio,
    };
    let width = track.width() - 2. * radius;
    (width > 0.).then(|| ((end.x - track.left() - radius) / width).clamp(0., 1.))
}
