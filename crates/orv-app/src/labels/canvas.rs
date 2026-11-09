// SPDX-License-Identifier: GPL-3.0-or-later
use super::{LabelUi, Operation};
use crate::annotations::{pixel_at, screen_at};
use eframe::egui;
use orv_core::{ViewState, study::Frame};
use std::sync::Arc;

#[derive(Clone)]
pub(super) struct BrushScope {
    map: Arc<orv_core::labels::Labels>,
    image: Arc<Frame>,
    view: ViewState,
    rect: egui::Rect,
    label: u8,
    radius: u32,
    erase: bool,
}
impl BrushScope {
    fn matches(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.map, &other.map)
            && Arc::ptr_eq(&self.image, &other.image)
            && self.view.plane == other.view.plane
            && self.view.slice == other.view.slice
            && self.view.zoom == other.view.zoom
            && self.view.pan == other.view.pan
            && self.view.transform == other.view.transform
            && self.rect == other.rect
            && self.label == other.label
            && self.radius == other.radius
            && self.erase == other.erase
    }
}
impl LabelUi {
    fn cancel_seed_capture(&mut self) {
        if self.regions.seeded.drawing() {
            self.regions.seeded.cancel_stroke();
            self.regions.invalidate();
            self.regions.picking = false;
            self.regions.seed_picking = None;
            self.open = true;
        }
    }
    pub fn canvas(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        image: &std::sync::Arc<Frame>,
        view: &ViewState,
        selected: usize,
    ) {
        if self.supplied_active {
            self.clear_brush();
            self.cancel_seed_capture();
            self.overlays.clear();
            self.supplied.canvas(ui, response, image, view, selected);
            return;
        }
        self.supplied.clear_overlays();
        if self.series != selected {
            self.clear_brush();
            self.cancel_seed_capture();
            return;
        }
        let Some(current) = self.current.clone() else {
            self.clear_brush();
            self.cancel_seed_capture();
            return;
        };
        let Some(z) = current.slice_index(image) else {
            self.clear_brush();
            self.cancel_seed_capture();
            return;
        };
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.stop_painting();
        }
        if self.regions.seeded.drawing()
            && !self
                .regions
                .seeded
                .drawing_on(&current, self.label, z, view, response.rect)
        {
            self.cancel_seed_capture();
        }
        self.sync_regions(&current, self.label, Some(z));
        let scope = BrushScope {
            map: current.clone(),
            image: image.clone(),
            view: view.clone(),
            rect: response.rect,
            label: self.label,
            radius: self.radius,
            erase: self.erase,
        };
        if self
            .brush_scope
            .as_ref()
            .is_some_and(|old| !old.matches(&scope))
        {
            self.clear_brush();
            self.message = "The image or brush settings changed; the stroke was discarded.".into();
        }
        self.prepare_planes(
            selected,
            &current.source_arc(),
            std::slice::from_ref(image),
            super::OverlayView::Source,
            ui.ctx(),
        );
        self.draw_plane(ui, response, image, view, 0);
        if self.visible || self.regions.picking {
            seed_markers(
                ui,
                response.rect,
                image,
                view,
                self.regions.seeded.markers(z),
            );
        }
        if self.task.is_some() || !self.painting(selected) {
            self.clear_brush();
            return;
        }
        if crate::viewport::navigation_override(ui, response) {
            self.clear_brush();
            self.regions.seeded.break_stroke();
            if !ui.input(|i| i.pointer.primary_down()) && self.regions.seeded.drawing() {
                self.regions.seeded.cancel_stroke();
                self.regions.invalidate();
            }
            return;
        }
        if self.regions.picking {
            if let Some(class) = self.regions.seed_picking {
                let down = ui.input(|i| i.pointer.primary_down());
                // Space may be pressed and released between two frames, so a
                // navigation gesture must not become a hint after it ends.
                if !down && ui.input(|i| i.key_pressed(egui::Key::Space)) {
                    if self.regions.seeded.drawing() {
                        self.regions.seeded.cancel_stroke();
                        self.regions.invalidate();
                    }
                    return;
                }
                let clicked = response.clicked_by(egui::PointerButton::Primary)
                    && !self.regions.seeded.drawing();
                let released = self.regions.seeded.drawing()
                    && ui.input(|i| i.pointer.button_released(egui::PointerButton::Primary));
                // A complete fast gesture can arrive in one frame. The final
                // pointer position alone would discard its earlier segments.
                let mut replayed = None;
                let start = if !down && !self.regions.seeded.drawing() {
                    ui.input(|input| {
                        let mut start = None;
                        for event in &input.events {
                            if let egui::Event::PointerButton {
                                pos,
                                button: egui::PointerButton::Primary,
                                pressed,
                                ..
                            } = event
                            {
                                if *pressed {
                                    start = Some(*pos);
                                } else if start.is_some() {
                                    return start;
                                }
                            }
                        }
                        None
                    })
                } else {
                    None
                };
                if start.is_some_and(|position| {
                    response.rect.contains(position)
                        && ui.clip_rect().contains(position)
                        && ui.ctx().layer_id_at(position) == Some(response.layer_id)
                }) {
                    ui.input(|input| {
                        let mut started = false;
                        for event in &input.events {
                            let (position, press, release) = match event {
                                egui::Event::PointerButton {
                                    pos,
                                    button: egui::PointerButton::Primary,
                                    pressed,
                                    ..
                                } => (*pos, *pressed, !*pressed),
                                egui::Event::PointerMoved(pos) if started => (*pos, false, false),
                                _ => continue,
                            };
                            if !started {
                                if !press {
                                    continue;
                                }
                                let Some(point) =
                                    hint_point(response.rect, image, view, position, z)
                                else {
                                    continue;
                                };
                                started = true;
                                replayed = Some(self.regions.seeded.begin_stroke(
                                    point,
                                    class,
                                    super::seeded::StrokeScope::new(
                                        &current,
                                        self.label,
                                        view,
                                        response.rect,
                                    ),
                                ));
                            } else if let Some(point) =
                                hint_point(response.rect, image, view, position, z)
                            {
                                replayed = Some(self.regions.seeded.extend_stroke(point));
                            } else {
                                self.regions.seeded.break_stroke();
                            }
                            if replayed == Some(false) || release {
                                break;
                            }
                        }
                    });
                    if let Some(accepted) = replayed {
                        self.regions.invalidate();
                        if !accepted {
                            self.regions.picking = false;
                            self.regions.seed_picking = None;
                            self.open = true;
                        }
                    }
                }
                if replayed.is_none()
                    && ((response.is_pointer_button_down_on() && down) || clicked || released)
                {
                    let point = response
                        .interact_pointer_pos()
                        .or_else(|| ui.input(|i| i.pointer.interact_pos()))
                        .and_then(|position| hint_point(response.rect, image, view, position, z));
                    if let Some(point) = point {
                        let accepted = if self.regions.seeded.drawing() {
                            self.regions.seeded.extend_stroke(point)
                        } else {
                            self.regions.seeded.begin_stroke(
                                point,
                                class,
                                super::seeded::StrokeScope::new(
                                    &current,
                                    self.label,
                                    view,
                                    response.rect,
                                ),
                            )
                        };
                        self.regions.invalidate();
                        if !accepted {
                            self.regions.picking = false;
                            self.regions.seed_picking = None;
                            self.open = true;
                        }
                    } else {
                        self.regions.seeded.break_stroke();
                    }
                }
                if self.regions.seeded.drawing() && !down {
                    self.regions.seeded.finish_stroke();
                    self.regions.picking = false;
                    self.regions.seed_picking = None;
                    self.regions.invalidate();
                    self.sync_regions(&current, self.label, Some(z));
                    self.open = true;
                }
                return;
            }
            if response.clicked()
                && let Some(position) = response.interact_pointer_pos()
                && let Some(point) = pixel_at(response.rect, image, view, position)
            {
                let [x, y] = point.map(|v| (v + 0.5).floor());
                if x >= 0. && y >= 0. && x < image.width as f64 && y < image.height as f64 {
                    self.regions.pick([x as usize, y as usize, z]);
                    self.sync_regions(&current, self.label, Some(z));
                    self.open = true;
                }
            }
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
        }
        // A short drag may be delivered as press, movement and release in one
        // frame, so the final pointer state alone cannot recover its path.
        let mut released = false;
        if !ui.is_enabled() || ui.input(|i| !i.focused || i.key_pressed(egui::Key::Space)) {
            self.clear_brush();
        } else {
            let events = ui.input(|input| {
                input
                    .events
                    .iter()
                    .filter_map(|event| match event {
                        egui::Event::PointerButton {
                            pos,
                            button: egui::PointerButton::Primary,
                            pressed,
                            ..
                        } => Some((Some(*pos), *pressed, !*pressed)),
                        egui::Event::PointerMoved(pos) => Some((Some(*pos), false, false)),
                        egui::Event::PointerGone => Some((None, false, false)),
                        _ => None,
                    })
                    .take(8193)
                    .collect::<Vec<_>>()
            });
            if events.len() > 8192 {
                self.clear_brush();
                self.message = "Brush input was too long; no edit was applied.".into();
                return;
            }
            for (pos, press, release) in events {
                let Some(pos) = pos else {
                    self.stroke_outside = !self.stroke.is_empty();
                    continue;
                };
                if press
                    && (!self.stroke.is_empty()
                        || !response.rect.contains(pos)
                        || !ui.clip_rect().contains(pos)
                        || ui.ctx().layer_id_at(pos) != Some(response.layer_id))
                {
                    continue;
                }
                if !press && self.stroke.is_empty() {
                    continue;
                }
                let point = (response.rect.contains(pos) && ui.clip_rect().contains(pos))
                    .then(|| pixel_at(response.rect, image, view, pos))
                    .flatten();
                if let Some(point) = point
                    && self.stroke.last().is_none_or(|old| {
                        self.stroke_outside
                            || (release && *old != point)
                            || (old[0] - point[0]).hypot(old[1] - point[1]) >= 0.5
                    })
                {
                    if self.stroke.len() == 8192 {
                        self.clear_brush();
                        self.message = "Brush input was too long; no edit was applied.".into();
                        return;
                    }
                    if press {
                        self.brush_scope = Some(scope.clone());
                    }
                    if self.stroke_outside {
                        self.stroke_breaks.push(self.stroke.len());
                        self.stroke_outside = false;
                    }
                    self.stroke.push(point);
                } else if point.is_none() && !self.stroke.is_empty() {
                    // Do not interpolate through the image when the pointer
                    // leaves the displayed footprint and later returns.
                    self.stroke_outside = true;
                }
                if release {
                    released = true;
                    break;
                }
            }
        }
        if released && !self.stroke.is_empty() {
            let points = std::mem::take(&mut self.stroke);
            let breaks = std::mem::take(&mut self.stroke_breaks);
            self.start(
                Operation::Paint(
                    current,
                    z,
                    points,
                    breaks,
                    self.radius,
                    if self.erase { 0 } else { self.label },
                ),
                ui.ctx(),
            );
        } else if !ui.input(|i| i.pointer.primary_down()) {
            self.clear_brush();
        }
    }
}

