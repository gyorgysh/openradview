// SPDX-License-Identifier: GPL-3.0-or-later
use crate::{
    annotations::Annotations,
    brand,
    mpr::MprSession,
    theme,
    viewport::{self, Tool},
    worker::{ImportTask, Source},
};
use eframe::egui::{self, RichText};
use orv_core::{
    ViewState,
    load::LoadError,
    study::{Frame, Study},
};
use std::sync::{Arc, atomic::Ordering, mpsc::TryRecvError};

#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub(super) enum ViewMode {
    #[default]
    Acquired,
    Mpr,
    Volume,
    Oblique,
    Compare,
}

struct ComparisonSource {
    study: Arc<Study>,
    browser: crate::series_browser::SeriesBrowser,
}
enum ComparisonAction {
    Mode(ViewMode),
    Close,
    Begin(Source),
    Select(usize),
    OpenRight(Source),
    ReturnLabels,
    Quit,
}

#[derive(Default)]
pub struct Workspace {
    pub(super) export: crate::export::PictureExport,
    pub(super) picture_size: Option<[u32; 2]>,
    pub(super) playback: crate::playback::Playback,
    demo: Option<crate::demos::Demo>,
    browser: crate::series_browser::SeriesBrowser,
    show_demo_credits: bool,
    pub(super) study: Option<Arc<Study>>,
    pub(super) image_worker: Option<crate::frame_worker::FrameWorker>,
    pub(super) decoded_image: Option<(usize, Arc<Frame>)>,
    pub(super) last_image_view: Option<ViewState>,
    pub(super) decode_error: Option<(usize, LoadError)>,
    pub(super) initialize_window: bool,
    pub(super) selected: usize,
    pub(super) view: ViewState,
    pub(super) tool: Tool,
    pub(super) loading: Option<ImportTask>,
    loading_comparison: bool,
    right_replace_confirmed: bool,
    comparison_source: Option<ComparisonSource>,
    pending_comparison: Option<ComparisonAction>,
    bypass_comparison_guard: bool,
    allow_comparison_quit: bool,
    error: Option<String>,
    pub(super) scroll: f32,
    clear_gpu: bool,
    pub(super) annotations: Annotations,
    pub(super) labels: crate::labels::LabelUi,
    pub(super) image_info: crate::image_info::ImageInfo,
    pending_source: Option<Source>,
    pending_close: bool,
    pending_quit: bool,
    discard_labels: bool,
    pub(super) mpr: Option<MprSession>,
    pub(super) mode: ViewMode,
    pub(super) volume: Option<crate::volume_ui::VolumeSession>,
    pub(super) oblique: Option<crate::oblique_ui::ObliqueSession>,
    pub(super) comparison: Option<crate::comparison::ComparisonSession>,
    #[cfg(feature = "mcp")]
    pub(super) sharing: crate::sharing::Sharing,
}
impl Workspace {
    pub(super) fn set_mode(&mut self, mode: ViewMode, ctx: &egui::Context) {
        if self.mode == mode {
            return;
        }
        if self.mode == ViewMode::Compare {
            if !self.bypass_comparison_guard
                && self
                    .comparison
                    .as_ref()
                    .is_some_and(|comparison| comparison.unsaved())
            {
                self.pending_comparison = Some(ComparisonAction::Mode(mode));
                return;
            }
            let external_active = self
                .comparison
                .as_ref()
                .zip(self.comparison_source.as_ref())
                .is_some_and(|(comparison, source)| {
                    Arc::ptr_eq(comparison.active_study(), &source.study)
                });
            if external_active && (self.labels.dirty || self.labels.busy()) {
                self.error = Some("Select the original source pane before leaving comparison; it has unsaved labels.".into());
                return;
            }
            self.error = None;
            if self.loading_comparison {
                self.loading = None;
                self.loading_comparison = false;
                self.right_replace_confirmed = false;
            }
            if let (Some(comparison), Some(source)) = (&self.comparison, &self.comparison_source)
                && Arc::ptr_eq(comparison.active_study(), &source.study)
                && let Some(source) = self.comparison_source.take()
            {
                self.study = Some(source.study);
                self.browser = source.browser;
                self.demo = None;
                self.labels = Default::default();
            }
            let selected = self
                .comparison
                .as_ref()
                .map_or(self.selected, |comparison| comparison.active_series());
            self.select(selected);
            self.comparison_source = None;
        }
        self.export.close();
        self.image_info.close();
        self.labels.stop_painting();
        self.labels.open = false;
        let previous_mode = self.mode;
        self.mode = mode;
        if mode == ViewMode::Compare {
            #[cfg(feature = "mcp")]
            self.sharing.enter_local_comparison();
            self.mpr = None;
            self.image_worker = None;
            self.decoded_image = None;
            self.last_image_view = None;
            self.comparison = self.study.as_ref().and_then(|study| {
                crate::comparison::ComparisonSession::start(study.clone(), self.selected, ctx)
            });
            if (previous_mode == ViewMode::Acquired || self.annotations.has_marks())
                && let Some(comparison) = &mut self.comparison
            {
                comparison.adopt_source_view(
                    std::mem::take(&mut self.annotations),
                    self.view.clone(),
                    self.tool,
                    self.initialize_window,
                );
            }
            if self.comparison.is_none() {
                self.mode = ViewMode::Acquired;
            }
        }
        self.discard_inactive_views();
        if let Some(frames) = self
            .study
            .as_ref()
            .and_then(|study| study.series.get(self.selected))
            .map(|series| Arc::new(series.clone()))
        {
            match self.mode {
                ViewMode::Mpr if self.mpr.is_none() => {
                    self.mpr = Some(MprSession::start(frames, self.view.window, ctx.clone()))
                }
                ViewMode::Volume => {
                    self.volume = Some(crate::volume_ui::VolumeSession::start(frames))
                }
                ViewMode::Oblique => {
                    self.oblique = Some(crate::oblique_ui::ObliqueSession::start(
                        frames,
                        self.view.window,
                        ctx.clone(),
                    ))
                }
                _ => (),
            }
        }
    }
    pub(crate) fn local_dialog_open(&self) -> bool {
        self.image_info.is_open()
            || self.export.is_open()
            || self.labels.open
            || self.labels.discard_question
            || self.discard_labels
            || self.show_demo_credits
            || self.pending_comparison.is_some()
            || self
                .comparison
                .as_ref()
                .is_some_and(|comparison| comparison.busy())
    }
    pub(super) fn discard_inactive_views(&mut self) {
        self.pause_playback();
        self.clear_gpu = true;
        if self.mode != ViewMode::Volume {
            self.volume = None;
        }
        if self.mode != ViewMode::Oblique {
            self.oblique = None;
        }
    }
    fn begin(&mut self, source: Source, ctx: &egui::Context) {
        self.pause_playback();
        if self.loading.is_some() {
            return;
        }
        if !self.bypass_comparison_guard
            && self
                .comparison
                .as_ref()
                .is_some_and(|comparison| comparison.unsaved())
        {
            self.pending_comparison = Some(ComparisonAction::Begin(source));
            return;
        }
        if self.labels.dirty || self.labels.busy() {
            self.pending_source = Some(source);
            self.labels.discard_question = true;
            return;
        }
        #[cfg(feature = "mcp")]
        self.sharing.revoke();
        self.error = None;
        self.loading = Some(ImportTask::start(source, ctx.clone()));
    }
    pub(super) fn close(&mut self) {
        self.pause_playback();
        if !self.bypass_comparison_guard
            && self
                .comparison
                .as_ref()
                .is_some_and(|comparison| comparison.unsaved())
        {
            self.pending_comparison = Some(ComparisonAction::Close);
            return;
        }
        if self.labels.dirty || self.labels.busy() {
            self.pending_close = true;
            self.labels.discard_question = true;
            return;
        }
        self.close_now();
    }
    fn close_now(&mut self) {
        self.export.close();
        self.picture_size = None;
        self.labels = Default::default();
        #[cfg(feature = "mcp")]
        self.sharing.revoke();
        self.mpr = None;
        self.mode = ViewMode::Acquired;
        self.volume = None;
        self.oblique = None;
        self.comparison = None;
        self.comparison_source = None;
        self.annotations = Default::default();
        self.image_worker = None;
        self.decoded_image = None;
        self.last_image_view = None;
        self.decode_error = None;
        self.image_info.close();
        self.browser = Default::default();
        self.study = None;
        self.demo = None;
        self.loading = None;
        self.loading_comparison = false;
        self.right_replace_confirmed = false;
        self.pending_comparison = None;
        self.allow_comparison_quit = false;
        self.clear_gpu = true;
        self.error = None;
        self.selected = 0;
        self.view = ViewState::default();
    }
    fn select(&mut self, index: usize) {
        if self.loading_comparison {
            self.loading = None;
            self.loading_comparison = false;
            self.right_replace_confirmed = false;
        }
        self.export.close();
        self.picture_size = None;
        self.pause_playback();
        self.image_info.close();
        #[cfg(feature = "mcp")]
        if self.selected != index {
            self.sharing.revoke();
        }
        self.mpr = None;
        self.mode = ViewMode::Acquired;
        self.volume = None;
        self.oblique = None;
        self.comparison = None;
        self.comparison_source = None;
        self.annotations = Default::default();
        self.clear_gpu = true;
        self.image_worker = None;
        self.decoded_image = None;
        self.last_image_view = None;
        self.decode_error = None;
        self.initialize_window = true;
        self.selected = index;
        self.view = ViewState::default();
        self.scroll = 0.0;
        if let Some(series) = self.study.as_ref().and_then(|s| s.series.get(index)) {
            self.view.slice = (series.frame_count() / 2) as u32;
            if let Some(image) = series.frames.get(self.view.slice as usize) {
                self.view.window = image.default_window;
            } else if let Some(window) = series
                .source
                .as_ref()
                .and_then(|s| s.original_window(self.view.slice as usize))
            {
                self.view.window = window;
            }
        }
    }
    fn poll(&mut self, ctx: &egui::Context) {
        if self
            .loading
            .as_ref()
            .is_some_and(|task| task.cancel.load(Ordering::Relaxed))
        {
            self.loading = None;
            self.loading_comparison = false;
            self.right_replace_confirmed = false;
            return;
        }
        let result = self.loading.as_ref().map(|task| task.receiver.try_recv());
        match result {
            Some(Ok(Ok(Some(study)))) => {
                if self.loading_comparison {
                    self.loading = None;
                    self.loading_comparison = false;
                    let confirmed = std::mem::take(&mut self.right_replace_confirmed);
                    if !confirmed
                        && self
                            .comparison
                            .as_ref()
                            .is_some_and(|comparison| comparison.pane_unsaved(1))
                    {
                        self.error = Some("The right pane has unsaved measurements. Save them before replacing its source.".into());
                        return;
                    }
                    let source = Arc::new(study.study);
                    if let Some(comparison) = &mut self.comparison
                        && comparison.open_right(source.clone(), ctx)
                    {
                        self.comparison_source = Some(ComparisonSource {
                            browser: crate::series_browser::SeriesBrowser::new(
                                study.catalog,
                                source.clone(),
                                ctx.clone(),
                            ),
                            study: source,
                        });
                        self.export.close();
                        self.clear_gpu = true;
                    }
                    return;
                }
                self.demo = self.loading.as_ref().and_then(|task| task.demo);
                self.loading = None;
                self.labels = Default::default();
                let source = Arc::new(study.study);
                self.browser = crate::series_browser::SeriesBrowser::new(
                    study.catalog,
                    source.clone(),
                    ctx.clone(),
                );
                self.study = Some(source);
                self.clear_gpu = true;
                self.select(0);
            }
            Some(Ok(Ok(None))) => {
                self.loading = None;
                self.loading_comparison = false;
                self.right_replace_confirmed = false;
            }
            Some(Ok(Err(error))) => {
                self.loading = None;
                self.loading_comparison = false;
                self.right_replace_confirmed = false;
                self.error = Some(error.to_string());
            }
            Some(Err(TryRecvError::Disconnected)) => {
                self.loading = None;
                self.loading_comparison = false;
                self.right_replace_confirmed = false;
                self.error = Some("The import worker stopped. Try a smaller input.".into());
            }
            _ => {}
        }
    }
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        frame: &mut eframe::Frame,
        seconds: f32,
        reduce_motion: bool,
        external_dialog_open: bool,
    ) {
        let local_dialog_was_open = self.local_dialog_open();
        self.poll(ui.ctx());
        self.labels.poll();
        self.poll_image(ui.ctx());
        #[cfg(feature = "mcp")]
        self.sharing_before_ui(ui.ctx(), external_dialog_open);
        let series = self
            .study
            .as_ref()
            .and_then(|s| s.series.get(self.selected))
            .map(|s| Arc::new(s.clone()));
        let slice = self
            .current_image()
            .and_then(|(_, f)| self.labels.current.as_ref()?.slice_index(&f));
        let other_modal = self.sharing_modal_open()
            || self.labels.discard_question
            || self.discard_labels
            || self.pending_comparison.is_some()
            || self
                .comparison
                .as_ref()
                .is_some_and(|comparison| comparison.busy());
        if other_modal {
            self.image_info.close();
        }
        let blocked = external_dialog_open
            || other_modal
            || self.image_info.is_open()
            || self.export.is_open();
        if blocked
            || self.show_demo_credits
            || self.labels.open
            || self.loading.is_some()
            || ui.input(|i| !i.focused || i.viewport().minimized == Some(true))
        {
            self.pause_playback();
        }
        if !blocked && self.mode != ViewMode::Compare {
            self.labels.window(ui.ctx(), self.selected, series, slice);
        }
        ui.add_enabled_ui(!blocked, |ui| {
            self.content_ui(ui, frame, seconds, reduce_motion)
        });
        #[cfg(feature = "mcp")]
        self.sharing_after_ui(ui.ctx());
        self.image_info.show(ui.ctx());
        self.export.show(ui.ctx());
        crate::demos::credits(ui.ctx(), &mut self.show_demo_credits);
        self.label_discard_dialog(ui.ctx());
        if let Some(comparison) = &mut self.comparison {
            comparison.file_dialogs(ui.ctx());
        }
        self.comparison_discard_dialog(ui.ctx(), frame);
        if local_dialog_was_open && !self.local_dialog_open() {
            ui.ctx().request_repaint();
        }
    }
    pub(super) fn sharing_active(&self) -> bool {
        #[cfg(feature = "mcp")]
        {
            self.sharing.is_active()
        }
        #[cfg(not(feature = "mcp"))]
        {
            false
        }
    }
    fn sharing_modal_open(&self) -> bool {
        #[cfg(feature = "mcp")]
        {
            self.sharing.modal_open()
        }
        #[cfg(not(feature = "mcp"))]
        {
            false
        }
    }
    fn content_ui(
        &mut self,
        ui: &mut egui::Ui,
        frame: &mut eframe::Frame,
        seconds: f32,
        reduce_motion: bool,
    ) {
        let ctx = ui.ctx().clone();
        if ui.is_enabled()
            && ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::O))
        {
            self.begin(Source::Files, &ctx);
        }
        let dropped = ui.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .collect::<Vec<_>>()
        });
        if ui.is_enabled() && !dropped.is_empty() {
            self.begin(Source::Paths(dropped), &ctx);
        }
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
            ui.spacing_mut().button_padding = egui::vec2(10., 5.);
            if ui
                .add_enabled(self.loading.is_none(), egui::Button::new("Open files"))
                .on_hover_text("DICOM, NIfTI or ZIP · Cmd/Ctrl+O")
                .clicked()
            {
                self.begin(Source::Files, &ctx);
            }
            if ui
                .add_enabled(self.loading.is_none(), egui::Button::new("Open folder"))
                .clicked()
            {
                self.begin(Source::Folder, &ctx);
            }
            if self.study.is_some() {
                ui.menu_button("Study", |ui| {
                    let ready = self.picture_ready();
                    if ui.add_enabled(ready, egui::Button::new("Export picture…"))
                        .on_hover_text("Save the current pane and view settings locally, without app overlays.")
                        .clicked() {
                        self.open_picture_export(frame);
                        ui.close();
                    }
                    if ui.button("Close study").clicked() { self.close(); ui.close(); }
                });
            }
            ui.add_enabled_ui(self.loading.is_none(), |ui| {
                ui.menu_button("Demos", |ui| {
                    for demo in crate::demos::Demo::ALL {
                        if ui.button(demo.title()).clicked() {
                            self.begin(Source::Demo(demo), &ctx);
                            ui.close();
                        }
                    }
                    ui.separator();
                    if ui.button("Source credits").clicked() {
                        self.show_demo_credits = true;
                        ui.close();
                    }
                });
            });
            if self.study.is_some() {
                let mut mode = self.mode;
                ui.selectable_value(&mut mode, ViewMode::Acquired, "2D image");
                ui.selectable_value(&mut mode, ViewMode::Mpr, "Linked MPR");
                ui.selectable_value(&mut mode, ViewMode::Volume, "3D / MIP");
                ui.selectable_value(&mut mode, ViewMode::Oblique, "Oblique");
                if ui.add_enabled(
                    self.study.as_ref().is_some_and(|study| !study.series.is_empty()),
                    egui::Button::selectable(mode == ViewMode::Compare, "Compare series"),
                ).clicked() { mode = ViewMode::Compare; }
                if mode != self.mode { self.set_mode(mode, ui.ctx()); }
            }
            #[cfg(feature = "mcp")]
            {
                if ui.available_width() < 400. { ui.end_row(); }
                self.agent_toolbar(ui);
            }
            if let Some(task) = &self.loading {
                ui.label(format!(
                    "Indexing · {} files",
                    task.processed.load(Ordering::Relaxed)
                ));
                let checked = task.checked_bytes.load(Ordering::Relaxed);
                if checked >= 1024 * 1024 {
                    ui.weak(format!(
                        "{:.1} GiB checked locally",
                        checked as f64 / (1024. * 1024. * 1024.)
                    ));
                }
                if ui.button("Cancel import").clicked() {
                    task.cancel.store(true, Ordering::Relaxed);
                }
            }
        });

        if let Some(error) = &self.error {
            ui.label(RichText::new(error).color(theme::SILVER));
        }
        ui.separator();
        if self.clear_gpu {
            viewport::clear(frame);
            self.clear_gpu = false;
        }
        egui::Panel::left("series")
            .default_size(204.0)
            .resizable(true)
            .size_range(160.0..=300.0)
            .frame(egui::Frame::new().inner_margin(egui::Margin {
                left: 0,
                right: 16,
                top: 16,
                bottom: 10,
            }))
            .show(ui, |ui| {
                ui.add_enabled_ui(self.loading.is_none(), |ui| self.sidebar(ui, frame));
            });
        egui::CentralPanel::default()
            .frame(egui::Frame::new().inner_margin(16.0))
            .show(ui, |ui| {
                ui.spacing_mut().button_padding = egui::vec2(10., 5.);
                ui.spacing_mut().item_spacing = egui::vec2(8., 6.);
                if self.study.is_some() {
                    match self.mode {
                        ViewMode::Mpr => {
                            if let Some(mpr) = &mut self.mpr {
                                mpr.ui(ui, frame);
                            }
                        }
                        ViewMode::Volume => {
                            if let Some(volume) = &mut self.volume {
                                volume.ui(ui, frame);
                            }
                        }
                        ViewMode::Oblique => {
                            if let Some(oblique) = &mut self.oblique {
                                oblique.ui(ui, frame);
                            }
                        }
                        ViewMode::Acquired => self.image_ui(ui, frame),
                        ViewMode::Compare => {
                            if let Some(comparison) = &mut self.comparison
                                && let Some(source) = ui
                                    .add_enabled_ui(self.loading.is_none(), |ui| {
                                        comparison.ui(ui, frame, true)
                                    })
                                    .inner
                            {
                                self.open_comparison_right(source, &ctx);
                            }
                        }
                    }
                } else {
                    self.empty(ui, seconds, reduce_motion);
                }
            });
    }
    fn sidebar(&mut self, ui: &mut egui::Ui, frame: &eframe::Frame) {
        theme::eyebrow(ui, "LOCAL SERIES");
        ui.add_space(12.0);
        if self.study.is_some() {
            let secondary_active = self
                .comparison
                .as_ref()
                .zip(self.comparison_source.as_ref())
                .is_some_and(|(comparison, source)| {
                    self.mode == ViewMode::Compare
                        && Arc::ptr_eq(comparison.active_study(), &source.study)
                });
            if let Some(demo) = self.demo.filter(|_| !secondary_active) {
                theme::eyebrow(ui, "PUBLIC RESEARCH DEMO");
                ui.label(demo.title());
                ui.small(demo.credit());
                if ui.small_button("Source credits").clicked() {
                    self.show_demo_credits = true;
                }
                ui.add_space(12.0);
            }
            let active_study = if secondary_active {
                self.comparison_source.as_ref().map(|source| &source.study)
            } else {
                self.study.as_ref()
            };
            if active_study.is_some_and(|study| study.synthetic) {
                theme::eyebrow(ui, "SYNTHETIC DEMO · NO PATIENT DATA");
            }
            let skipped_files = active_study.map_or(0, |study| study.skipped_files);
            let active_series = self
                .comparison
                .as_ref()
                .filter(|_| self.mode == ViewMode::Compare)
                .map_or(self.selected, |comparison| comparison.active_series());
            if secondary_active {
                ui.weak("Right pane source");
            } else if self.mode == ViewMode::Compare {
                ui.weak("Original source");
            }
            let browser = match (secondary_active, &mut self.comparison_source) {
                (true, Some(source)) => &mut source.browser,
                _ => &mut self.browser,
            };
            let select = browser.ui(ui, active_series, frame.wgpu_render_state());
            ui.add_space(18.0);
            if skipped_files > 0 {
                ui.label(
                    RichText::new(format!(
                        "{} unsupported or non-image files skipped",
                        skipped_files
                    ))
                    .small()
                    .color(theme::SILVER),
                );
            }
            if let Some(index) = select {
                if self.mode == ViewMode::Compare
                    && let Some(comparison) = &mut self.comparison
                {
                    if comparison.active_series() != index && comparison.active_unsaved() {
                        self.pending_comparison = Some(ComparisonAction::Select(index));
                    } else {
                        comparison.select_active(index, ui.ctx(), frame);
                        self.export.close();
                    }
                } else {
                    self.select(index);
                }
            }
        } else {
            ui.label("No study open");
            ui.label(
                RichText::new("Open or drop a local folder, DICOM, NIfTI or ZIP.")
                    .color(theme::SILVER),
            );
        }
        ui.add_space(22.0);
        ui.separator();
        theme::eyebrow(ui, "PRIVATE SESSION");
        let privacy = if self.sharing_active() {
            "Agent access is enabled.\nRevoke to end sharing.\nNo study history is saved."
        } else {
            "No scan data is shared.\nNo study history is saved."
        };
        ui.label(RichText::new(privacy).size(12.0).color(theme::SILVER));
    }
    fn empty(&mut self, ui: &mut egui::Ui, seconds: f32, reduce_motion: bool) {
        let height = ui.available_height();
        ui.vertical_centered(|ui| {
            ui.add_space((height - 340.0).max(0.0) / 2.0);
            brand::symbol(ui, 80.0, seconds, reduce_motion);
            ui.add_space(24.0);
            ui.heading("Open a study");
            ui.label(
                RichText::new("Open local images. Sharing with an agent is optional.")
                    .color(theme::SILVER),
            );
            ui.add_space(24.0);
            ui.label("Explore real scans");
            for demo in crate::demos::Demo::ALL {
                if ui
                    .add_enabled(self.loading.is_none(), egui::Button::new(demo.title()))
                    .clicked()
                {
                    self.begin(Source::Demo(demo), ui.ctx());
                }
            }
            ui.label(
                RichText::new("Public, de-identified research scans · included offline")
                    .small()
                    .color(theme::SILVER),
            );
        });
    }
}

