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
}

#[derive(Default)]
pub struct Workspace {
    pub(super) export: crate::export::PictureExport,
    pub(super) picture_size: Option<[u32; 2]>,
    pub(super) playback: crate::playback::Playback,
    demo: Option<crate::demos::Demo>,
    browser: crate::series_browser::SeriesBrowser,
    show_demo_credits: bool,
    pub(super) study: Option<Study>,
    pub(super) image_worker: Option<crate::frame_worker::FrameWorker>,
    pub(super) decoded_image: Option<(usize, Arc<Frame>)>,
    pub(super) last_image_view: Option<ViewState>,
    pub(super) decode_error: Option<(usize, LoadError)>,
    pub(super) initialize_window: bool,
    pub(super) selected: usize,
    pub(super) view: ViewState,
    pub(super) tool: Tool,
    pub(super) loading: Option<ImportTask>,
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
    #[cfg(feature = "mcp")]
    pub(super) sharing: crate::sharing::Sharing,
}
impl Workspace {
    pub(crate) fn local_dialog_open(&self) -> bool {
        self.image_info.is_open()
            || self.export.is_open()
            || self.labels.open
            || self.labels.discard_question
            || self.discard_labels
            || self.show_demo_credits
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
        self.clear_gpu = true;
        self.error = None;
        self.selected = 0;
        self.view = ViewState::default();
    }
    fn select(&mut self, index: usize) {
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
        let result = self.loading.as_ref().map(|task| task.receiver.try_recv());
        match result {
            Some(Ok(Ok(Some(study)))) => {
                self.demo = self.loading.as_ref().and_then(|task| task.demo);
                self.loading = None;
                self.labels = Default::default();
                self.browser =
                    crate::series_browser::SeriesBrowser::new(study.catalog, ctx.clone());
                self.study = Some(study.study);
                self.clear_gpu = true;
                self.select(0);
            }
            Some(Ok(Ok(None))) => {
                self.loading = None;
            }
            Some(Ok(Err(error))) => {
                self.loading = None;
                self.error = Some(error.to_string());
            }
            Some(Err(TryRecvError::Disconnected)) => {
                self.loading = None;
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
        let other_modal =
            self.sharing_modal_open() || self.labels.discard_question || self.discard_labels;
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
        if !blocked {
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
                    let ready = self.rendered_view().is_some_and(|(_, ready)| ready);
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
                let previous = self.mode;
                ui.selectable_value(&mut self.mode, ViewMode::Acquired, "2D image");
                ui.selectable_value(&mut self.mode, ViewMode::Mpr, "Linked MPR");
                ui.selectable_value(&mut self.mode, ViewMode::Volume, "3D / MIP");
                ui.selectable_value(&mut self.mode, ViewMode::Oblique, "Oblique");
                if previous != self.mode
                    && let Some(frames) = self
                        .study
                        .as_ref()
                        .and_then(|s| s.series.get(self.selected))
                        .map(|s| Arc::new(s.clone()))
                {
                    self.discard_inactive_views();
                    match self.mode {
                        ViewMode::Mpr if self.mpr.is_none() => {
                            self.mpr = Some(MprSession::start(
                                frames,
                                self.view.window,
                                ui.ctx().clone(),
                            ))
                        }
                        ViewMode::Volume => {
                            self.volume = Some(crate::volume_ui::VolumeSession::start(frames));
                        }
                        ViewMode::Oblique => {
                            self.oblique = Some(crate::oblique_ui::ObliqueSession::start(
                                frames,
                                self.view.window,
                                ui.ctx().clone(),
                            ));
                        }
                        _ => (),
                    }
                }
            }
            #[cfg(feature = "mcp")]
            self.agent_toolbar(ui);
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
            .show(ui, |ui| self.sidebar(ui));
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
                    }
                } else {
                    self.empty(ui, seconds, reduce_motion);
                }
            });
    }
    fn sidebar(&mut self, ui: &mut egui::Ui) {
        theme::eyebrow(ui, "LOCAL SERIES");
        ui.add_space(12.0);
        if let Some(study) = &self.study {
            if let Some(demo) = self.demo {
                theme::eyebrow(ui, "PUBLIC RESEARCH DEMO");
                ui.label(demo.title());
                ui.small(demo.credit());
                if ui.small_button("Source credits").clicked() {
                    self.show_demo_credits = true;
                }
                ui.add_space(12.0);
            }
            if study.synthetic {
                theme::eyebrow(ui, "SYNTHETIC DEMO · NO PATIENT DATA");
            }
            let select = self.browser.ui(ui, self.selected);
            ui.add_space(18.0);
            if study.skipped_files > 0 {
                ui.label(
                    RichText::new(format!(
                        "{} unsupported or non-image files skipped",
                        study.skipped_files
                    ))
                    .small()
                    .color(theme::SILVER),
                );
            }
            if let Some(index) = select {
                self.select(index);
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
    pub(super) fn protect_labels_on_quit(&mut self, ctx: &egui::Context) {
        if ctx.input(|i| i.viewport().close_requested())
            && (self.labels.dirty || self.labels.busy())
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.pending_quit = true;
            self.labels.discard_question = true;
        }
    }
    #[cfg(feature = "updates")]
    pub(super) fn labels_need_saving(&self) -> bool {
        self.labels.dirty || self.labels.busy()
    }
    fn label_discard_dialog(&mut self, ctx: &egui::Context) {
        if self.labels.discard_question {
            let mut open = true;
            let mut dismiss = false;
            crate::dialogs::window("Keep your labels",ctx).open(&mut open).show(ctx,|ui| {
                ui.label("Labels are in memory. Export them to keep your work. Continuing discards the current map; nothing is saved automatically.");
                ui.horizontal(|ui| {
                    if ui.button("Return to labels").clicked(){self.labels.open=true;dismiss=true;}
                    if ui.button("Discard labels and continue").clicked(){self.discard_labels=true;self.labels.cancel();dismiss=true;}
                });
            });
            if !open || dismiss {
                self.labels.discard_question = false;
                if !self.discard_labels {
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
    fn unsaved_labels_protect_study_closure_and_replacement() {
        let ctx = egui::Context::default();
        let mut workspace = Workspace {
            study: Some(Study::default()),
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
