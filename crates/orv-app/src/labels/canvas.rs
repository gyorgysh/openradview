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
                if let Some(mesh) =
                    overlay_mesh(response.rect, image, view, texture.id(), self.opacity)
                {
                    ui.painter_at(response.rect).add(egui::Shape::mesh(mesh));
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
            let radius = brush_radius(response.rect, image, view, point, f64::from(self.radius))
                .unwrap_or(egui::Vec2::splat(6.));
            ui.painter_at(response.rect)
                .add(egui::Shape::ellipse_stroke(
                    pos,
                    radius,
                    egui::Stroke::new(1., crate::theme::PAPER),
                ));
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

fn overlay_mesh(
    rect: egui::Rect,
    image: &Frame,
    view: &ViewState,
    texture: egui::TextureId,
    opacity: f32,
) -> Option<egui::Mesh> {
    let mut mesh = egui::Mesh::with_texture(texture);
    for uv in [[0., 0.], [1., 0.], [1., 1.], [0., 1.]] {
        let pixel = [
            uv[0] * f64::from(image.width) - 0.5,
            uv[1] * f64::from(image.height) - 0.5,
        ];
        mesh.vertices.push(egui::epaint::Vertex {
            pos: screen_at(rect, image, view, pixel)?,
            uv: egui::pos2(uv[0] as f32, uv[1] as f32),
            color: egui::Color32::from_white_alpha((opacity * 255.) as u8),
        });
    }
    mesh.indices.extend([0, 1, 2, 0, 2, 3]);
    Some(mesh)
}
fn brush_radius(
    rect: egui::Rect,
    image: &Frame,
    view: &ViewState,
    point: [f64; 2],
    radius: f64,
) -> Option<egui::Vec2> {
    let center = screen_at(rect, image, view, point)?;
    let column = screen_at(rect, image, view, [point[0] + radius, point[1]])? - center;
    let row = screen_at(rect, image, view, [point[0], point[1] + radius])? - center;
    Some(egui::vec2(
        column.x.abs() + row.x.abs(),
        column.y.abs() + row.y.abs(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn label_texture_and_brush_follow_rotated_anisotropic_source_axes() {
        let mut image = (*orv_core::study::demo_study().series[0].frames[0]).clone();
        image.width = 8;
        image.height = 8;
        image.geometry.as_mut().unwrap().spacing = [2., 0.5];
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(200., 200.));
        for flip_horizontal in [false, true] {
            for flip_vertical in [false, true] {
                let view = ViewState {
                    transform: orv_core::DisplayTransform {
                        quarter_turns: 1,
                        flip_horizontal,
                        flip_vertical,
                    },
                    ..Default::default()
                };
                let mesh =
                    overlay_mesh(rect, &image, &view, egui::TextureId::Managed(1), 0.5).unwrap();
                for (index, [mut x, mut y]) in [[200., 75.], [200., 125.], [0., 125.], [0., 75.]]
                    .into_iter()
                    .enumerate()
                {
                    if flip_horizontal {
                        x = 200. - x;
                    }
                    if flip_vertical {
                        y = 200. - y;
                    }
                    assert!(mesh.vertices[index].pos.distance(egui::pos2(x, y)) < 0.0001);
                    assert_eq!(
                        mesh.vertices[index].uv,
                        [
                            egui::pos2(0., 0.),
                            egui::pos2(1., 0.),
                            egui::pos2(1., 1.),
                            egui::pos2(0., 1.)
                        ][index]
                    );
                }
                let radius = brush_radius(rect, &image, &view, [3., 3.], 1.).unwrap();
                assert!((radius.x - 25.).abs() < 0.0001);
                assert!((radius.y - 6.25).abs() < 0.0001);
            }
        }
    }
}
