// SPDX-License-Identifier: GPL-3.0-or-later
//! Temporary local two-series viewing. Agent composition deliberately stays separate.
mod navigation;
#[cfg(test)]
mod tests;
use crate::{
    annotations::Annotations,
    controls,
    frame_worker::FrameWorker,
    rendered::ViewSpec,
    theme,
    viewport::{self, Tool},
};
use eframe::egui;
use orv_core::{
    ViewState,
    navigation::NavigationStack,
    study::{Frame, Series, Study},
};
use std::sync::Arc;

fn activates_pane(ui: &egui::Ui, response: &egui::Response) -> bool {
    // Scalar/color toolbars can differ in height; hovering must not move a view.
    response.clicked()
        || response.dragged()
        || (response.hovered()
            && ui.input(|input| {
                input.is_scrolling() || (input.zoom_delta() - 1.).abs() > f32::EPSILON
            }))
}
struct Pane {
    series: usize,
    source: Arc<Series>,
    view: ViewState,
    worker: FrameWorker,
    image: Option<(usize, Arc<Frame>)>,
    last_view: Option<ViewState>,
    picture_size: Option<[u32; 2]>,
    error: Option<String>,
    geometry_task: Option<navigation::Task>,
    geometry: Option<NavigationStack>,
    geometry_error: Option<String>,
    annotations: Annotations,
    scroll: f32,
    tool: Tool,
}
impl Pane {
    fn new(study: &Study, series: usize, ctx: &egui::Context) -> Option<Self> {
        let source = Arc::new(study.series.get(series)?.clone());
        let index = source.frame_count() / 2;
        let view = ViewState {
            slice: index as u32,
            ..Default::default()
        };
        Some(Self {
            series,
            view,
            worker: FrameWorker::start(source.clone(), index, ctx.clone()),
            geometry_task: Some(navigation::Task::start(source.clone(), ctx.clone())),
            source,
            image: None,
            last_view: None,
            picture_size: None,
            error: None,
            geometry: None,
            geometry_error: None,
            annotations: Annotations::default(),
            scroll: 0.,
            tool: Tool::Pan,
        })
    }
    fn poll(&mut self) {
        if let Some(result) = self.worker.take() {
            match result.image {
                Ok(image) => {
                    if self.image.is_none() {
                        self.view.window = image.default_window;
                    }
                    self.image = Some((result.index, image));
                    self.error = None;
                }
                Err(error) => self.error = Some(error.to_string()),
            }
        }
        if let Some(result) = self.geometry_task.as_ref().and_then(navigation::Task::take) {
            self.geometry_task = None;
            match result {
                Ok(geometry) => {
                    self.geometry = Some(geometry);
                    self.geometry_error = None;
                }
                Err(error) => self.geometry_error = Some(error),
            }
        }
    }
    fn controls(&mut self, ui: &mut egui::Ui) {
        let color = self
            .image
            .as_ref()
            .is_some_and(|(_, image)| image.rgba.is_some());
        if color && self.tool == Tool::Window {
            self.tool = Tool::Pan;
        }
        if let Some((index, image)) = &self.image {
            let mut view = self.view.clone();
            view.slice = *index as u32;
            self.annotations.bind_image(image, &view);
        }
        if controls::tools(ui, &mut self.tool, false, !color, |ui| {
            controls::orientation(ui, &mut self.view.transform);
            self.annotations.controls(ui);
        }) {
            self.view.zoom = 1.;
            self.view.pan = [0.; 2];
        }
        if let Some((_, image)) = &self.image {
            crate::view_context::show(ui, image);
            if color {
                controls::color_display(
                    ui,
                    &mut self.view.invert,
                    image.color.as_ref().map(|color| &color.info),
                );
            } else {
                controls::window(
                    ui,
                    &mut self.view.window,
                    &mut self.view.invert,
                    image.default_window,
                    image.scalar.as_ref().map(|scalar| &**scalar.choices()),
                );
            }
        }
        if ui.is_enabled() && !ui.ctx().egui_wants_keyboard_input() {
            ui.input(|input| {
                if input.key_pressed(egui::Key::ArrowDown) {
                    self.view.slice = self
                        .view
                        .slice
                        .saturating_add(1)
                        .min(self.source.frame_count().saturating_sub(1) as u32);
                }
                if input.key_pressed(egui::Key::ArrowUp) {
                    self.view.slice = self.view.slice.saturating_sub(1);
                }
                for (key, tool) in [
                    (egui::Key::P, Tool::Pan),
                    (egui::Key::Z, Tool::Zoom),
                    (egui::Key::S, Tool::Browse),
                    (egui::Key::W, Tool::Window),
                ] {
                    if input.key_pressed(key) && (!color || tool != Tool::Window) {
                        self.tool = tool;
                    }
                }
                if input.key_pressed(egui::Key::R) {
                    self.view.pan = [0.; 2];
                    self.view.zoom = 1.;
                }
                if input.key_pressed(egui::Key::I) {
                    self.view.invert = !self.view.invert;
                }
            });
        }
    }
    fn canvas(
        &mut self,
        ui: &mut egui::Ui,
        frame: &eframe::Frame,
        pane: usize,
        size: egui::Vec2,
        active: bool,
    ) -> bool {
        ui.add(
            egui::Label::new(format!(
                "{} · {}",
                if pane == 0 { "LEFT" } else { "RIGHT" },
                self.source.label()
            ))
            .truncate(),
        );
        let (rect, response) = ui.allocate_exact_size(
            egui::vec2(size.x, (size.y - 54.).max(40.)),
            egui::Sense::click_and_drag(),
        );
        let interacted = activates_pane(ui, &response);
        viewport::interact(
            ui,
            &response,
            &mut self.view,
            self.tool,
            self.source.frame_count(),
            &mut self.scroll,
        );
        let pending = self
            .image
            .as_ref()
            .is_none_or(|(index, _)| *index != self.view.slice as usize);
        if let Some((index, image)) = &self.image {
            let mut view = self.view.clone();
            view.slice = *index as u32;
            self.last_view = Some(view.clone());
            self.picture_size = Some(crate::rendered::picture_size(
                rect.size() * ui.ctx().pixels_per_point(),
            ));
            viewport::draw(ui, frame, rect, image.clone(), &view, 8 + pane, "");
            crate::image_overlay::draw(ui, &response, image, &view);
            if !pending {
                self.annotations
                    .show(ui, &response, image, &view, self.tool);
                self.annotations.status_overlay(ui, rect, self.tool);
            }
        } else {
            ui.painter().rect_filled(rect, 0., theme::INK);
        }
        if active {
            ui.painter().rect_stroke(
                rect,
                0.,
                egui::Stroke::new(1.5, theme::SMOKE),
                egui::StrokeKind::Inside,
            );
        }
        if pending {
            ui.painter_at(rect).text(
                rect.min + egui::vec2(8., 8.),
                egui::Align2::LEFT_TOP,
                self.error.as_deref().unwrap_or("Loading image locally…"),
                egui::FontId::proportional(12.),
                theme::PAPER,
            );
        }
        let slider = ui.add(
            egui::Slider::new(
                &mut self.view.slice,
                0..=self.source.frame_count().saturating_sub(1) as u32,
            )
            .text("Image")
            .custom_formatter(|value, _| {
                format!("{} / {}", value as u32 + 1, self.source.frame_count())
            }),
        );
        interacted || slider.dragged() || slider.changed()
    }
    fn picture(&self) -> Option<(ViewSpec, Arc<Frame>, [u32; 2])> {
        let (index, image) = self.image.as_ref()?;
        let view = self.last_view.as_ref()?;
        if *index != self.view.slice as usize || view != &self.view {
            return None;
        }
        Some((
            ViewSpec {
                view: view.clone(),
                point: None,
                reformat_side: 512,
                reconstruction: None,
                preferred_size: None,
            },
            image.clone(),
            self.picture_size?,
        ))
    }
}
pub(crate) struct ComparisonSession {
    study: Arc<Study>,
    panes: [Pane; 2],
    active: usize,
    linked: bool,
    notice: Option<&'static str>,
}
impl ComparisonSession {
    pub fn start(study: Arc<Study>, series: usize, ctx: &egui::Context) -> Option<Self> {
        let other = if series == 0 { 1 } else { 0 };
        Some(Self {
            panes: [
                Pane::new(&study, series, ctx)?,
                Pane::new(&study, other, ctx)?,
            ],
            study,
            active: 0,
            linked: false,
            notice: None,
        })
    }
    pub fn active_series(&self) -> usize {
        self.panes[self.active].series
    }
    pub fn select_active(&mut self, series: usize, ctx: &egui::Context, frame: &eframe::Frame) {
        if self.replace_active(series, ctx) {
            viewport::clear_slot(frame, 8 + self.active);
        }
    }
    fn replace_active(&mut self, series: usize, ctx: &egui::Context) -> bool {
        if self.active_series() != series
            && let Some(pane) = Pane::new(&self.study, series, ctx)
        {
            self.panes[self.active] = pane;
            self.linked = false;
            self.notice = None;
            return true;
        }
        false
    }
    pub fn picture(&self) -> Option<(usize, ViewSpec, crate::export::Pixels, [u32; 2])> {
        let pane = &self.panes[self.active];
        let (spec, image, size) = pane.picture()?;
        Some((pane.series, spec, crate::export::Pixels::Image(image), size))
    }
    fn compatible(&self) -> bool {
        match (&self.panes[0].geometry, &self.panes[1].geometry) {
            (Some(left), Some(right)) => left.compatible(right),
            _ => false,
        }
    }
    fn synchronize(&mut self, from: usize) {
        let target = 1 - from;
        let index = self.panes[from]
            .geometry
            .as_ref()
            .and_then(|source| source.center(self.panes[from].view.slice as usize))
            .and_then(|point| self.panes[target].geometry.as_ref()?.nearest(point));
        if let Some(index) = index {
            self.panes[target].view.slice = index as u32;
            self.notice = None;
        } else {
            self.notice = Some(
                "The source image center is outside the other series. Its image is unchanged.",
            );
        }
    }
    pub fn ui(&mut self, ui: &mut egui::Ui, frame: &eframe::Frame) {
        for pane in &mut self.panes {
            pane.poll();
        }
        ui.weak("Local comparison · sharing is off. Select a pane, then choose its series in the sidebar.");
        ui.horizontal_wrapped(|ui| {
            for index in 0..2 { ui.selectable_value(&mut self.active, index, if index == 0 { "Left pane" } else { "Right pane" }); }
            let compatible = self.compatible();
            if ui.add_enabled(compatible, egui::Checkbox::new(&mut self.linked, "Link physical slices")).on_hover_text("Requires regular parallel planes in the same declared spatial reference. Uses physical image centers; does not register anatomy or link contrast/pan/zoom.").changed() && self.linked { self.synchronize(self.active); }
        });
        self.link_status(ui);
        let before = self.panes.each_ref().map(|pane| pane.view.slice);
        ui.push_id("comparison-controls", |ui| {
            self.panes[self.active].controls(ui)
        });
        let size = ui.available_size();
        let stacked = size.x < 640.;
        let gap = ui.spacing().item_spacing;
        let pane_size = if stacked {
            egui::vec2(size.x, ((size.y - gap.y) / 2.).max(94.))
        } else {
            egui::vec2(((size.x - gap.x) / 2.).max(80.), size.y.max(94.))
        };
        let layout = if stacked {
            egui::Layout::top_down(egui::Align::Min)
        } else {
            egui::Layout::left_to_right(egui::Align::Min)
        };
        let mut interacted = None;
        ui.with_layout(layout, |ui| {
            for index in 0..2 {
                let active = self.active == index;
                ui.push_id(index, |ui| {
                    ui.allocate_ui_with_layout(
                        pane_size,
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            if self.panes[index].canvas(ui, frame, index, pane_size, active) {
                                interacted = Some(index);
                            }
                        },
                    );
                });
            }
        });
        if let Some(index) = interacted
            && index != self.active
        {
            self.active = index;
            ui.ctx().request_repaint();
        }
        let changed = (0..2).find(|index| self.panes[*index].view.slice != before[*index]);
        if self.linked
            && self.compatible()
            && let Some(index) = changed
        {
            self.synchronize(index);
        }
        for pane in &mut self.panes {
            pane.worker.request(pane.view.slice as usize);
        }
    }
    fn link_status(&self, ui: &mut egui::Ui) {
        let text = self.notice.unwrap_or_else(|| {
            if self.compatible() { "Physical slice linking is available. Contrast, pan and zoom remain independent." }
            else if self.panes.iter().any(|pane| pane.geometry_task.is_some()) { "Checking geometry for optional slice linking…" }
            else { "Independent navigation: slice linking needs regular parallel planes and the same spatial reference." }
        });
        let height = ui.text_style_height(&egui::TextStyle::Small) * 2. + 4.;
        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), height),
            egui::Sense::hover(),
        );
        let mut child = ui.new_child(egui::UiBuilder::new().id_salt("link-status").max_rect(rect));
        child.set_clip_rect(ui.clip_rect().intersect(rect));
        child
            .add(egui::Label::new(egui::RichText::new(text).small().weak()).wrap())
            .on_hover_ui(|ui| {
                ui.weak(text);
                for (index, pane) in self.panes.iter().enumerate() {
                    if let Some(error) = &pane.geometry_error {
                        ui.weak(format!(
                            "{}: {error}",
                            if index == 0 { "Left" } else { "Right" }
                        ));
                    }
                }
            });
    }
}
