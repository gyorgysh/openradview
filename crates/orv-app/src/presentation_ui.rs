// SPDX-License-Identifier: GPL-3.0-or-later
//! Explicitly chosen presentation state for the selected series, applied whole
//! to the 2D view with a visible return to the image's own presentation.
use crate::jobs::Worker;
use eframe::egui;
use orv_core::{
    DisplayTransform, ViewState, WindowLevel,
    index::{PresentationState, read_presentation_state},
    study::Series,
};
use std::sync::Arc;

type Outcome = Option<PresentationState>;

fn read(series: Arc<Series>, cancel: &dyn Fn() -> bool) -> Result<Outcome, String> {
    if cancel() {
        return Ok(None);
    }
    let Some(path) = rfd::FileDialog::new()
        .set_title("Apply a presentation state to the selected DICOM series")
        .add_filter("DICOM presentation state", &["dcm", "pre"])
        .pick_file()
    else {
        return Ok(None);
    };
    read_presentation_state(&path, &series, cancel)
        .map(Some)
        .map_err(|error| error.to_string())
}

struct Active {
    state: PresentationState,
    before: (WindowLevel, bool, DisplayTransform),
}

#[derive(Default)]
pub struct PresentationUi {
    worker: Option<Worker<Arc<Series>, Outcome>>,
    pending: Option<u64>,
    active: Option<Active>,
    pub message: Option<String>,
}
impl PresentationUi {
    /// Toolbar controls. Returns true when the view changed.
    pub fn controls(
        &mut self,
        ui: &mut egui::Ui,
        series: &Series,
        view: &mut ViewState,
        monochrome1: bool,
    ) -> bool {
        let mut changed = self.poll(view, monochrome1);
        if self.active.is_some() {
            crate::toolbar::label(
                ui,
                egui::RichText::new({
                    let count = self.active.as_ref().map_or(0, |a| a.state.images.len());
                    format!(
                        "Presentation state · {count} image{}",
                        if count == 1 { "" } else { "s" }
                    )
                })
                .small(),
            );
            if ui
                .button("As acquired")
                .on_hover_text("Return to this image's own window, inversion and orientation.")
                .clicked()
            {
                changed |= self.restore(view);
                self.message = None;
            }
        } else if ui
            .add_enabled(
                self.pending.is_none() && series.source.is_some(),
                egui::Button::new("Presentation state…"),
            )
            .on_hover_text("Apply a grayscale softcopy presentation state for this series: window, inversion, rotation and flip. States with annotations, shutters or other features are refused whole.")
            .clicked()
        {
            self.start(Arc::new(series.clone()), ui.ctx());
        }
        if let Some(message) = &self.message {
            crate::toolbar::label(ui, egui::RichText::new(message.as_str()).small());
        }
        changed
    }
    fn start(&mut self, series: Arc<Series>, ctx: &egui::Context) {
        if self.worker.is_none() {
            match Worker::start(ctx.clone(), "orv-presentation-state", read) {
                Ok(worker) => self.worker = Some(worker),
                Err(error) => {
                    self.message = Some(error);
                    return;
                }
            }
        }
        if let Some(worker) = &self.worker {
            self.pending = Some(worker.submit(series));
            self.message = None;
        }
    }
    fn poll(&mut self, view: &mut ViewState, monochrome1: bool) -> bool {
        let (Some(worker), Some(expected)) = (&self.worker, self.pending) else {
            return false;
        };
        let Some((generation, result)) = worker.take() else {
            return false;
        };
        if generation != expected {
            return false;
        }
        self.pending = None;
        match result {
            Ok(Some(state)) => {
                let before = (view.window, view.invert, view.transform);
                view.window = state.window;
                // The presentation LUT replaces the photometric inversion.
                view.invert = state.inverse ^ monochrome1;
                view.transform = state.transform;
                view.zoom = 1.;
                view.pan = [0.; 2];
                if !state.images.contains(&(view.slice as usize)) {
                    view.slice = state.images[0] as u32;
                }
                self.active = Some(Active { state, before });
                self.message = None;
                true
            }
            Ok(None) => false,
            Err(message) => {
                self.message = Some(message);
                false
            }
        }
    }
    fn restore(&mut self, view: &mut ViewState) -> bool {
        let Some(active) = self.active.take() else {
            return false;
        };
        (view.window, view.invert, view.transform) = active.before;
        true
    }
    /// Leave the state when browsing to an image it does not reference.
    pub fn follow(&mut self, view: &mut ViewState) -> bool {
        let outside = self
            .active
            .as_ref()
            .is_some_and(|active| !active.state.images.contains(&(view.slice as usize)));
        if outside {
            self.restore(view);
            self.message = Some(format!(
                "Returned to the image presentation: image {} is not in the presentation state.",
                view.slice + 1
            ));
        }
        outside
    }
}
