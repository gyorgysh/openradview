// SPDX-License-Identifier: GPL-3.0-or-later
use crate::{theme, viewport::Tool};
use eframe::egui::{self, Pos2, Rect};
use orv_core::{
    ViewState, measurements,
    study::{Frame, Geometry},
    view_mapping::ImageMapping,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
mod creation;
mod drawing;
mod outlines;
pub(crate) use outlines::{Outline, Outlines};
mod editor;
mod history;
mod serialization;
mod stats_worker;
mod storage;
pub(crate) use serialization::deserialize_marks;

#[derive(Clone, Debug, PartialEq)]
struct ImageKey {
    slab: Option<orv_core::slab::Slab>,
    size: [u32; 2],
    geometry: Option<Geometry>,
    frame: Option<u32>,
}
pub(crate) const MAX_MARKS: usize = 128;
pub(crate) const MAX_POINTS: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SavedTool {
    Distance,
    Angle,
    Roi,
    EllipseRoi,
    PolygonRoi,
}
impl SavedTool {
    fn from_tool(tool: Tool) -> Result<Self, &'static str> {
        match tool {
            Tool::Distance => Ok(Self::Distance),
            Tool::Angle => Ok(Self::Angle),
            Tool::Roi => Ok(Self::Roi),
            Tool::EllipseRoi => Ok(Self::EllipseRoi),
            Tool::PolygonRoi => Ok(Self::PolygonRoi),
            _ => Err("Unsupported measurement"),
        }
    }
    fn tool(self) -> Tool {
        match self {
            Self::Distance => Tool::Distance,
            Self::Angle => Tool::Angle,
            Self::Roi => Tool::Roi,
            Self::EllipseRoi => Tool::EllipseRoi,
            Self::PolygonRoi => Tool::PolygonRoi,
        }
    }
    pub(crate) fn extended(self) -> bool {
        matches!(self, Self::EllipseRoi | Self::PolygonRoi)
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SavedMark {
    pub frame: u32,
    pub tool: SavedTool,
    #[serde(deserialize_with = "serialization::deserialize_points")]
    pub points: Vec<[f64; 2]>,
}
impl SavedMark {
    pub(crate) fn validate(
        &self,
        image: &Frame,
        cancelled: &impl Fn() -> bool,
    ) -> Result<(), &'static str> {
        validate_points(image, self.tool.tool(), &self.points, cancelled)
    }
}
fn validate_points(
    image: &Frame,
    tool: Tool,
    points: &[[f64; 2]],
    cancelled: &impl Fn() -> bool,
) -> Result<(), &'static str> {
    if cancelled() {
        return Err("Measurement operation cancelled.");
    }
    let count = match tool {
        Tool::Angle => points.len() == 3,
        Tool::PolygonRoi => (3..=measurements::MAX_POLYGON_VERTICES).contains(&points.len()),
        _ => points.len() == 2,
    };
    if !count {
        return Err("Invalid measurement points.");
    }
    if image.slab.is_some() || points.iter().any(|p| !sampled_point(image, *p)) {
        return Err("Measurement points are outside the verified image.");
    }
    if tool.roi() {
        roi_shape(tool, points)?.validate([image.width, image.height], cancelled)?;
    }
    Ok(())
}
pub(crate) fn validate_mark_counts(marks: &[SavedMark]) -> Result<(), &'static str> {
    if marks.len() > MAX_MARKS
        || marks
            .iter()
            .try_fold(0usize, |total, mark| {
                total
                    .checked_add(mark.points.len())
                    .filter(|n| *n <= MAX_POINTS)
            })
            .is_none()
    {
        return Err("The comparison exceeds the measurement mark or point allowance.");
    }
    Ok(())
}
pub(crate) struct CapturedMark {
    pub saved: SavedMark,
    image: ImageKey,
}
impl CapturedMark {
    pub fn matches(&self, frame: &Frame) -> bool {
        self.image.slab.is_none()
            && self.image.size == [frame.width, frame.height]
            && self.image.geometry == frame.geometry
            && self.image.frame == Some(self.saved.frame)
    }
}