impl Workspace {
    pub(super) fn pause_playback(&mut self) {
        self.playback.pause();
    }
    fn open_comparison_right(&mut self, source: Source, ctx: &egui::Context) {
        if !self.bypass_comparison_guard
            && self
                .comparison
                .as_ref()
                .is_some_and(|comparison| comparison.pane_unsaved(1))
        {
            self.pending_comparison = Some(ComparisonAction::OpenRight(source));
            return;
        }
        self.error = None;
        self.loading_comparison = true;
        self.right_replace_confirmed = self.bypass_comparison_guard;
        self.loading = Some(ImportTask::start(source, ctx.clone()));
    }
    fn comparison_discard_dialog(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
        if self.pending_comparison.is_none() {
            return;
        }
        let mut discard = false;
        let mut cancel = false;
        egui::Modal::new(egui::Id::new("keep-comparison-measurements")).show(ctx, |ui| {
            ui.heading("Keep your comparison?");
            ui.label("Some comparison changes have not been saved. Return to the comparison and choose Comparison → Save comparison to keep them.");
            ui.horizontal(|ui| {
                if ui.button("Return to comparison").clicked() { cancel = true; }
                if ui.button("Discard and continue").clicked() { discard = true; }
            });
        });
        if cancel {
            self.pending_comparison = None;
        }
        if discard && let Some(action) = self.pending_comparison.take() {
            self.bypass_comparison_guard = true;
            match action {
                ComparisonAction::Mode(mode) => self.set_mode(mode, ctx),
                ComparisonAction::Close => self.close(),
                ComparisonAction::Begin(source) => self.begin(source, ctx),
                ComparisonAction::Select(index) => {
                    if let Some(comparison) = &mut self.comparison {
                        comparison.select_active(index, ctx, frame);
                    }
                    self.export.close();
                }
                ComparisonAction::OpenRight(source) => self.open_comparison_right(source, ctx),
                ComparisonAction::ReturnLabels => self.return_to_labels(),
                ComparisonAction::Quit => {
                    self.allow_comparison_quit = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
            self.bypass_comparison_guard = false;
        }
    }
    pub(super) fn protect_labels_on_quit(&mut self, ctx: &egui::Context) {
        if !ctx.input(|i| i.viewport().close_requested()) {
            return;
        }
        if self
            .comparison
            .as_ref()
            .is_some_and(|comparison| comparison.busy())
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            return;
        }
        let allow = self.allow_comparison_quit;
        if !allow
            && self
                .comparison
                .as_ref()
                .is_some_and(|comparison| comparison.unsaved())
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.pending_comparison = Some(ComparisonAction::Quit);
            return;
        }
        if self.labels.dirty || self.labels.busy() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.pending_quit = true;
            self.labels.discard_question = true;
        } else {
            self.allow_comparison_quit = false;
        }
    }
    fn return_to_labels(&mut self) {
        if !self.bypass_comparison_guard
            && self
                .comparison
                .as_ref()
                .is_some_and(|comparison| comparison.unsaved())
        {
            self.pending_comparison = Some(ComparisonAction::ReturnLabels);
            return;
        }
        if self
            .study
            .as_ref()
            .is_some_and(|study| study.series.get(self.labels.series).is_some())
            && (self.mode != ViewMode::Acquired || self.selected != self.labels.series)
        {
            self.select(self.labels.series);
        }
        self.labels.open = true;
    }
    #[cfg(feature = "updates")]
    pub(super) fn labels_need_saving(&self) -> bool {
        self.labels.dirty
            || self.labels.busy()
            || self
                .comparison
                .as_ref()
                .is_some_and(|comparison| comparison.unsaved() || comparison.busy())
    }
    fn label_discard_dialog(&mut self, ctx: &egui::Context) {
        if self.labels.discard_question {
            let mut open = true;
            let mut dismiss = false;
            crate::dialogs::window("Keep your labels",ctx).open(&mut open).show(ctx,|ui| {
                ui.label("Labels are in memory. Export them to keep your work. Continuing discards the current map; nothing is saved automatically.");
                ui.horizontal(|ui| {
                    if ui.button("Return to labels").clicked(){self.return_to_labels();dismiss=true;}
                    if ui.button("Discard labels and continue").clicked(){self.discard_labels=true;self.labels.cancel();dismiss=true;}
                });
            });
            if !open || dismiss {
                self.labels.discard_question = false;
                if !self.discard_labels {
                    self.allow_comparison_quit = false;
                    self.pending_source = None;
                    self.pending_close = false;
                    self.pending_quit = false;
                }
            }
        }
        if self.discard_labels && !self.labels.busy() {
            self.discard_labels = false;
            self.labels = Default::default();
            if self.pending_quit {
                self.pending_quit = false;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            } else if self.pending_close {
                self.pending_close = false;
                self.close_now();
            } else if let Some(source) = self.pending_source.take() {
                self.begin(source, ctx);
            }
        }
    }
}