fn hint_point(
    rect: egui::Rect,
    image: &Frame,
    view: &ViewState,
    position: egui::Pos2,
    z: usize,
) -> Option<[usize; 3]> {
    if !rect.contains(position) {
        return None;
    }
    let [x, y] = pixel_at(rect, image, view, position)?.map(|v| (v + 0.5).floor());
    (x >= 0. && y >= 0. && x < f64::from(image.width) && y < f64::from(image.height))
        .then_some([x as usize, y as usize, z])
}

fn seed_markers(
    ui: &egui::Ui,
    rect: egui::Rect,
    image: &Frame,
    view: &ViewState,
    seeds: &[orv_core::labels::SeedPoint],
) {
    let Some(mapping) = crate::annotations::mapping(rect, image, view) else {
        return;
    };
    let mut mesh = egui::Mesh::default();
    for seed in seeds {
        let Some(uv) = mapping.pixel_to_view([seed.position[0] as f64, seed.position[1] as f64])
        else {
            continue;
        };
        let pos = rect.min + egui::vec2(uv[0] as f32 * rect.width(), uv[1] as f32 * rect.height());
        if !rect.contains(pos) {
            continue;
        }
        mesh.add_colored_rect(
            egui::Rect::from_center_size(pos, egui::vec2(6., 6.)),
            crate::theme::INK,
        );
        match seed.class {
            orv_core::labels::SeedClass::Include => mesh.add_colored_rect(
                egui::Rect::from_center_size(pos, egui::vec2(3., 3.)),
                crate::theme::PAPER,
            ),
            orv_core::labels::SeedClass::Exclude => {
                mesh.add_colored_rect(
                    egui::Rect::from_center_size(pos, egui::vec2(4., 1.5)),
                    crate::theme::PAPER,
                );
                mesh.add_colored_rect(
                    egui::Rect::from_center_size(pos, egui::vec2(1.5, 4.)),
                    crate::theme::PAPER,
                );
            }
        }
    }
    if !mesh.vertices.is_empty() {
        ui.painter_at(rect).add(egui::Shape::mesh(mesh));
    }
}