struct Mark {
    id: u64,
    revision: u64,
    image: ImageKey,
    points: Vec<[f64; 2]>,
    tool: Tool,
    label: String,
}
struct Candidate {
    id: u64,
    revision: u64,
    image: ImageKey,
    points: Vec<[f64; 2]>,
    tool: Tool,
    result: Option<Result<stats_worker::Report, String>>,
    ready_repainted: bool,
    base_revision: Option<u64>,
}
#[derive(Default)]
pub struct Annotations {
    calculation: stats_worker::Calculations,
    image: Option<ImageKey>,
    active_tool: Option<Tool>,
    pending: Vec<[f64; 2]>,
    candidate: Option<Candidate>,
    draft_error: Option<String>,
    finish_requested: bool,
    marks: Vec<Mark>,
    selected: Option<u64>,
    next_id: u64,
    next_revision: u64,
    preserve_plane: bool,
    memory: Option<Arc<orv_core::resources::Reservation>>,
    canvas_id: Option<egui::Id>,
    editor: editor::Editor,
    history: history::History,
}
pub fn mapping(rect: Rect, image: &Frame, view: &ViewState) -> Option<ImageMapping> {
    ImageMapping::new(
        [image.width, image.height],
        image.aspect_ratio() as f64,
        rect.aspect_ratio() as f64,
        view,
    )
}
pub fn pixel_at(rect: Rect, image: &Frame, view: &ViewState, pos: Pos2) -> Option<[f64; 2]> {
    let uv = (pos - rect.min) / rect.size();
    mapping(rect, image, view)?.view_to_pixel([uv.x as f64, uv.y as f64])
}
pub fn screen_at(rect: Rect, image: &Frame, view: &ViewState, pixel: [f64; 2]) -> Option<Pos2> {
    let uv = mapping(rect, image, view)?.pixel_to_view(pixel)?;
    Some(rect.min + egui::vec2(uv[0] as f32 * rect.width(), uv[1] as f32 * rect.height()))
}
fn sampled_point(image: &Frame, point: [f64; 2]) -> bool {
    image.width > 0
        && image.height > 0
        && point.iter().all(|v| v.is_finite())
        && point[0] >= 0.
        && point[1] >= 0.
        && point[0] <= f64::from(image.width - 1)
        && point[1] <= f64::from(image.height - 1)
        && (point[1].round() as usize)
            .checked_mul(image.width as usize)
            .and_then(|index| index.checked_add(point[0].round() as usize))
            .is_some_and(|index| image.has_sample(index))
}