#[cfg(test)]
mod label_safety_tests {
    use super::*;
    #[test]
    fn cancelled_import_never_publishes_a_result_that_arrived_before_cancellation() {
        let ctx = egui::Context::default();
        let original = Arc::new(orv_core::study::demo_study());
        let mut workspace = Workspace {
            study: Some(original.clone()),
            ..Default::default()
        };
        workspace.set_mode(ViewMode::Compare, &ctx);
        workspace
            .comparison
            .as_mut()
            .unwrap()
            .add_test_measurement(1);
        let incoming = orv_core::study::demo_study();
        let catalog = crate::series_browser::Catalog::build(&incoming, || false).unwrap();
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        sender
            .send(Ok(Some(crate::worker::ImportedStudy {
                study: incoming,
                catalog,
            })))
            .unwrap();
        workspace.loading = Some(ImportTask {
            demo: None,
            receiver,
            cancel: Arc::new(std::sync::atomic::AtomicBool::new(true)),
            processed: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            checked_bytes: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        });
        workspace.loading_comparison = true;
        workspace.right_replace_confirmed = true;
        workspace.poll(&ctx);
        assert!(workspace.loading.is_none());
        assert!(workspace.comparison_source.is_none());
        assert!(workspace.comparison.as_ref().unwrap().pane_unsaved(1));
        assert!(Arc::ptr_eq(workspace.study.as_ref().unwrap(), &original));
    }
    #[test]
    fn entering_comparison_preserves_source_measurements_and_view() {
        use crate::annotations::{SavedMark, SavedTool};
        let ctx = egui::Context::default();
        let study = Arc::new(orv_core::study::demo_study());
        let mark = SavedMark {
            frame: 2,
            tool: SavedTool::Angle,
            points: vec![[0., 0.], [1., 1.], [2., 0.]],
        };
        let mut workspace = Workspace {
            study: Some(study.clone()),
            ..Default::default()
        };
        workspace.view.slice = 2;
        workspace.view.invert = true;
        workspace.tool = Tool::Angle;
        workspace.annotations = Annotations::restore(&[mark], &study.series[0], || false).unwrap();
        workspace.set_mode(ViewMode::Compare, &ctx);
        assert!(!workspace.annotations.has_marks());
        let comparison = workspace.comparison.as_ref().unwrap();
        assert!(comparison.pane_unsaved(0));
        workspace.set_mode(ViewMode::Acquired, &ctx);
        assert_eq!(workspace.mode, ViewMode::Compare);
        assert!(matches!(
            workspace.pending_comparison,
            Some(ComparisonAction::Mode(ViewMode::Acquired))
        ));
    }
    #[test]
    fn failed_right_import_after_discard_confirmation_keeps_existing_marks() {
        let ctx = egui::Context::default();
        let study = Arc::new(orv_core::study::demo_study());
        let mut workspace = Workspace {
            study: Some(study),
            ..Default::default()
        };
        workspace.set_mode(ViewMode::Compare, &ctx);
        workspace
            .comparison
            .as_mut()
            .unwrap()
            .add_test_measurement(1);
        workspace.bypass_comparison_guard = true;
        workspace.open_comparison_right(Source::Paths(vec![]), &ctx);
        workspace.bypass_comparison_guard = false;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while workspace.loading.is_some() {
            workspace.poll(&ctx);
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(workspace.comparison.as_ref().unwrap().pane_unsaved(1));
    }
    #[test]
    fn comparison_changes_block_mode_close_and_native_quit_until_disposition() {
        let ctx = egui::Context::default();
        let mut workspace = Workspace {
            study: Some(Arc::new(orv_core::study::demo_study())),
            ..Default::default()
        };
        workspace.set_mode(ViewMode::Compare, &ctx);
        workspace
            .comparison
            .as_mut()
            .unwrap()
            .add_test_measurement(0);
        workspace.set_mode(ViewMode::Acquired, &ctx);
        assert_eq!(workspace.mode, ViewMode::Compare);
        assert!(matches!(
            workspace.pending_comparison,
            Some(ComparisonAction::Mode(ViewMode::Acquired))
        ));
        workspace.pending_comparison = None;
        workspace.close();
        assert!(workspace.study.is_some());
        assert!(matches!(
            workspace.pending_comparison,
            Some(ComparisonAction::Close)
        ));
        workspace.pending_comparison = None;
        let mut input = egui::RawInput::default();
        input
            .viewports
            .get_mut(&egui::ViewportId::ROOT)
            .unwrap()
            .events
            .push(egui::ViewportEvent::Close);
        let mut output = ctx.run_ui(input, |ui| workspace.protect_labels_on_quit(ui.ctx()));
        assert!(
            output.viewport_output[&egui::ViewportId::ROOT]
                .commands
                .contains(&egui::ViewportCommand::CancelClose)
        );
        assert!(matches!(
            workspace.pending_comparison,
            Some(ComparisonAction::Quit)
        ));
        output.textures_delta.clear();
    }
    #[test]
    fn comparison_import_keeps_the_original_source_and_opens_the_new_source() {
        let ctx = egui::Context::default();
        let original = Arc::new(orv_core::study::demo_study());
        let mut workspace = Workspace {
            study: Some(original.clone()),
            ..Default::default()
        };
        workspace.set_mode(ViewMode::Compare, &ctx);
        workspace.loading_comparison = true;
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/demos/covid-xray.zip");
        workspace.loading = Some(ImportTask::start(Source::Paths(vec![path]), ctx.clone()));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while workspace.loading.is_some() {
            workspace.poll(&ctx);
            assert!(
                std::time::Instant::now() < deadline,
                "Import did not complete"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(workspace.error.is_none());
        assert!(Arc::ptr_eq(workspace.study.as_ref().unwrap(), &original));
        let comparison = workspace.comparison.as_ref().unwrap();
        let external = workspace.comparison_source.as_ref().unwrap();
        assert!(Arc::ptr_eq(comparison.active_study(), &external.study));
        assert!(!Arc::ptr_eq(&external.study, &original));
        assert_eq!(external.study.series[0].frame_count(), 1);
    }
    #[test]
    fn leaving_comparison_retains_active_external_source_and_protects_original_labels() {
        let ctx = egui::Context::default();
        let original = Arc::new(orv_core::study::demo_study());
        let other = Arc::new(orv_core::study::demo_study());
        let mut workspace = Workspace {
            study: Some(original.clone()),
            ..Default::default()
        };
        workspace.set_mode(ViewMode::Compare, &ctx);
        assert!(
            workspace
                .comparison
                .as_mut()
                .unwrap()
                .open_right(other.clone(), &ctx)
        );
        let catalog = crate::series_browser::Catalog::build(&other, || false).unwrap();
        workspace.comparison_source = Some(ComparisonSource {
            study: other.clone(),
            browser: crate::series_browser::SeriesBrowser::new(catalog, other.clone(), ctx.clone()),
        });
        workspace.labels.dirty = true;
        workspace.set_mode(ViewMode::Acquired, &ctx);
        assert_eq!(workspace.mode, ViewMode::Compare);
        assert!(Arc::ptr_eq(workspace.study.as_ref().unwrap(), &original));
        assert!(workspace.labels.dirty);
        workspace.labels.dirty = false;
        workspace.set_mode(ViewMode::Acquired, &ctx);
        assert_eq!(workspace.mode, ViewMode::Acquired);
        assert!(Arc::ptr_eq(workspace.study.as_ref().unwrap(), &other));
        assert!(workspace.comparison_source.is_none());
    }
    #[test]
    fn returning_to_unsaved_labels_leaves_comparison_and_selects_their_source_series() {
        let ctx = egui::Context::default();
        let mut study = orv_core::study::demo_study();
        study.series.push(study.series[0].clone());
        let mut workspace = Workspace {
            study: Some(Arc::new(study)),
            ..Default::default()
        };
        workspace.labels.dirty = true;
        workspace.labels.series = 0;
        workspace.set_mode(ViewMode::Compare, &ctx);
        workspace.return_to_labels();
        assert_eq!(workspace.mode, ViewMode::Acquired);
        assert_eq!(workspace.selected, 0);
        assert!(workspace.comparison.is_none());
        assert!(workspace.labels.open && workspace.labels.dirty);
        workspace.labels.dirty = false;
        workspace.close();
    }
    #[test]
    fn native_close_request_waits_for_explicit_disposition_of_unsaved_labels() {
        let ctx = egui::Context::default();
        let mut workspace = Workspace::default();
        workspace.labels.dirty = true;
        let mut input = egui::RawInput::default();
        input
            .viewports
            .get_mut(&egui::ViewportId::ROOT)
            .unwrap()
            .events
            .push(egui::ViewportEvent::Close);
        let mut output = ctx.run_ui(input, |ui| workspace.protect_labels_on_quit(ui.ctx()));
        assert!(
            output.viewport_output[&egui::ViewportId::ROOT]
                .commands
                .contains(&egui::ViewportCommand::CancelClose)
        );
        assert!(
            workspace.labels.dirty && workspace.pending_quit && workspace.labels.discard_question
        );
        output.textures_delta.clear();
        workspace.labels.discard_question = false;
        workspace.discard_labels = true;
        let mut output = ctx.run_ui(Default::default(), |ui| {
            workspace.label_discard_dialog(ui.ctx())
        });
        assert!(
            output.viewport_output[&egui::ViewportId::ROOT]
                .commands
                .contains(&egui::ViewportCommand::Close)
        );
        assert!(!workspace.labels.dirty && !workspace.pending_quit);
        output.textures_delta.clear();
    }
    #[test]
    fn unsaved_labels_protect_study_closure_and_replacement() {
        let ctx = egui::Context::default();
        let mut workspace = Workspace {
            study: Some(Arc::new(Study::default())),
            ..Default::default()
        };
        workspace.labels.dirty = true;
        workspace.close();
        assert!(workspace.study.is_some());
        assert!(workspace.labels.dirty);
        assert!(workspace.pending_close);
        workspace.pending_close = false;
        workspace.begin(Source::Files, &ctx);
        assert!(workspace.loading.is_none());
        assert!(matches!(workspace.pending_source, Some(Source::Files)));
        assert!(workspace.labels.discard_question);
    }
}
