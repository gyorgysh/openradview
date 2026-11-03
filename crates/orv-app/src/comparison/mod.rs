// SPDX-License-Identifier: GPL-3.0-or-later
//! Temporary local source panes. Agent composition deliberately stays separate.
mod display_link;
mod fusion_ui;
mod layout;
mod navigation;
mod reference_ui;
mod saved;
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
    study: Arc<Study>,
    source: Arc<Series>,
    view: ViewState,
    worker: FrameWorker,
    image: Option<(usize, Arc<Frame>)>,
    last_view: Option<ViewState>,
    picture_size: Option<[u32; 2]>,
    canvas_aspect: Option<f64>,
    error: Option<String>,
    error_index: Option<usize>,
    geometry_task: Option<navigation::Task>,
    geometry: Option<NavigationStack>,
    geometry_error: Option<String>,
    annotations: Annotations,
    scroll: f32,
    tool: Tool,
    initialize_window: bool,
    /// Display-only fused texture and opacity drawn over this pane.
    fused: Option<(egui::TextureId, f32)>,
}
impl Pane {
    fn new(study: Arc<Study>, series: usize, ctx: &egui::Context) -> Option<Self> {
        let source = Arc::new(study.series.get(series)?.clone());
        Some(Self::from_source(study, series, source, ctx))
    }
    fn from_source(
        study: Arc<Study>,
        series: usize,
        source: Arc<Series>,
        ctx: &egui::Context,
    ) -> Self {
        let index = source.frame_count() / 2;
        let view = ViewState {
            slice: index as u32,
            ..Default::default()
        };
        Self {
            series,
            study,
            view,
            worker: FrameWorker::start(source.clone(), index, ctx.clone()),
            geometry_task: Some(navigation::Task::start(source.clone(), ctx.clone())),
            source,
            image: None,
            last_view: None,
            picture_size: None,
            canvas_aspect: None,
            error: None,
            error_index: None,
            geometry: None,
            geometry_error: None,
            annotations: Annotations::default(),
            scroll: 0.,
            tool: Tool::Pan,
            initialize_window: true,
            fused: None,
        }
    }
    fn poll(&mut self) {
        if let Some(result) = self.worker.take() {
            match result.image {
                Ok(image) => {
                    if self.initialize_window {
                        self.view.window = image.default_window;
                        self.initialize_window = false;
                    }
                    self.image = Some((result.index, image));
                    self.error = None;
                    self.error_index = None;
                }
                Err(error) => {
                    self.error_index = Some(result.index);
                    self.error = Some(error.to_string());
                }
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
    }
    fn input(&mut self, ui: &egui::Ui) {
        let color = self
            .image
            .as_ref()
            .is_some_and(|(_, image)| image.rgba.is_some());
        if color && self.tool == Tool::Window {
            self.tool = Tool::Pan;
        }
        crate::actions::tool_input(ui, &mut self.tool, !color, false, true);
        if crate::actions::pressed(ui, crate::actions::Action::Next) {
            self.view.slice = self
                .view
                .slice
                .saturating_add(1)
                .min(self.source.frame_count().saturating_sub(1) as u32);
        }
        if crate::actions::pressed(ui, crate::actions::Action::Previous) {
            self.view.slice = self.view.slice.saturating_sub(1);
        }
        if crate::actions::pressed(ui, crate::actions::Action::Fit) {
            self.view.pan = [0.; 2];
            self.view.zoom = 1.;
        }
        if crate::actions::pressed(ui, crate::actions::Action::Invert) {
            self.view.invert = !self.view.invert;
        }
    }
    fn canvas(
        &mut self,
        ui: &mut egui::Ui,
        frame: &eframe::Frame,
        pane: usize,
        size: egui::Vec2,
        active: bool,
        reference_lines: bool,
    ) -> (bool, Option<reference_ui::Target>) {
        let mut reference = None;
        let title = ui.add(
            egui::Label::new(format!("PANE {} · {}", pane + 1, self.source.label())).truncate(),
        );
        let footer =
            ui.spacing().interact_size.y.max(
                ui.text_style_height(&egui::TextStyle::Body) + ui.spacing().button_padding.y * 2.,
            );
        let (rect, response) = ui.allocate_exact_size(
            egui::vec2(
                size.x,
                (size.y - title.rect.height() - footer - ui.spacing().item_spacing.y * 2.).max(1.),
            ),
            egui::Sense::click_and_drag(),
        );
        let interacted = activates_pane(ui, &response);
        self.canvas_aspect = (rect.width() > 0. && rect.height() > 0.)
            .then(|| f64::from(rect.width()) / f64::from(rect.height()));
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
            if !pending
                && let Some((texture, opacity)) = self.fused
                && let Some(mesh) =
                    crate::labels::overlay_mesh(rect, image, &view, texture, opacity)
            {
                ui.painter_at(rect).add(egui::Shape::mesh(mesh));
            }
            if reference_lines && !pending {
                reference = Some(reference_ui::Target::reserve(
                    ui,
                    rect,
                    image.clone(),
                    view.clone(),
                ));
            }
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
        let slider = controls::image_number(ui, &mut self.view.slice, self.source.frame_count());
        (
            interacted || slider.dragged() || slider.changed(),
            reference,
        )
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
    panes: Vec<Pane>,
    active: usize,
    pair: [usize; 2],
    close_extra_question: bool,
    clear_extra_slots: bool,
    show_display: bool,
    linked: bool,
    reference_lines: bool,
    reference_status: reference_ui::Status,
    display_links: display_link::Group,
    notice: Option<&'static str>,
    file_task: Option<saved::Task>,
    file_notice: Option<&'static str>,
    restore_question: bool,
    saved_marks: [Vec<crate::annotations::SavedMark>; 2],
    saved_views: Option<[ViewState; 2]>,
    composition_dirty: bool,
    fusion: fusion_ui::Fusion,
}
impl ComparisonSession {
    #[cfg(test)]
    pub fn add_test_measurement(&mut self, index: usize) {
        use crate::annotations::{SavedMark, SavedTool};
        let mark = SavedMark {
            frame: 0,
            tool: SavedTool::Angle,
            points: vec![[0., 0.], [1., 1.], [2., 0.]],
        };
        self.panes[index].annotations =
            Annotations::restore(&[mark], &self.panes[index].source, || false).unwrap();
    }
    pub fn start(study: Arc<Study>, series: usize, ctx: &egui::Context) -> Option<Self> {
        let other = if series == 0 && study.series.len() > 1 {
            1
        } else {
            0
        };
        Some(Self {
            panes: vec![
                Pane::new(study.clone(), series, ctx)?,
                Pane::new(study, other, ctx)?,
            ],
            active: 0,
            pair: [0, 1],
            close_extra_question: false,
            clear_extra_slots: false,
            show_display: false,
            linked: false,
            reference_lines: false,
            reference_status: reference_ui::Status::Off,
            display_links: display_link::Group::default(),
            notice: None,
            file_task: None,
            file_notice: None,
            restore_question: false,
            saved_marks: [Vec::new(), Vec::new()],
            saved_views: None,
            composition_dirty: false,
            fusion: Default::default(),
        })
    }
    pub fn active_series(&self) -> usize {
        self.panes[self.active].series
    }
    pub fn adopt_source_view(
        &mut self,
        annotations: Annotations,
        view: ViewState,
        tool: Tool,
        initialize_window: bool,
    ) {
        let pane = &mut self.panes[0];
        pane.annotations = annotations;
        pane.view = view;
        pane.tool = tool;
        pane.initialize_window = initialize_window;
        pane.worker.request(pane.view.slice as usize);
    }
    pub fn active_study(&self) -> &Arc<Study> {
        &self.panes[self.active].study
    }
    pub fn active_source(&self) -> Arc<Series> {
        self.panes[self.active].source.clone()
    }
    pub fn open_right(&mut self, study: Arc<Study>, ctx: &egui::Context) -> bool {
        if self.temporary_panes() {
            self.file_notice = Some("Return to two panes before opening another study.");
            return false;
        }
        let Some(pane) = Pane::new(study, 0, ctx) else {
            return false;
        };
        self.panes[1] = pane;
        self.saved_marks[1].clear();
        self.composition_dirty |= self.saved_views.is_some();
        self.saved_views = None;
        self.active = 1;
        self.linked = false;
        self.display_links = display_link::Group::default();
        self.reference_status = if self.reference_lines {
            reference_ui::Status::Pending
        } else {
            reference_ui::Status::Off
        };
        self.notice = None;
        self.file_notice = None;
        true
    }
    pub fn select_active(&mut self, series: usize, ctx: &egui::Context, frame: &eframe::Frame) {
        if self.replace_active(series, ctx) {
            viewport::clear_slot(frame, 8 + self.active);
        }
    }
    fn replace_active(&mut self, series: usize, ctx: &egui::Context) -> bool {
        if self.active_series() != series
            && let Some(pane) = Pane::new(self.active_study().clone(), series, ctx)
        {
            self.panes[self.active] = pane;
            if let Some(marks) = self.saved_marks.get_mut(self.active) {
                marks.clear();
                self.composition_dirty |= self.saved_views.is_some();
                self.saved_views = None;
            }
            self.linked = false;
            self.display_links = display_link::Group::default();
            self.reference_status = if self.reference_lines {
                reference_ui::Status::Pending
            } else {
                reference_ui::Status::Off
            };
            self.notice = None;
            self.file_notice = None;
            return true;
        }
        false
    }
    pub(crate) fn illustration_outlines(
        &self,
        image: &Frame,
        view: &ViewState,
    ) -> Result<crate::annotations::Outlines, &'static str> {
        self.panes[self.active].annotations.outlines(image, view)
    }
    pub fn picture(&self) -> Option<(usize, ViewSpec, crate::export::Pixels, [u32; 2])> {
        let pane = &self.panes[self.active];
        let (spec, image, size) = pane.picture()?;
        Some((pane.series, spec, crate::export::Pixels::Image(image), size))
    }
    pub fn busy(&self) -> bool {
        self.file_task.is_some() || self.restore_question || self.close_extra_question
    }
    pub fn pane_unsaved(&self, index: usize) -> bool {
        let pane = &self.panes[index];
        if self
            .saved_views
            .as_ref()
            .and_then(|views| views.get(index))
            .is_some_and(|view| *view != pane.view)
        {
            return true;
        }
        pane.annotations.capture().map_or(true, |marks| {
            marks.into_iter().map(|mark| mark.saved).collect::<Vec<_>>()
                != self
                    .saved_marks
                    .get(index)
                    .map(Vec::as_slice)
                    .unwrap_or_default()
        })
    }
    pub fn unsaved(&self) -> bool {
        self.composition_dirty || (0..self.panes.len()).any(|index| self.pane_unsaved(index))
    }
    pub fn active_unsaved(&self) -> bool {
        self.pane_unsaved(self.active)
    }
    #[cfg(test)]
    fn discard_marks(&mut self) {
        for pane in &mut self.panes {
            pane.annotations.clear();
        }
        self.saved_marks = [Vec::new(), Vec::new()];
        self.saved_views = None;
        self.composition_dirty = false;
    }
    fn begin_save(&mut self, ctx: &egui::Context) {
        if self.panes.len() != 2 {
            self.file_notice = Some(
                "Return to two panes before saving a comparison. Extra panes stay in memory only.",
            );
            return;
        }
        let capture = |pane: &Pane| -> Result<saved::Capture, &'static str> {
            let (index, image) = pane
                .image
                .as_ref()
                .ok_or("Wait for both images before saving.")?;
            if *index != pane.view.slice as usize {
                return Err("Wait for both images before saving.");
            }
            Ok(saved::Capture {
                source: pane.source.clone(),
                image: image.clone(),
                view: pane.view.clone(),
                marks: pane.annotations.capture()?,
            })
        };
        match (capture(&self.panes[0]), capture(&self.panes[1])) {
            (Ok(left), Ok(right)) => {
                self.file_notice = None;
                self.file_task = Some(saved::Task::start(
                    saved::Action::Save([left, right], self.active),
                    ctx.clone(),
                ));
            }
            (Err(error), _) | (_, Err(error)) => self.file_notice = Some(error),
        }
    }
    fn begin_open(&mut self, ctx: &egui::Context) {
        self.restore_question = false;
        if self.panes.len() != 2 {
            self.file_notice = Some("Return to two panes before opening a saved comparison.");
            return;
        }
        if self.panes.iter().any(|pane| {
            pane.image
                .as_ref()
                .is_none_or(|(index, _)| *index != pane.view.slice as usize)
        }) {
            self.file_notice = Some("Wait for both images before opening a saved comparison.");
            return;
        }
        self.file_notice = None;
        self.file_task = Some(saved::Task::start(
            saved::Action::Open(std::array::from_fn(|index| {
                self.panes[index].source.clone()
            })),
            ctx.clone(),
        ));
    }
    pub fn file_dialogs(&mut self, ctx: &egui::Context) {
        self.layout_question(ctx);
        if let Some(result) = self.file_task.as_ref().and_then(saved::Task::poll) {
            self.file_task = None;
            match result {
                Ok(saved::Outcome::Saved { marks, views }) => {
                    self.saved_marks = marks;
                    self.saved_views = Some(views);
                    self.composition_dirty = false;
                    self.file_notice =
                        Some("Comparison saved locally. Keep the original images to reopen it.");
                }
                Ok(saved::Outcome::Restored { panes, active }) => {
                    for (index, restored) in (*panes).into_iter().enumerate() {
                        let pane = &mut self.panes[index];
                        pane.view = restored.view;
                        pane.annotations = restored.annotations;
                        pane.worker.request(pane.view.slice as usize);
                        self.saved_marks[index] = restored.marks;
                    }
                    self.active = active;
                    self.saved_views =
                        Some(std::array::from_fn(|index| self.panes[index].view.clone()));
                    self.composition_dirty = false;
                    self.linked = false;
                    self.display_links = display_link::Group::default();
                    self.reference_status = if self.reference_lines {
                        reference_ui::Status::Pending
                    } else {
                        reference_ui::Status::Off
                    };
                    self.notice = None;
                    self.file_notice = Some("Comparison restored after verifying both sources.");
                }
                Err(message) => self.file_notice = Some(message),
            }
        }
        if self.restore_question {
            egui::Modal::new(egui::Id::new("replace-comparison-marks")).show(ctx, |ui| {
                ui.heading("Open a saved comparison?");
                ui.label("Opening replaces the current views and measurements after both sources match. Save the current comparison first if you want to keep it.");
                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() { self.restore_question = false; }
                    if ui.button("Choose comparison…").clicked() { self.begin_open(ctx); }
                });
            });
        }
        if let Some(task) = &mut self.file_task {
            egui::Modal::new(egui::Id::new("comparison-file-progress")).show(ctx, |ui| {
                ui.heading("Working locally…");
                ui.label("Checking both sources can take time. No images or paths are saved in the comparison file.");
                if ui.add_enabled(!task.cancelling, egui::Button::new("Cancel")).clicked() { task.cancel(); }
                if task.cancelling { ui.weak("Cancelling…"); }
            });
        }
    }
    fn update_fusion(&mut self, ctx: &egui::Context) {
        for pane in &mut self.panes {
            pane.fused = None;
        }
        let current = |pane: &Pane| {
            pane.image
                .as_ref()
                .filter(|(index, _)| *index == pane.view.slice as usize)
                .map(|(_, image)| image.clone())
        };
        let [target, source] = self.pair.map(|index| &self.panes[index]);
        // Half the source stack's regular spacing: the nearest plane only.
        let offset = self
            .compatible()
            .then(|| {
                source
                    .geometry
                    .as_ref()
                    .map(|stack| stack.step() * 0.5 + 1e-6)
            })
            .flatten();
        let fused = self.fusion.update(
            ctx,
            current(target).as_ref(),
            current(source).as_ref().map(|image| (image, &source.view)),
            offset,
        );
        let target = self.pair[0];
        self.panes[target].fused = fused;
    }
    fn compatible(&self) -> bool {
        match (
            &self.panes[self.pair[0]].geometry,
            &self.panes[self.pair[1]].geometry,
        ) {
            (Some(left), Some(right)) => left.compatible(right),
            _ => false,
        }
    }
    fn synchronize(&mut self, from: usize) {
        let target = if from == self.pair[0] {
            self.pair[1]
        } else if from == self.pair[1] {
            self.pair[0]
        } else {
            return;
        };
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
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        frame: &eframe::Frame,
        can_open: bool,
    ) -> Option<crate::worker::Source> {
        if std::mem::take(&mut self.clear_extra_slots) {
            viewport::clear_slot(frame, 10);
            viewport::clear_slot(frame, 11);
        }
        for pane in &mut self.panes {
            pane.poll();
        }
        let before_views: Vec<_> = self.panes.iter().map(|pane| pane.view.clone()).collect();
        let before_aspects: Vec<_> = self.panes.iter().map(|pane| pane.canvas_aspect).collect();
        let previous_active = self.active;
        let mut open = None;
        let mut reconfigured = false;
        crate::toolbar::row(ui, |ui| {
            for index in 0..self.panes.len() {
                ui.selectable_value(&mut self.active, index, format!("Pane {}", index + 1));
            }
            ui.add_enabled_ui(can_open, |ui| self.layout_controls(ui));
            ui.toggle_value(&mut self.show_display, "Display and tools");
        });
        crate::toolbar::row(ui, |ui| {
            ui.add_enabled_ui(can_open, |ui| {
                ui.add_enabled_ui(!self.temporary_panes(), |ui| {
                    ui.menu_button("Open in pane 2…", |ui| {
                        if ui.button("Files or ZIP…").clicked() {
                            open = Some(crate::worker::Source::Files);
                            ui.close();
                        }
                        if ui.button("Folder…").clicked() {
                            open = Some(crate::worker::Source::Folder);
                            ui.close();
                        }
                    });
                }).response.on_disabled_hover_text("Return to two panes before opening another study.");
                ui.menu_button("Comparison…", |ui| {
                    ui.label("Save views and measurements locally, without images or paths. The file may still be sensitive.");
                    ui.label("To reopen, select the original series in the same left/right order first.");
                    if self.temporary_panes() { ui.label("Four-pane views stay in memory. Return to two panes to save or open a comparison file."); }
                    if ui.add_enabled(!self.temporary_panes(), egui::Button::new("Save comparison…")).clicked() { self.begin_save(ui.ctx()); ui.close(); }
                    if ui.add_enabled(!self.temporary_panes(), egui::Button::new("Open saved comparison…")).clicked() {
                        if self.unsaved() || self.panes.iter().any(|pane| pane.annotations.has_marks()) { self.restore_question = true; }
                        else { self.begin_open(ui.ctx()); }
                        ui.close();
                    }
                });
            });
            egui::containers::menu::MenuButton::new(format!("Links and reference · {} + {}", self.pair[0] + 1, self.pair[1] + 1))
            .config(egui::containers::menu::MenuConfig::new().close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside))
            .ui(ui, |ui| {
                ui.set_max_width(420.);
                egui::ScrollArea::vertical().id_salt("comparison-links-menu").max_height(ui.available_height().min(460.)).show(ui, |ui| {
                    ui.weak("Local comparison · sharing is off");
                    self.pair_controls(ui);
                    let compatible = self.compatible() && self.pair.contains(&self.active);
                    if ui.add_enabled(compatible, egui::Checkbox::new(&mut self.linked, "Link physical slices"))
                        .on_hover_text("Requires regular parallel planes in the same declared spatial reference; no registration.")
                        .changed() && self.linked { self.synchronize(self.active); }
                    let response = ui.checkbox(&mut self.reference_lines, "Reference lines")
                        .on_hover_text("Finite thin-plane intersections for the chosen pair; exports and snapshots omit them.");
                    ui.ctx().accesskit_node_builder(response.id, |node| node.set_description(self.reference_status.text()));
                    if response.changed() { self.reference_status = if self.reference_lines { reference_ui::Status::Pending } else { reference_ui::Status::Off }; }
                    reconfigured = self.display_links.controls(ui, self.pair.contains(&self.active) && self.pair.iter().all(|index| !self.panes[*index].initialize_window));
                    ui.separator();
                    let fusable = self.compatible();
                    self.fusion.controls(ui, fusable, self.pair);
                    reference_ui::status(ui, self.reference_status);
                    self.display_links.status(ui);
                    crate::dialogs::reveal_focus(ui);
                });
            });
        });
        self.link_status(ui);
        let before: Vec<_> = self.panes.iter().map(|pane| pane.view.slice).collect();
        if self.show_display {
            let height = (ui.available_height() - 240.).max(80.);
            egui::ScrollArea::vertical()
                .id_salt("comparison-display-controls")
                .max_height(height)
                .show(ui, |ui| {
                    ui.push_id("comparison-controls", |ui| {
                        self.panes[self.active].controls(ui)
                    });
                    crate::dialogs::reveal_focus(ui);
                });
        }
        self.panes[self.active].input(ui);
        self.update_fusion(ui.ctx());
        let size = ui.available_size();
        let grid = layout::grid(self.panes.len(), size, ui.spacing().item_spacing);
        let pane_size = grid.pane;
        let mut interacted = None;
        let mut reference_targets: [Option<reference_ui::Target>; 4] =
            std::array::from_fn(|_| None);
        ui.vertical(|ui| {
            for row in 0..grid.rows {
                ui.horizontal_top(|ui| {
                    for column in 0..grid.columns {
                        let index = row * grid.columns + column;
                        let active = self.active == index;
                        ui.push_id(index, |ui| {
                            ui.allocate_ui_with_layout(
                                pane_size,
                                egui::Layout::top_down(egui::Align::Min),
                                |ui| {
                                    let (changed, reference) = self.panes[index].canvas(
                                        ui,
                                        frame,
                                        index,
                                        pane_size,
                                        active,
                                        self.reference_lines && self.pair.contains(&index),
                                    );
                                    reference_targets[index] = reference;
                                    if changed {
                                        interacted = Some(index);
                                    }
                                },
                            );
                        });
                    }
                });
            }
        });
        if let Some(index) = interacted
            && index != self.active
        {
            self.active = index;
            ui.ctx().request_repaint();
        }
        let changed = self
            .pair
            .into_iter()
            .find(|index| self.panes[*index].view.slice != before[*index]);
        if self.linked
            && self.compatible()
            && let Some(index) = changed
        {
            self.synchronize(index);
        }
        let display_update = self.display_links.update(display_link::Input {
            panes: self.pair.map(|index| &self.panes[index]),
            before: &self.pair.map(|index| {
                before_views
                    .get(index)
                    .cloned()
                    .unwrap_or_else(|| self.panes[index].view.clone())
            }),
            before_aspects: self
                .pair
                .map(|index| before_aspects.get(index).copied().flatten()),
            active: self
                .pair
                .iter()
                .position(|index| *index == self.active)
                .unwrap_or(0),
            previous_active: self
                .pair
                .iter()
                .position(|index| *index == previous_active)
                .unwrap_or(1),
            reconfigured,
            enabled: ui.is_enabled() && self.pair.contains(&self.active),
        });
        if let Some((target, view)) = display_update.view {
            self.panes[self.pair[target]].view = view;
            ui.ctx().request_repaint();
        }
        if display_update.status_changed {
            ui.ctx().request_repaint();
        }
        let status = if self.reference_lines {
            reference_ui::paint_pair(
                self.pair.map(|index| reference_targets[index].as_ref()),
                self.pair.map(|index| {
                    let pane = &self.panes[index];
                    pane.image
                        .as_ref()
                        .map(|(index, image)| (*index, image, &pane.view))
                }),
            )
        } else {
            reference_ui::Status::Off
        };
        if status != self.reference_status {
            self.reference_status = status;
            ui.ctx().request_repaint();
        }
        for pane in &mut self.panes {
            pane.worker.request(pane.view.slice as usize);
        }
        open
    }
    fn link_status(&self, ui: &mut egui::Ui) {
        let file_notice = match self.file_notice {
            Some(
                "Comparison saved locally. Keep the original images to reopen it."
                | "Comparison restored after verifying both sources.",
            ) if self.unsaved() => Some("Comparison has unsaved changes."),
            other => other,
        };
        let text = file_notice.or(self.notice).unwrap_or_else(|| {
            if self.compatible() { "Physical slice linking is available. Display links have separate controls; no registration is performed." }
            else if self.pair.iter().any(|index| self.panes[*index].geometry_task.is_some()) { "Checking geometry for optional slice linking…" }
            else { "Physical slice linking needs regular parallel planes and the same spatial reference." }
        });
        let height = ui.text_style_height(&egui::TextStyle::Small) + 4.;
        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), height),
            egui::Sense::hover(),
        );
        let mut child = ui.new_child(egui::UiBuilder::new().id_salt("link-status").max_rect(rect));
        child.set_clip_rect(ui.clip_rect().intersect(rect));
        child
            .add(egui::Label::new(egui::RichText::new(text).small().weak()).truncate())
            .on_hover_ui(|ui| {
                ui.weak(text);
                for index in self.pair {
                    if let Some(error) = &self.panes[index].geometry_error {
                        ui.weak(format!("Pane {}: {error}", index + 1));
                    }
                }
            });
    }
}
