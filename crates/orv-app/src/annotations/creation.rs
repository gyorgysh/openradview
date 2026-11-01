// SPDX-License-Identifier: GPL-3.0-or-later
//! Drafts become marks only after bounded worker validation of the exact candidate.
use super::*;
#[cfg(test)]
#[path = "creation_tests.rs"]
mod tests;
pub(super) struct PointInput {
    coordinates: [f64; 2],
    pub(super) request: Option<[f64; 2]>,
    focus_after_calculation: Option<egui::Id>,
}
impl Default for PointInput {
    fn default() -> Self {
        Self {
            coordinates: [1.; 2],
            request: None,
            focus_after_calculation: None,
        }
    }
}
impl Annotations {
    pub(crate) fn cancel_interaction(&mut self, ctx: &egui::Context) {
        self.cancel_draft();
        if let Some(id) = self.canvas_id {
            ctx.memory_mut(|memory| memory.surrender_focus(id));
        }
    }
    pub(super) fn cancel_draft(&mut self) {
        self.editor = editor::Editor::default();
        if let Some(candidate) = self.candidate.take() {
            self.calculation.cancel_for(candidate.id);
        }
        self.pending.clear();
        self.point_input.request = None;
        self.point_input.focus_after_calculation = None;
        self.draft_error = None;
        self.finish_requested = false;
    }
    fn reopen_draft(&mut self) {
        if let Some(candidate) = self.candidate.take() {
            self.calculation.cancel_for(candidate.id);
            self.pending = candidate.points;
        }
    }
    pub(super) fn point_count(&self) -> usize {
        self.marks
            .iter()
            .map(|mark| mark.points.len())
            .sum::<usize>()
            + self.pending.len()
            + self.candidate.as_ref().map_or(0, |c| c.points.len())
    }
    fn complete_draft(
        &mut self,
        image: &Arc<Frame>,
        view: &ViewState,
        tool: Tool,
        ctx: &egui::Context,
    ) {
        let valid_count = match tool {
            Tool::Distance | Tool::Roi | Tool::EllipseRoi => self.pending.len() == 2,
            Tool::Angle => self.pending.len() == 3,
            Tool::PolygonRoi => {
                (3..=measurements::MAX_POLYGON_VERTICES).contains(&self.pending.len())
            }
            _ => false,
        };
        if !valid_count {
            self.draft_error = Some("Finish the region's required points first".into());
            return;
        }
        if self.marks.len() >= MAX_MARKS || self.point_count() > MAX_POINTS {
            self.draft_error = Some("The measurement mark or point allowance is full".into());
            return;
        }
        if let Err(error) = self.prepare_storage(tool) {
            self.draft_error = Some(error);
            return;
        }
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        let revision = self.next_revision;
        self.next_revision = self.next_revision.wrapping_add(1);
        let key = self.image.clone().unwrap_or_else(|| self.key(image, view));
        if tool.roi() {
            let mut points = Vec::new();
            if points.try_reserve_exact(self.pending.len()).is_err() {
                self.draft_error = Some("Not enough memory for the region request".into());
                return;
            }
            points.extend_from_slice(&self.pending);
            match self
                .calculation
                .start(id, revision, image.clone(), points, tool, ctx)
            {
                Ok(()) => {
                    self.candidate = Some(Candidate {
                        id,
                        revision,
                        image: key,
                        points: std::mem::take(&mut self.pending),
                        tool,
                        result: None,
                        ready_repainted: false,
                        base_revision: None,
                    })
                }
                Err(error) => self.draft_error = Some(error),
            }
        } else {
            let label = report(image, &self.pending, tool, || false)
                .unwrap_or_else(|error| format!("Measurement unavailable: {error}"));
            self.marks.push(Mark {
                id,
                revision,
                image: key,
                points: std::mem::take(&mut self.pending),
                tool,
                label,
            });
            self.selected = Some(id);
        }
    }
    pub(super) fn finish_candidate(&mut self) {
        let Some(result) = self.candidate.as_mut().and_then(|c| c.result.take()) else {
            return;
        };
        let Some(candidate) = self.candidate.take() else {
            return;
        };
        let expected_tool = if candidate.base_revision.is_some() {
            Tool::EditMarks
        } else {
            candidate.tool
        };
        if self.image.as_ref() != Some(&candidate.image) || self.active_tool != Some(expected_tool)
        {
            return;
        }
        if candidate.base_revision.is_some() {
            self.finish_edit(candidate, result);
            return;
        }
        match result {
            Ok(report)
                if report.geometry_valid
                    && self.marks.len() < MAX_MARKS
                    && self
                        .marks
                        .iter()
                        .map(|mark| mark.points.len())
                        .sum::<usize>()
                        + candidate.points.len()
                        <= MAX_POINTS =>
            {
                self.marks.push(Mark {
                    id: candidate.id,
                    revision: candidate.revision,
                    image: candidate.image,
                    points: candidate.points,
                    tool: candidate.tool,
                    label: report.text,
                });
                self.selected = Some(candidate.id);
                self.draft_error = None;
            }
            result => {
                self.pending = candidate.points;
                self.draft_error = Some(match result {
                    Ok(report) => report.text,
                    Err(error) => error,
                });
            }
        }
    }
    pub(super) fn point_controls(&mut self, ui: &mut egui::Ui) {
        let Some(tool) = self
            .active_tool
            .filter(|tool| tool.measures() && *tool != Tool::EditMarks)
        else {
            return;
        };
        let Some(key) = &self.image else { return };
        if key.slab.is_some() || key.size.contains(&0) {
            return;
        }
        let size = key.size;
        ui.add_enabled_ui(
            self.candidate.is_none() && !self.calculation.is_busy() && self.marks.len() < MAX_MARKS,
            |ui| {
                ui.label("Add a measurement point · image coordinates, numbered from 1");
                crate::toolbar::number(
                    ui,
                    "Point column",
                    egui::DragValue::new(&mut self.point_input.coordinates[0])
                        .range(1.0..=f64::from(size[0]))
                        .max_decimals(3),
                );
                crate::toolbar::number(
                    ui,
                    "Point row",
                    egui::DragValue::new(&mut self.point_input.coordinates[1])
                        .range(1.0..=f64::from(size[1]))
                        .max_decimals(3),
                );
                let add = ui.button("Add point to measurement");
                if add.clicked() {
                    self.point_input.request =
                        Some(self.point_input.coordinates.map(|value| value - 1.));
                    self.point_input.focus_after_calculation = add.has_focus().then_some(add.id);
                } else if let Some(origin) = self.point_input.focus_after_calculation {
                    let focused = ui.memory(|memory| memory.focused());
                    if origin != add.id || focused.is_some_and(|id| id != origin) {
                        self.point_input.focus_after_calculation = None;
                    } else if ui.is_enabled() && self.point_input.request.is_none() {
                        // Worker validation temporarily disables the control. Restore
                        // its keyboard origin only if no other control took focus.
                        if focused.is_none() {
                            add.request_focus();
                        }
                        self.point_input.focus_after_calculation = None;
                    }
                }
                ui.weak(if tool == Tool::PolygonRoi {
                    "Add at least 3 vertices, then Finish polygon."
                } else if tool == Tool::Angle {
                    "Endpoint, vertex, endpoint · 3 points."
                } else {
                    "Add 2 points; the completed result appears below."
                });
                if ui
                    .add_enabled(
                        !self.pending.is_empty(),
                        egui::Button::new("Remove last draft point"),
                    )
                    .clicked()
                {
                    self.pending.pop();
                    self.draft_error = None;
                    if self.pending.is_empty() {
                        add.request_focus();
                    }
                    ui.ctx().request_repaint();
                }
            },
        );
        if let Some(error) = &self.draft_error {
            ui.label(error);
        }
    }
    fn add_point(
        &mut self,
        pixel: [f64; 2],
        image: &Arc<Frame>,
        view: &ViewState,
        tool: Tool,
        ctx: &egui::Context,
    ) {
        ctx.request_repaint();
        if !sampled_point(image, pixel) {
            self.draft_error = Some("The point is outside a valid image sample".into());
            return;
        }
        if let Err(error) = self.prepare_storage(tool) {
            self.draft_error = Some(error);
            return;
        }
        if tool != Tool::PolygonRoi && self.pending.len() >= if tool == Tool::Angle { 3 } else { 2 }
        {
            self.pending.pop();
        }
        if self.point_count() >= MAX_POINTS
            || tool == Tool::PolygonRoi && self.pending.len() >= measurements::MAX_POLYGON_VERTICES
        {
            self.draft_error = Some("The region or collection point allowance is full".into());
        } else {
            self.pending.push(pixel);
            self.draft_error = None;
            if tool != Tool::PolygonRoi
                && self.pending.len() == if tool == Tool::Angle { 3 } else { 2 }
            {
                self.complete_draft(image, view, tool, ctx);
            }
        }
    }
    pub(super) fn handle_creation(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        image: &Arc<Frame>,
        view: &ViewState,
        tool: Tool,
    ) {
        let owns_keys = ui.is_enabled()
            && !egui::Popup::is_any_open(ui.ctx())
            && (response.has_focus()
                || response.hovered() && !ui.ctx().egui_wants_keyboard_input());
        let has_draft =
            !self.pending.is_empty() || self.candidate.is_some() || self.draft_error.is_some();
        if owns_keys && has_draft {
            if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
                self.cancel_draft();
                response.surrender_focus();
            } else if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Backspace)) {
                self.reopen_draft();
                self.pending.pop();
                self.draft_error = None;
                if self.pending.is_empty() {
                    response.surrender_focus();
                }
            } else if tool == Tool::PolygonRoi
                && self.candidate.is_none()
                && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter))
            {
                self.finish_requested = true;
            }
        }
        self.poll();
        if !ui.is_enabled() {
            return;
        }
        self.finish_candidate();
        let typed_point = self.point_input.request.take();
        let pointer_point = (!egui::Popup::is_any_open(ui.ctx())
            && !crate::viewport::navigation_override(ui, response)
            && response.clicked_by(egui::PointerButton::Primary))
        .then(|| {
            response
                .interact_pointer_pos()
                .and_then(|position| pixel_at(response.rect, image, view, position))
        })
        .flatten();
        if tool.measures()
            && tool != Tool::EditMarks
            && self.marks.len() < MAX_MARKS
            && self.candidate.is_none()
            && (!tool.roi() || !self.calculation.is_busy())
            && let Some(pixel) = typed_point.or(pointer_point)
        {
            self.add_point(pixel, image, view, tool, ui.ctx());
            if typed_point.is_none()
                && matches!(tool, Tool::EllipseRoi | Tool::PolygonRoi)
                && !self.pending.is_empty()
            {
                response.request_focus();
            }
        }
        if self.finish_requested && self.candidate.is_none() && !self.calculation.is_busy() {
            self.finish_requested = false;
            self.complete_draft(image, view, tool, ui.ctx());
        }
        if self.pending.is_empty() && self.candidate.is_none() {
            response.surrender_focus();
        }
    }
}