impl Annotations {
    pub(crate) fn capture(&self) -> Result<Vec<CapturedMark>, &'static str> {
        if !self.pending.is_empty() || self.candidate.is_some() || self.editor.unfinished() {
            return Err("Finish or cancel the current measurement before saving.");
        }
        self.marks
            .iter()
            .map(|mark| {
                let tool = SavedTool::from_tool(mark.tool)?;
                Ok(CapturedMark {
                    saved: SavedMark {
                        frame: mark
                            .image
                            .frame
                            .ok_or("Only source images can be saved in a comparison.")?,
                        tool,
                        points: mark.points.clone(),
                    },
                    image: mark.image.clone(),
                })
            })
            .collect()
    }
    pub(crate) fn has_marks(&self) -> bool {
        !self.marks.is_empty() || !self.pending.is_empty() || self.candidate.is_some()
    }
    pub(crate) fn restore(
        saved: &[SavedMark],
        series: &orv_core::study::Series,
        cancelled: impl Fn() -> bool,
    ) -> Result<Self, &'static str> {
        validate_mark_counts(saved)?;
        let mut annotations = Self::default();
        if !saved.is_empty() {
            annotations
                .ensure_admitted()
                .map_err(|_| "Not enough memory for restored measurements.")?;
            annotations
                .marks
                .try_reserve_exact(saved.len())
                .map_err(|_| "Not enough memory for restored measurements.")?;
        }
        for mark in saved {
            if cancelled() {
                return Err("Comparison operation cancelled.");
            }
            let tool = mark.tool.tool();
            let image = series
                .read_fresh_frame(mark.frame as usize, &cancelled)
                .map_err(|_| "A measurement image is unavailable or changed.")?;
            mark.validate(&image, &cancelled)?;
            let label = report(&image, &mark.points, tool, &cancelled)
                .unwrap_or_else(|error| format!("Measurement unavailable: {error}"));
            if cancelled() {
                return Err("Comparison operation cancelled.");
            }
            let view = ViewState {
                slice: mark.frame,
                ..Default::default()
            };
            let id = annotations.next_id;
            annotations.next_id += 1;
            annotations.marks.push(Mark {
                id,
                revision: id,
                image: annotations.key(&image, &view),
                points: mark.points.clone(),
                tool,
                label,
            });
        }
        annotations.next_revision = annotations.next_id;
        Ok(annotations)
    }
    pub fn for_reformats() -> Self {
        Self {
            preserve_plane: true,
            ..Default::default()
        }
    }
    pub fn has_content(&self, tool: Tool) -> bool {
        tool.measures()
            || self.visible_marks().next().is_some()
            || !self.pending.is_empty()
            || self.candidate.is_some()
    }
    pub fn status_overlay(&mut self, ui: &mut egui::Ui, rect: Rect, tool: Tool) {
        self.poll();
        if let Some(candidate) = self.candidate.as_mut()
            && candidate.result.is_some()
            && !candidate.ready_repainted
        {
            candidate.ready_repainted = true;
            ui.ctx().request_repaint();
        }
        if !self.has_content(tool) {
            return;
        }
        // Paint without allocating layout space: changing tools or wrapping an
        // ROI result must not move the image under the next click.
        let mut overlay = ui.new_child(
            egui::UiBuilder::new()
                .id_salt("measurement-status")
                .max_rect(Rect::from_min_max(
                    rect.min + egui::vec2(8., 28.),
                    rect.max - egui::vec2(8., 8.),
                )),
        );
        overlay.set_clip_rect(ui.clip_rect().intersect(rect));
        egui::Frame::new()
            .fill(theme::INK)
            .inner_margin(4.)
            .corner_radius(3.)
            .show(&mut overlay, |ui| {
                // The image canvas owns clicks and drags beneath this text.
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(self.status(tool))
                            .small()
                            .color(theme::PAPER),
                    )
                    .selectable(false),
                );
            });
    }
    pub fn clear(&mut self) {
        self.calculation.release();
        self.candidate = None;
        self.draft_error = None;
        self.finish_requested = false;
        self.pending = Vec::new();
        self.marks = Vec::new();
        self.selected = None;
        self.memory = None;
        self.editor = editor::Editor::default();
        self.history = history::History::default();
    }
    pub fn status(&self, tool: Tool) -> String {
        if let Some(error) = &self.editor.error {
            return format!("{error} · Escape dismisses.");
        }
        if self.editor.unfinished() {
            return "Vertex change · release the handle or Apply in Measurements… · Escape cancels.".into();
        }
        if self.candidate.is_some() {
            return "Checking region locally… Escape cancels.".into();
        }
        if let Some(error) = &self.draft_error {
            return format!("{error} · Backspace removes the last point; Escape cancels.");
        }
        if !self.pending.is_empty() {
            return if tool == Tool::PolygonRoi {
                format!(
                    "{} vertices · Enter or Measurements → Finish polygon · Backspace removes a point",
                    self.pending.len()
                )
            } else {
                format!("{} point(s) placed · click to finish", self.pending.len())
            };
        }
        if let Some(mark) = self.selected_mark() {
            return format!("#{} · {}", mark.id.saturating_add(1), mark.label);
        }
        if self.marks.len() == MAX_MARKS {
            return format!("{MAX_MARKS} measurements in memory. Delete or clear to add more.");
        }
        if tool.roi() && self.calculation.is_busy() {
            return "Region statistics are still being calculated. Other measurement tools remain available.".into();
        }
        match tool {
            Tool::Distance => "Distance: click two points. Requires valid spacing.",
            Tool::Angle => "Angle: click endpoint, vertex, endpoint.",
            Tool::Roi => "ROI: click two corners. Scalar values or RGB codes follow the selected representation.",
            Tool::EllipseRoi => "Ellipse ROI: click opposite corners of its bounds.",
            Tool::PolygonRoi => "Polygon ROI: click vertices. Enter or Measurements → Finish polygon completes it. Backspace removes a point; Escape cancels.",
            Tool::EditMarks => "Edit measurements: select or drag a vertex. Measurements… offers numeric coordinates and undo/redo. Escape cancels a change.",
            _ => "Research measurements only",
        }
        .into()
    }
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        response: &egui::Response,
        image: &Arc<Frame>,
        view: &ViewState,
        tool: Tool,
    ) {
        self.canvas_id = Some(response.id);
        self.bind_image(image, view);
        if self.active_tool != Some(tool) {
            self.cancel_draft();
            self.active_tool = Some(tool);
        }
        if image.slab.is_some() {
            self.cancel_interaction(ui.ctx());
            return;
        }
        if tool == Tool::EditMarks {
            self.handle_editor(ui, response, image, view);
        } else {
            self.handle_creation(ui, response, image, view, tool);
        }
        for mark in self.visible_marks() {
            let candidate = self
                .candidate
                .as_ref()
                .filter(|c| c.base_revision.is_some() && c.id == mark.id);
            let gesture = self.editor.points().filter(|(id, _)| *id == mark.id);
            if candidate.is_none() {
                let points = gesture.map_or(mark.points.as_slice(), |(_, points)| points);
                paint(ui, response.rect, image, view, points, mark.tool);
                if tool == Tool::EditMarks
                    && let Some((_, vertex)) = self
                        .editor
                        .selected_vertex()
                        .filter(|(id, _)| *id == mark.id)
                    && let Some(point) = points.get(vertex)
                    && let Some(position) = screen_at(response.rect, image, view, *point)
                {
                    for stroke in [
                        egui::Stroke::new(3., theme::INK),
                        egui::Stroke::new(1., theme::PAPER),
                    ] {
                        ui.painter_at(response.rect).rect_stroke(
                            Rect::from_center_size(position, egui::vec2(14., 14.)),
                            0.,
                            stroke,
                            egui::StrokeKind::Middle,
                        );
                    }
                }
            }
            if let Some(point) = mark.points.first() {
                drawing::number(
                    &ui.painter_at(response.rect),
                    response.rect,
                    image,
                    view,
                    *point,
                    mark.id.saturating_add(1),
                    self.selected == Some(mark.id),
                );
            }
        }
        if let Some(candidate) = &self.candidate {
            paint(
                ui,
                response.rect,
                image,
                view,
                &candidate.points,
                candidate.tool,
            );
        }
        let mut draft = self.pending.clone();
        if !draft.is_empty()
            && let Some(pos) = response.hover_pos()
            && let Some(pixel) = pixel_at(response.rect, image, view, pos)
        {
            draft.push(pixel);
        }
        if !draft.is_empty() {
            paint(ui, response.rect, image, view, &draft, tool);
        }
    }
}
impl Annotations {
    fn key(&self, image: &Frame, view: &ViewState) -> ImageKey {
        ImageKey {
            slab: image.slab,
            size: [image.width, image.height],
            geometry: image.geometry.clone(),
            frame: (!self.preserve_plane).then_some(view.slice),
        }
    }
    pub fn bind_image(&mut self, image: &Frame, view: &ViewState) {
        let key = self.key(image, view);
        if self.image.as_ref() != Some(&key) {
            self.cancel_draft();
            self.image = Some(key);
            self.selected = self.visible_marks().last().map(|mark| mark.id);
        }
    }
    fn visible_marks(&self) -> impl Iterator<Item = &Mark> {
        self.marks
            .iter()
            .filter(|mark| Some(&mark.image) == self.image.as_ref())
    }
    fn selected_mark(&self) -> Option<&Mark> {
        self.visible_marks()
            .find(|mark| Some(mark.id) == self.selected)
    }
    fn delete_selected(&mut self) {
        if let Some(id) = self.selected.take() {
            self.cancel_draft();
            self.history = history::History::default();
            self.marks.retain(|mark| mark.id != id);
            self.calculation.cancel_for(id);
            self.selected = self.visible_marks().last().map(|mark| mark.id);
        }
    }
    pub fn controls(&mut self, ui: &mut egui::Ui) {
        self.poll();
        egui::containers::menu::MenuButton::new("Measurements…")
            .config(
                egui::containers::menu::MenuConfig::new()
                    .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside),
            )
            .ui(ui, |ui| {
                ui.set_max_width(320.);
                egui::ScrollArea::vertical()
                    .id_salt("measurements-menu")
                    .max_height(ui.available_height().min(460.))
                    .show(ui, |ui| {
                        ui.weak(format!(
                            "Measurements · {} / {MAX_MARKS} in memory",
                            self.marks.len()
                        ));
                        ui.weak(
                            "Only this image / plane is listed. Nothing is saved automatically.",
                        );
                        ui.horizontal_wrapped(|ui| {
                            if ui
                                .add_enabled(
                                    self.active_tool == Some(Tool::PolygonRoi)
                                        && self.pending.len() >= 3
                                        && self.candidate.is_none()
                                        && !self.calculation.is_busy(),
                                    egui::Button::new("Finish polygon"),
                                )
                                .clicked()
                            {
                                self.finish_requested = true;
                                ui.close();
                            }
                            if ui
                                .add_enabled(
                                    !self.pending.is_empty() || self.candidate.is_some(),
                                    egui::Button::new("Cancel current measurement"),
                                )
                                .clicked()
                            {
                                self.cancel_draft();
                                ui.close();
                            }
                        });
                        let mut selected = self.selected;
                        for mark in self.visible_marks() {
                            ui.selectable_value(
                                &mut selected,
                                Some(mark.id),
                                format!("#{} · {}", mark.id.saturating_add(1), mark.label),
                            );
                        }
                        if self.selected != selected {
                            self.cancel_draft();
                            self.selected = selected;
                            ui.ctx().request_repaint();
                        }
                        self.editor_controls(ui);
                        if ui
                            .add_enabled(
                                self.selected_mark().is_some(),
                                egui::Button::new("Delete selected measurement"),
                            )
                            .clicked()
                        {
                            self.delete_selected();
                            ui.close();
                        }
                        if ui
                            .add_enabled(
                                self.has_marks(),
                                egui::Button::new("Clear all measurements"),
                            )
                            .clicked()
                        {
                            self.clear();
                            ui.close();
                        }
                    });
            });
    }
    fn poll(&mut self) {
        let Some((id, revision, result)) = self.calculation.poll() else {
            return;
        };
        if let Some(candidate) = self
            .candidate
            .as_mut()
            .filter(|c| c.id == id && c.revision == revision)
        {
            candidate.result = Some(result);
        } else if let Some(mark) = self
            .marks
            .iter_mut()
            .find(|mark| mark.id == id && mark.revision == revision)
        {
            mark.label = result
                .map(|report| report.text)
                .unwrap_or_else(|error| format!("Measurement unavailable: {error}"));
        }
    }
}

