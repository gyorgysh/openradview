// SPDX-License-Identifier: GPL-3.0-or-later
//! Desktop ownership of completed 2D frames. No file I/O runs here.
use crate::workspace::{ViewMode, Workspace};
use eframe::egui;
use orv_core::{ViewState, study::Frame};
use std::sync::Arc;
impl Workspace {
    pub(crate) fn current_image(&self) -> Option<(usize, Arc<Frame>)> {
        let series = self.study.as_ref()?.series.get(self.selected)?;
        if series.source.is_some() {
            self.decoded_image.clone()
        } else {
            series
                .frames
                .get(self.view.slice as usize)
                .cloned()
                .map(|f| (self.view.slice as usize, f))
        }
    }
    pub(crate) fn request_image(&mut self, ctx: &egui::Context) {
        if self.mode != ViewMode::Acquired {
            self.image_worker = None;
            return;
        }
        let Some(series) = self
            .study
            .as_ref()
            .and_then(|s| s.series.get(self.selected))
            .filter(|s| s.source.is_some())
        else {
            return;
        };
        if self.image_worker.is_none() {
            self.image_worker = Some(crate::frame_worker::FrameWorker::start(
                Arc::new(series.clone()),
                self.view.slice as usize,
                ctx.clone(),
            ));
        }
        let Some(worker) = &mut self.image_worker else {
            return;
        };
        worker.request(self.view.slice as usize);
    }
    pub(crate) fn poll_image(&mut self, ctx: &egui::Context) {
        self.request_image(ctx);
        let Some(worker) = &mut self.image_worker else {
            return;
        };
        if let Some(result) = worker.take() {
            match result.image {
                Ok(image) => {
                    if image.rgba.is_some()
                        || self
                            .decoded_image
                            .as_ref()
                            .is_some_and(|(_, previous)| previous.rgba.is_some())
                        || (self.initialize_window
                            && self.view.window == ViewState::default().window)
                    {
                        self.view.window = image.default_window;
                    }
                    self.initialize_window = false;
                    self.decoded_image = Some((result.index, image));
                    self.decode_error = None;
                }
                Err(e) => self.decode_error = Some((result.index, e)),
            }
        }
    }
}