pub(crate) fn overlay_mesh(
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
    use orv_core::labels::{SeedClass, SeedPoint};

    #[test]
    fn seed_markers_follow_source_transform_and_are_clipped_to_the_view() {
        let mut image = (*orv_core::study::demo_study().series[0].frames[0]).clone();
        image.width = 8;
        image.height = 8;
        let seeds = [
            SeedPoint {
                position: [1, 2],
                class: SeedClass::Include,
            },
            SeedPoint {
                position: [5, 6],
                class: SeedClass::Exclude,
            },
            SeedPoint {
                position: [80, 80],
                class: SeedClass::Include,
            },
        ];
        for turns in 0..4 {
            let view = ViewState {
                transform: orv_core::DisplayTransform {
                    quarter_turns: turns,
                    flip_horizontal: true,
                    flip_vertical: false,
                },
                ..Default::default()
            };
            let rect = egui::Rect::from_min_size(egui::pos2(10., 20.), egui::vec2(400., 400.));
            let ctx = egui::Context::default();
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                seed_markers(ui, rect, &image, &view, &seeds);
            });
            output.textures_delta.clear();
            let meshes: Vec<_> = output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::Shape::Mesh(mesh) => Some(mesh),
                    _ => None,
                })
                .collect();
            assert_eq!(meshes.len(), 1);
            assert_eq!(meshes[0].vertices.len(), 20);
            let expected = screen_at(rect, &image, &view, [1., 2.]).unwrap();
            let corners = &meshes[0].vertices[..4];
            let center = corners
                .iter()
                .fold(egui::Vec2::ZERO, |sum, v| sum + v.pos.to_vec2())
                / 4.;
            assert!((center - expected.to_vec2()).length() < 0.01);
            assert_eq!(
                meshes[0]
                    .vertices
                    .iter()
                    .filter(|v| v.color == crate::theme::INK)
                    .count(),
                8
            );
            assert_eq!(
                meshes[0]
                    .vertices
                    .iter()
                    .filter(|v| v.color == crate::theme::PAPER)
                    .count(),
                12
            );
        }
    }

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