fn roi_shape(tool: Tool, points: &[[f64; 2]]) -> Result<measurements::RoiShape<'_>, &'static str> {
    match tool {
        Tool::Roi | Tool::EllipseRoi if points.len() == 2 => {
            let corners = [points[0], points[1]];
            Ok(if tool == Tool::Roi {
                measurements::RoiShape::Rectangle(corners)
            } else {
                measurements::RoiShape::Ellipse(corners)
            })
        }
        Tool::PolygonRoi => Ok(measurements::RoiShape::Polygon(points)),
        _ => Err("The region needs its complete source-coordinate points"),
    }
}
fn report(
    image: &Frame,
    points: &[[f64; 2]],
    tool: Tool,
    cancelled: impl Fn() -> bool,
) -> Result<String, String> {
    if tool.roi() {
        let region = roi_shape(tool, points).map_err(str::to_owned)?;
        let name = match tool {
            Tool::EllipseRoi => "Ellipse ROI",
            Tool::PolygonRoi => "Polygon ROI",
            _ => "ROI",
        };
        let shape_area = |area: Option<f64>| {
            area.map(|v| format!(" · {v:.2} mm² shape"))
                .unwrap_or_default()
        };
        if image.rgba.is_some() {
            let region = measurements::color_roi_shape_stats_cancellable(image, region, &cancelled)
                .map_err(str::to_owned)?;
            let stats = region.samples;
            let domain = match stats.domain {
                measurements::ColorRoiDomain::BeforeIcc => "RGB codes before ICC/display",
                measurements::ColorRoiDomain::SrgbPresentation => "sRGB presentation codes",
            };
            let area = stats
                .area_mm2
                .map(|v| format!(" · {v:.2} mm² sampled"))
                .unwrap_or_default();
            let channels = ["R", "G", "B"]
                .into_iter()
                .zip(&stats.channels)
                .map(|(name, s)| {
                    format!(
                        "{name} · mean {:.2} · SD {:.2} · min {:.0} · max {:.0}",
                        s.mean, s.standard_deviation, s.min, s.max
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            return Ok(format!(
                "{name} · {domain} · {}-bit · n={}{}{}\n{channels}",
                stats.bits,
                stats.count,
                area,
                shape_area(region.geometric_area_mm2)
            ));
        }
        let region = measurements::roi_shape_stats_cancellable(image, region, &cancelled)
            .map_err(str::to_owned)?;
        let stats = region.samples;
        let area = stats
            .area_mm2
            .map(|v| format!(" · {v:.2} mm² sampled"))
            .unwrap_or_default();
        return Ok(format!(
            "{name} · n={} · mean {:.2} · SD {:.2}\nmin {:.2} · max {:.2}{}{}",
            stats.count,
            stats.mean,
            stats.standard_deviation,
            stats.min,
            stats.max,
            area,
            shape_area(region.geometric_area_mm2)
        ));
    }
    let g = image
        .geometry
        .as_ref()
        .ok_or("No image geometry is available")?;
    let points = points
        .iter()
        .map(|p| g.pixel_to_world(*p).map_err(|e| e.to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    match tool {
        Tool::Distance => measurements::distance_mm(points[0], points[1])
            .map(|v| format!("Distance · {v:.2} mm"))
            .map_err(str::to_owned),
        Tool::Angle => measurements::angle_degrees(points[0], points[1], points[2])
            .map(|v| format!("Angle · {v:.2}°"))
            .map_err(str::to_owned),
        _ => Err("Choose a measurement tool".into()),
    }
}
fn paint(
    ui: &egui::Ui,
    rect: Rect,
    image: &Frame,
    view: &ViewState,
    points: &[[f64; 2]],
    tool: Tool,
) {
    drawing::geometry(&ui.painter_at(rect), rect, image, view, points, tool, true);
}

#[cfg(test)]
mod tests {
    use super::*;
    fn draw(
        ctx: &egui::Context,
        annotations: &mut Annotations,
        image: &Arc<Frame>,
        view: &ViewState,
        events: Vec<egui::Event>,
    ) {
        let mut output = ctx.run_ui(
            egui::RawInput {
                events,
                ..Default::default()
            },
            |ui| {
                let (_, response) =
                    ui.allocate_exact_size(egui::vec2(400., 400.), egui::Sense::click_and_drag());
                annotations.show(ui, &response, image, view, Tool::Distance);
            },
        );
        output.textures_delta.clear();
    }
    fn click(
        ctx: &egui::Context,
        annotations: &mut Annotations,
        image: &Arc<Frame>,
        view: &ViewState,
        pos: Pos2,
    ) {
        draw(
            ctx,
            annotations,
            image,
            view,
            vec![egui::Event::PointerMoved(pos)],
        );
        for pressed in [true, false] {
            draw(
                ctx,
                annotations,
                image,
                view,
                vec![egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                }],
            );
        }
    }
    #[test]
    fn marks_survive_navigation_and_redecoding_without_retaining_source_pixels() {
        let ctx = egui::Context::default();
        let image = Arc::new((*orv_core::study::demo_study().series[0].frames[0]).clone());
        let mut annotations = Annotations::default();
        let mut view = ViewState::default();
        for x in [100., 200., 120., 220.] {
            click(&ctx, &mut annotations, &image, &view, Pos2::new(x, 160.));
        }
        assert_eq!(annotations.marks.len(), 2);
        assert_eq!(
            Arc::strong_count(&image),
            1,
            "Completed marks retain no images"
        );
        view.slice = 1;
        annotations.bind_image(&image, &view);
        assert_eq!(
            annotations.visible_marks().count(),
            0,
            "Frame ordinal matters even with identical geometry"
        );
        view.slice = 0;
        view.transform.quarter_turns = 1;
        let decoded_again = Arc::new((*image).clone());
        annotations.bind_image(&decoded_again, &view);
        assert_eq!(annotations.visible_marks().count(), 2);
        assert_eq!(annotations.selected, Some(1));
        annotations.delete_selected();
        assert_eq!(annotations.visible_marks().count(), 1);
        assert_eq!(annotations.selected, Some(0));
        annotations.clear();
        annotations.bind_image(&image, &view);
        assert_eq!(annotations.visible_marks().count(), 0);
    }
    #[test]
    fn physical_plane_keys_reject_changed_geometry_and_mark_collection_is_bounded() {
        let ctx = egui::Context::default();
        let image = orv_core::study::demo_study().series[0].frames[0].clone();
        let mut annotations = Annotations::for_reformats();
        let view = ViewState::default();
        for _ in 0..MAX_MARKS + 1 {
            click(&ctx, &mut annotations, &image, &view, Pos2::new(100., 160.));
            click(&ctx, &mut annotations, &image, &view, Pos2::new(200., 160.));
        }
        assert_eq!(annotations.marks.len(), MAX_MARKS);
        let mut changed = (*image).clone();
        changed.geometry.as_mut().unwrap().position[2] += 1.;
        annotations.bind_image(&changed, &view);
        assert_eq!(annotations.visible_marks().count(), 0);
        annotations.bind_image(&image, &view);
        assert_eq!(annotations.visible_marks().count(), MAX_MARKS);
    }
    #[test]
    fn measurement_text_does_not_capture_canvas_clicks_or_hide_the_pointer() {
        let ctx = egui::Context::default();
        let image = orv_core::study::demo_study().series[0].frames[0].clone();
        let mut annotations = Annotations::default();
        let mut time = 0.;
        let mut canvas_hovered = false;
        let mut draw = |events| {
            time += 0.1;
            let mut output = ctx.run_ui(
                egui::RawInput {
                    events,
                    time: Some(time),
                    ..Default::default()
                },
                |ui| {
                    let (_, response) = ui
                        .allocate_exact_size(egui::vec2(400., 400.), egui::Sense::click_and_drag());
                    canvas_hovered = response.hovered();
                    annotations.show(ui, &response, &image, &ViewState::default(), Tool::Distance);
                    annotations.status_overlay(ui, response.rect, Tool::Distance);
                },
            );
            output.textures_delta.clear();
            output
        };
        let mut output = draw(vec![]);
        for _ in 0..3 {
            output = draw(vec![]);
        }
        let pos = crate::controls::tests::text_rect(
            &output,
            "Distance: click two points. Requires valid spacing.",
        )
        .center();
        let button = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        let _ = draw(vec![egui::Event::PointerMoved(pos)]);
        let _ = draw(vec![button(true)]);
        let _ = draw(vec![button(false)]);
        let _ = draw(vec![]);
        assert!(
            canvas_hovered,
            "The overlay stole the canvas hover/scroll target"
        );
        assert_eq!(
            annotations.pending.len(),
            1,
            "The overlay stole the measurement click"
        );
    }

    #[test]
    fn measurement_overlay_respects_the_parent_clip() {
        let ctx = egui::Context::default();
        let mut annotations = Annotations::default();
        let clip = Rect::from_min_size(Pos2::ZERO, egui::vec2(240., 50.));
        for _ in 0..3 {
            let mut output = ctx.run_ui(Default::default(), |ui| {
                ui.set_clip_rect(clip);
                annotations.status_overlay(
                    ui,
                    Rect::from_min_size(Pos2::ZERO, egui::vec2(300., 300.)),
                    Tool::Roi,
                );
            });
            output.textures_delta.clear();
            let actual_clip = output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::epaint::Shape::Text(t) if t.galley.job.text.starts_with("ROI:") => {
                        Some(shape.clip_rect)
                    }
                    _ => None,
                })
                .expect("The ROI instruction must still be painted");
            assert!(
                clip.contains_rect(actual_clip),
                "Overlay expands its parent's clip: {actual_clip:?}"
            );
        }
    }
    #[test]
    fn measurement_hints_and_wrapped_results_do_not_shift_the_image_or_next_control() {
        let ctx = egui::Context::default();
        let mut annotations = Annotations::default();
        for width in [300., 800.] {
            let mut previous = None;
            for tool in [
                Tool::Pan,
                Tool::Distance,
                Tool::Angle,
                Tool::Roi,
                Tool::Window,
            ] {
                let mut bounds = None;
                let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(width, 200.), egui::Sense::click());
                    let cursor = ui.cursor();
                    annotations.status_overlay(ui, rect, tool);
                    assert_eq!(ui.cursor(), cursor);
                    bounds = Some((rect, ui.button("Next control").rect));
                });
                output.textures_delta.clear();
                if let Some(previous) = previous {
                    assert_eq!(bounds.unwrap(), previous);
                }
                previous = bounds;
            }
        }
    }
    #[test]
    fn annotation_reports_are_physical_and_do_not_measure_letterboxing() {
        let study = orv_core::study::demo_study();
        let image = &study.series[0].frames[0];
        assert_eq!(
            report(image, &[[0., 0.], [3., 4.]], Tool::Distance, || false).unwrap(),
            "Distance · 5.00 mm"
        );
        let rect = Rect::from_min_size(Pos2::ZERO, egui::vec2(800., 400.));
        assert!(pixel_at(rect, image, &ViewState::default(), Pos2::new(10., 200.)).is_none());
        let pixel = [100.25, 78.5];
        let screen = screen_at(rect, image, &ViewState::default(), pixel).unwrap();
        let recovered = pixel_at(rect, image, &ViewState::default(), screen).unwrap();
        assert!((pixel[0] - recovered[0]).abs() < 1e-4 && (pixel[1] - recovered[1]).abs() < 1e-4);
    }
}
