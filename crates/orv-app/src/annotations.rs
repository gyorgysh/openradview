// SPDX-License-Identifier: GPL-3.0-or-later
use crate::{theme, viewport::Tool};
use eframe::egui::{self, Pos2, Rect};
use orv_core::{
    ViewState, measurements,
    study::{Frame, Geometry},
    view_mapping::ImageMapping,
};
use std::sync::Arc;
mod stats_worker;

#[derive(PartialEq)]
struct ImageKey {
    size: [u32; 2],
    geometry: Option<Geometry>,
    pixels: usize,
}
struct Mark {
    points: Vec<[f64; 2]>,
    tool: Tool,
    label: String,
}
#[derive(Default)]
pub struct Annotations {
    calculation: Option<stats_worker::Calculation>,
    image: Option<ImageKey>,
    active_tool: Option<Tool>,
    pending: Vec<[f64; 2]>,
    complete: Option<Mark>,
    preserve_plane: bool,
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

impl Annotations {
    pub fn for_reformats() -> Self {
        Self {
            preserve_plane: true,
            ..Default::default()
        }
    }
    pub fn has_content(&self, tool: Tool) -> bool {
        tool.measures() || self.complete.is_some() || !self.pending.is_empty()
    }
    pub fn status_overlay(&mut self, ui: &mut egui::Ui, rect: Rect, tool: Tool) {
        self.poll();
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
        self.calculation = None;
        self.pending.clear();
        self.complete = None;
    }
    pub fn status(&self, tool: Tool) -> String {
        if !self.pending.is_empty() {
            return format!("{} point(s) placed · click to finish", self.pending.len());
        }
        if let Some(mark) = &self.complete {
            return mark.label.clone();
        }
        match tool {
            Tool::Distance => "Distance: click two points. Requires valid spacing.",
            Tool::Angle => "Angle: click endpoint, vertex, endpoint.",
            Tool::Roi => "ROI: click two corners. Scalar values or RGB codes follow the selected representation.",
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
        self.poll();
        let key = ImageKey {
            size: [image.width, image.height],
            geometry: image.geometry.clone(),
            pixels: if self.preserve_plane {
                0
            } else {
                Arc::as_ptr(image) as usize
            },
        };
        if self.image.as_ref() != Some(&key) {
            self.clear();
            self.image = Some(key);
        }
        if self.active_tool != Some(tool) {
            self.pending.clear();
            self.active_tool = Some(tool);
        }
        if tool.measures()
            && !crate::viewport::navigation_override(ui, response)
            && response.clicked_by(egui::PointerButton::Primary)
            && let Some(pos) = response.interact_pointer_pos()
            && let Some(pixel) = pixel_at(response.rect, image, view, pos)
        {
            let index =
                pixel[1].round() as usize * image.width as usize + pixel[0].round() as usize;
            if image.has_sample(index) {
                self.pending.push(pixel);
                let needed = if tool == Tool::Angle { 3 } else { 2 };
                if self.pending.len() == needed {
                    let label = if tool == Tool::Roi {
                        self.calculation = Some(stats_worker::Calculation::start(
                            image.clone(),
                            self.pending.clone(),
                            ui.ctx().clone(),
                        ));
                        "Calculating local rectangle statistics…".into()
                    } else {
                        report(image, &self.pending, tool, || false)
                            .unwrap_or_else(|error| format!("Measurement unavailable: {error}"))
                    };
                    self.complete = Some(Mark {
                        points: std::mem::take(&mut self.pending),
                        tool,
                        label,
                    });
                }
            }
        }
        if let Some(mark) = &self.complete {
            paint(ui, response.rect, image, view, &mark.points, mark.tool);
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
    fn poll(&mut self) {
        if let Some(result) =
            self.calculation
                .as_ref()
                .and_then(|task| match task.result.try_recv() {
                    Ok(result) => Some(result),
                    Err(std::sync::mpsc::TryRecvError::Empty) => None,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        Some(Err("The rectangle statistics worker stopped".into()))
                    }
                })
        {
            self.calculation = None;
            if let Some(mark) = &mut self.complete {
                mark.label =
                    result.unwrap_or_else(|error| format!("Measurement unavailable: {error}"));
            }
        }
    }
}

fn report(
    image: &Frame,
    points: &[[f64; 2]],
    tool: Tool,
    cancelled: impl Fn() -> bool,
) -> Result<String, String> {
    if tool == Tool::Roi {
        if image.rgba.is_some() {
            let stats = measurements::color_roi_stats_cancellable(
                image,
                [points[0], points[1]],
                &cancelled,
            )
            .map_err(str::to_owned)?;
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
                "{domain} · {}-bit · n={}{}\n{channels}",
                stats.bits, stats.count, area
            ));
        }
        let stats = measurements::roi_stats_cancellable(image, [points[0], points[1]], &cancelled)
            .map_err(str::to_owned)?;
        let area = stats
            .area_mm2
            .map(|v| format!(" · {v:.2} mm² sampled"))
            .unwrap_or_default();
        return Ok(format!(
            "ROI · n={} · mean {:.2} · SD {:.2}\nmin {:.2} · max {:.2}{}",
            stats.count, stats.mean, stats.standard_deviation, stats.min, stats.max, area
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
    let painter = ui.painter_at(rect);
    let screen = points
        .iter()
        .filter_map(|p| screen_at(rect, image, view, *p))
        .collect::<Vec<_>>();
    let stroke = egui::Stroke::new(1.5, theme::PAPER);
    if tool == Tool::Roi && screen.len() > 1 {
        painter.rect_stroke(
            Rect::from_two_pos(screen[0], screen[1]),
            0.0,
            stroke,
            egui::StrokeKind::Inside,
        );
    } else {
        for pair in screen.windows(2) {
            painter.line_segment([pair[0], pair[1]], stroke);
        }
    }
    for pos in screen {
        painter.circle_filled(pos, 3.0, theme::PAPER);
        painter.circle_stroke(pos, 4.5, egui::Stroke::new(1.0, theme::INK));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
