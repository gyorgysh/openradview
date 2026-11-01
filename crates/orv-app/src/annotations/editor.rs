// SPDX-License-Identifier: GPL-3.0-or-later
//! Private vertex proposals use the same transformed mapping and worker as creation.
use super::*;
#[cfg(test)]
#[path = "editor_tests.rs"]
mod tests;
struct Gesture {
    id: u64,
    revision: u64,
    vertex: usize,
    points: Vec<[f64; 2]>,
    valid: bool,
    origin: Pos2,
    anchor: Pos2,
    moved: bool,
}
struct Numeric {
    id: u64,
    revision: u64,
    vertex: usize,
    coordinates: [f64; 2],
    dirty: bool,
}
#[derive(Default)]
pub(super) struct Editor {
    gesture: Option<Gesture>,
    numeric: Option<Numeric>,
    apply: bool,
    pub error: Option<String>,
}
impl Editor {
    pub fn unfinished(&self) -> bool {
        self.gesture.is_some() || self.numeric.as_ref().is_some_and(|n| n.dirty)
    }
    pub fn points(&self) -> Option<(u64, &[[f64; 2]])> {
        self.gesture.as_ref().map(|g| (g.id, g.points.as_slice()))
    }
    pub fn selected_vertex(&self) -> Option<(u64, usize)> {
        self.numeric.as_ref().map(|n| (n.id, n.vertex))
    }
}
impl Annotations {
    fn pick_handle(
        &self,
        rect: Rect,
        image: &Frame,
        view: &ViewState,
        pos: Pos2,
    ) -> Option<(u64, usize)> {
        self.visible_marks()
            .flat_map(|mark| {
                mark.points
                    .iter()
                    .enumerate()
                    .filter_map(move |(vertex, point)| {
                        let distance = screen_at(rect, image, view, *point)?.distance_sq(pos);
                        (distance <= 100.).then_some((distance, mark.id, vertex))
                    })
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, id, vertex)| (id, vertex))
    }
    fn select_vertex(&mut self, id: u64, vertex: usize) {
        if let Some(mark) = self.marks.iter().find(|m| m.id == id)
            && let Some(point) = mark.points.get(vertex)
        {
            self.selected = Some(id);
            self.editor.numeric = Some(Numeric {
                id,
                revision: mark.revision,
                vertex,
                coordinates: point.map(|v| v + 1.),
                dirty: false,
            });
            self.editor.error = None;
        }
    }
    fn propose_edit(
        &mut self,
        id: u64,
        base_revision: u64,
        points: Vec<[f64; 2]>,
        image: &Arc<Frame>,
        ctx: &egui::Context,
    ) -> Result<(), String> {
        let mark = self
            .marks
            .iter()
            .find(|m| {
                m.id == id && m.revision == base_revision && Some(&m.image) == self.image.as_ref()
            })
            .ok_or("The measurement context changed")?;
        if mark.points == points {
            self.editor = Editor::default();
            return Ok(());
        }
        if mark.points.len() != points.len() || points.iter().any(|p| !sampled_point(image, *p)) {
            return Err("Every vertex must remain on a valid image sample".into());
        }
        let worker_points = history::copy_points(&points)?;
        let revision = self.next_revision;
        self.next_revision = self.next_revision.wrapping_add(1);
        self.calculation
            .start(id, revision, image.clone(), worker_points, mark.tool, ctx)?;
        self.candidate = Some(Candidate {
            id,
            revision,
            image: mark.image.clone(),
            points,
            tool: mark.tool,
            result: None,
            ready_repainted: false,
            base_revision: Some(base_revision),
        });
        self.editor = Editor::default();
        Ok(())
    }
    pub(super) fn finish_edit(
        &mut self,
        candidate: Candidate,
        result: Result<stats_worker::Report, String>,
    ) {
        let Some(mark) = self.marks.iter_mut().find(|m| {
            m.id == candidate.id
                && Some(m.revision) == candidate.base_revision
                && m.image == candidate.image
        }) else {
            return;
        };
        match result {
            Ok(report)
                if report.geometry_valid && report.text.len() <= history::MAX_REPORT_BYTES =>
            {
                match self.history.record(mark) {
                    Ok(()) => {
                        mark.points = candidate.points;
                        mark.label = report.text;
                        mark.revision = candidate.revision;
                        self.selected = Some(mark.id);
                        self.editor = Editor::default();
                    }
                    Err(error) => self.editor.error = Some(error),
                }
            }
            Ok(report) if !report.geometry_valid => {
                self.editor.error = Some(format!("{} · Original geometry kept", report.text))
            }
            Ok(_) => {
                self.editor.error =
                    Some("The report exceeds the history allowance. Original geometry kept".into())
            }
            Err(error) => self.editor.error = Some(error),
        }
    }
    pub(super) fn handle_editor(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        image: &Arc<Frame>,
        view: &ViewState,
    ) {
        let owns_keys = ui.is_enabled()
            && !egui::Popup::is_any_open(ui.ctx())
            && (response.has_focus()
                || response.hovered() && !ui.ctx().egui_wants_keyboard_input());
        if owns_keys && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            self.cancel_interaction(ui.ctx());
            return;
        }
        self.poll();
        if !ui.is_enabled() {
            return;
        }
        if crate::viewport::navigation_override(ui, response) {
            self.cancel_interaction(ui.ctx());
            return;
        }
        self.finish_candidate();
        if self.candidate.is_some() || egui::Popup::is_any_open(ui.ctx()) {
            return;
        }
        // Native input can batch an entire short gesture without drag-start flags.
        // Only an enabled, unobscured canvas handle can own this primary press.
        let origin = ui.input(|i| {
            i.events.iter().rev().find_map(|event| {
                if let egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    ..
                } = event
                {
                    Some(*pos)
                } else {
                    None
                }
            })
        });
        if self.editor.gesture.is_none()
            && let Some(origin) = origin.filter(|p| {
                response
                    .interact_rect
                    .intersect(ui.clip_rect())
                    .contains(*p)
            })
            && let Some((id, vertex)) = self.pick_handle(response.rect, image, view, origin)
        {
            self.select_vertex(id, vertex);
            if let Some(mark) = self.selected_mark()
                && let Some(anchor) = screen_at(response.rect, image, view, mark.points[vertex])
            {
                match history::copy_points(&mark.points) {
                    Ok(points) => {
                        self.editor.gesture = Some(Gesture {
                            id,
                            revision: mark.revision,
                            vertex,
                            points,
                            valid: true,
                            origin,
                            anchor,
                            moved: false,
                        })
                    }
                    Err(error) => self.editor.error = Some(error),
                }
            }
            response.request_focus();
        }
        if let Some(gesture) = &mut self.editor.gesture {
            let position = ui.input(|i| i.pointer.interact_pos());
            gesture.moved |= position.is_some_and(|pos| pos.distance_sq(gesture.origin) > 9.);
            if gesture.moved {
                // Preserve where the handle was grabbed instead of snapping to the cursor.
                let pixel = position.and_then(|pos| {
                    pixel_at(
                        response.rect,
                        image,
                        view,
                        gesture.anchor + (pos - gesture.origin),
                    )
                });
                gesture.valid = pixel.is_some_and(|p| sampled_point(image, p));
                if let Some(pixel) = pixel.filter(|p| sampled_point(image, *p)) {
                    gesture.points[gesture.vertex] = pixel;
                }
            }
        }
        if ui.input(|i| i.pointer.button_released(egui::PointerButton::Primary))
            && let Some(gesture) = self.editor.gesture.take()
        {
            if gesture.moved {
                if gesture.valid {
                    if let Err(error) = self.propose_edit(
                        gesture.id,
                        gesture.revision,
                        gesture.points,
                        image,
                        ui.ctx(),
                    ) {
                        self.editor.error = Some(error);
                    }
                } else {
                    self.editor.error = Some("Edit cancelled outside valid image samples".into());
                }
            }
            response.surrender_focus();
        }
        if self.editor.apply {
            self.editor.apply = false;
            let numeric = self.editor.numeric.take();
            if let Some(numeric) = numeric {
                let points = self
                    .marks
                    .iter()
                    .find(|m| m.id == numeric.id && m.revision == numeric.revision)
                    .ok_or_else(|| "The selected measurement changed".to_owned())
                    .and_then(|m| history::copy_points(&m.points));
                let result = points.and_then(|mut points| {
                    let point = points
                        .get_mut(numeric.vertex)
                        .ok_or("The selected vertex changed")?;
                    *point = numeric.coordinates.map(|v| v - 1.);
                    self.propose_edit(numeric.id, numeric.revision, points, image, ui.ctx())
                });
                if let Err(error) = result {
                    self.editor.error = Some(error);
                }
            }
        }
    }
    pub(super) fn editor_controls(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    self.history.can_undo(),
                    egui::Button::new("Undo geometry edit"),
                )
                .clicked()
            {
                self.undo_edit(false);
            }
            if ui
                .add_enabled(
                    self.history.can_redo(),
                    egui::Button::new("Redo geometry edit"),
                )
                .clicked()
            {
                self.undo_edit(true);
            }
        });
        ui.weak("16 edits in memory · history clears on delete/clear.");
        if self.active_tool != Some(Tool::EditMarks) {
            return;
        }
        let Some(index) = self
            .marks
            .iter()
            .position(|m| Some(m.id) == self.selected && Some(&m.image) == self.image.as_ref())
        else {
            return;
        };
        let mark = &self.marks[index];
        if self
            .editor
            .numeric
            .as_ref()
            .is_none_or(|n| n.id != mark.id || n.revision != mark.revision)
        {
            self.select_vertex(mark.id, 0);
        }
        let Some(numeric) = &mut self.editor.numeric else {
            return;
        };
        let mut cancel_numeric = false;
        ui.add_enabled_ui(
            self.candidate.is_none() && self.editor.gesture.is_none(),
            |ui| {
                let mut vertex = numeric.vertex;
                egui::ComboBox::from_id_salt("measurement-vertex")
                    .selected_text(format!("Vertex {}", vertex + 1))
                    .show_ui(ui, |ui| {
                        for index in 0..self.marks[index].points.len() {
                            ui.selectable_value(
                                &mut vertex,
                                index,
                                format!("Vertex {}", index + 1),
                            );
                        }
                    });
                if numeric.vertex != vertex {
                    numeric.vertex = vertex;
                    numeric.coordinates = self.marks[index].points[vertex].map(|v| v + 1.);
                    numeric.dirty = false;
                }
                crate::toolbar::row(ui, |ui| {
                    for (axis, name) in ["Column", "Row"].into_iter().enumerate() {
                        numeric.dirty |= crate::toolbar::number(
                            ui,
                            name,
                            egui::DragValue::new(&mut numeric.coordinates[axis])
                                .speed(0.1)
                                .max_decimals(4),
                        )
                        .changed();
                    }
                });
                ui.weak("1-based source coordinates. Apply validates before changing the mark.");
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .add_enabled(numeric.dirty, egui::Button::new("Apply vertex coordinates"))
                        .clicked()
                    {
                        self.editor.apply = true;
                        ui.close();
                    }
                    if ui.button("Cancel vertex change").clicked() {
                        cancel_numeric = true;
                        ui.close();
                    }
                });
            },
        );
        if cancel_numeric {
            self.editor.numeric = None;
            self.editor.error = None;
        }
        if let Some(error) = &self.editor.error {
            ui.label(error);
        }
    }
}
