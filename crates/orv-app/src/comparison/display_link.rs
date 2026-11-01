// SPDX-License-Identifier: GPL-3.0-or-later
//! Latest user actions wait for exact completed slices, without retaining pixels.
use super::Pane;
use eframe::egui;
use orv_core::{
    ViewState,
    linked_display::{self, DisplayLinks, LinkError, PhysicalDisplay},
    study::Series,
};
use std::sync::{Arc, Weak};

#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct Selection {
    pan: bool,
    zoom: bool,
    window: bool,
}
#[derive(Clone, Copy, Default)]
struct Mask {
    display: DisplayLinks,
    window: bool,
}
impl Mask {
    fn any(self) -> bool {
        self.display.pan || self.display.zoom || self.window
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Status {
    #[default]
    Off,
    Pending,
    Applied,
    Cancelled,
    Failed(LinkError),
}
impl Status {
    fn text(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Pending => "waiting for both requested images",
            Self::Applied => "last action applied",
            Self::Cancelled => "pending action cancelled by changed controls or view",
            Self::Failed(LinkError::Geometry) => "needs valid thin-image geometry",
            Self::Failed(LinkError::Reference) => "needs matching known spatial references",
            Self::Failed(LinkError::Orientation) => "needs parallel image planes",
            Self::Failed(LinkError::View) => "completed image or viewport unavailable",
            Self::Failed(LinkError::Limits) => "proposed view exceeds pan/zoom limits",
            Self::Failed(LinkError::Representation) => {
                "needs compatible scalar mappings and known units"
            }
        }
    }
}
struct Pending {
    generation: u64,
    from: usize,
    selection: Selection,
    sources: [Weak<Series>; 2],
    views: [ViewState; 2],
    aspects: [Option<f64>; 2],
    mask: Mask,
}
impl Pending {
    fn context_matches(&self, panes: &[Pane; 2], active: usize, selection: Selection) -> bool {
        self.from == active
            && self.selection == selection
            && panes.iter().enumerate().all(|(index, pane)| {
                self.sources[index].ptr_eq(&Arc::downgrade(&pane.source))
                    && self.aspects[index] == pane.canvas_aspect
            })
            && self.views[1 - active] == panes[1 - active].view
    }
    fn matches(&self, panes: &[Pane; 2], active: usize, selection: Selection) -> bool {
        self.context_matches(panes, active, selection) && self.views[active] == panes[active].view
    }
}
pub(super) struct Input<'a> {
    pub panes: &'a [Pane; 2],
    pub before: &'a [ViewState; 2],
    pub before_aspects: [Option<f64>; 2],
    pub active: usize,
    pub previous_active: usize,
    pub reconfigured: bool,
    pub enabled: bool,
}
pub(super) struct Update {
    pub view: Option<(usize, ViewState)>,
    pub status_changed: bool,
}
#[derive(Default)]
pub(super) struct Group {
    selection: Selection,
    previous_selection: Selection,
    pending: Option<Pending>,
    generation: u64,
    display_status: Status,
    window_status: Status,
}
impl Group {
    pub fn controls(&mut self, ui: &mut egui::Ui, ready: bool) -> bool {
        let before = self.selection;
        ui.add_enabled_ui(ready, |ui| {
            ui.checkbox(&mut self.selection.pan, "Link physical pan")
                .on_hover_text("Link the physical centre of parallel thin images with the same spatial reference. No registration is performed.");
            ui.checkbox(&mut self.selection.zoom, "Link physical zoom")
                .on_hover_text("Match the vertical field of view in millimetres; each pane keeps its own rotation and flips.");
            ui.checkbox(&mut self.selection.window, "Link scalar window")
                .on_hover_text("Link the selected window/level choice only when scalar units, representation and mappings match. Inversion remains per pane.");
        });
        before != self.selection
    }
    pub fn status(&self, ui: &mut egui::Ui) {
        let text = format!(
            "Physical display: {}. Scalar window: {}.",
            self.display_status.text(),
            self.window_status.text()
        );
        let height = ui.text_style_height(&egui::TextStyle::Small) * 2. + 4.;
        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), height),
            egui::Sense::hover(),
        );
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .id_salt("display-link-status")
                .max_rect(rect),
        );
        child.set_clip_rect(ui.clip_rect().intersect(rect));
        let response =
            child.add(egui::Label::new(egui::RichText::new(&text).small().weak()).wrap());
        ui.ctx().accesskit_node_builder(response.id, |node| {
            node.set_live(egui::accesskit::Live::Polite)
        });
        response.on_hover_text(text);
    }
    fn cancel(&mut self) {
        if let Some(pending) = self.pending.take() {
            if pending.mask.display.pan || pending.mask.display.zoom {
                self.display_status = Status::Cancelled;
            }
            if pending.mask.window {
                self.window_status = Status::Cancelled;
            }
        }
    }
    pub fn update(&mut self, input: Input<'_>) -> Update {
        let before = (self.display_status, self.window_status);
        let view = self.update_inner(input);
        Update {
            view,
            status_changed: before != (self.display_status, self.window_status),
        }
    }
    fn update_inner(&mut self, input: Input<'_>) -> Option<(usize, ViewState)> {
        let Input {
            panes,
            before,
            before_aspects,
            active,
            previous_active,
            reconfigured,
            enabled,
        } = input;
        let previous_selection = std::mem::replace(&mut self.previous_selection, self.selection);
        if !enabled {
            self.cancel();
            return None;
        }
        if active != previous_active || reconfigured {
            self.cancel();
        }
        if reconfigured {
            if !self.selection.pan && !self.selection.zoom {
                self.display_status = Status::Off;
            }
            if !self.selection.window {
                self.window_status = Status::Off;
            }
        }
        let source = panes.get(active)?;
        let old = before.get(active)?;
        let rotated = source.view.transform != old.transform;
        let zoomed = source.view.zoom != old.zoom;
        let resized = panes
            .iter()
            .enumerate()
            .any(|(index, pane)| before_aspects[index] != pane.canvas_aspect);
        if resized {
            self.cancel();
        }
        let mut mask = Mask {
            display: DisplayLinks {
                pan: self.selection.pan
                    && (reconfigured && !previous_selection.pan
                        || rotated
                        || zoomed
                        || resized
                        || source.view.pan != old.pan),
                zoom: self.selection.zoom
                    && (reconfigured && !previous_selection.zoom || rotated || zoomed || resized),
            },
            window: self.selection.window
                && (reconfigured && !previous_selection.window || source.view.window != old.window),
        };
        if mask.any() {
            if let Some(previous) = self
                .pending
                .as_ref()
                .filter(|action| action.context_matches(panes, active, self.selection))
            {
                mask.display.pan |= previous.mask.display.pan;
                mask.display.zoom |= previous.mask.display.zoom;
                mask.window |= previous.mask.window;
            }
            self.cancel();
            self.generation = self.generation.wrapping_add(1);
            self.pending = Some(Pending {
                generation: self.generation,
                from: active,
                selection: self.selection,
                sources: panes.each_ref().map(|pane| Arc::downgrade(&pane.source)),
                views: panes.each_ref().map(|pane| pane.view.clone()),
                aspects: panes.each_ref().map(|pane| pane.canvas_aspect),
                mask,
            });
            if mask.display.pan || mask.display.zoom {
                self.display_status = Status::Pending;
            }
            if mask.window {
                self.window_status = Status::Pending;
            }
        }
        let action = self.pending.as_ref()?;
        if action.generation != self.generation || !action.matches(panes, active, self.selection) {
            self.cancel();
            return None;
        }
        if panes.iter().any(|pane| {
            pane.image
                .as_ref()
                .is_none_or(|(index, _)| *index != pane.view.slice as usize)
        }) {
            if panes
                .iter()
                .any(|pane| pane.error_index == Some(pane.view.slice as usize))
            {
                let action = self.pending.take()?;
                if action.mask.display.pan || action.mask.display.zoom {
                    self.display_status = Status::Failed(LinkError::View);
                }
                if action.mask.window {
                    self.window_status = Status::Failed(LinkError::View);
                }
            }
            return None;
        }
        // Taking the action before calculating guarantees a forwarded view never
        // becomes another action on the next pass.
        let action = self.pending.take()?;
        let target = 1 - active;
        let source = panes[active].image.as_ref()?.1.as_ref();
        let destination = panes[target].image.as_ref()?.1.as_ref();
        let mut candidate = panes[target].view.clone();
        if action.mask.display.pan || action.mask.display.zoom {
            let result = action.aspects[active]
                .zip(action.aspects[target])
                .ok_or(LinkError::View)
                .and_then(|(a, b)| {
                    PhysicalDisplay::capture(source, &action.views[active], a).and_then(|display| {
                        display.apply(destination, &candidate, b, action.mask.display)
                    })
                });
            match result {
                Ok(view) => {
                    candidate = view;
                    self.display_status = Status::Applied;
                }
                Err(error) => self.display_status = Status::Failed(error),
            }
        }
        if action.mask.window {
            match linked_display::scalar_window(source, destination, action.views[active].window) {
                Ok(window) => {
                    candidate.window = window;
                    self.window_status = Status::Applied;
                }
                Err(error) => self.window_status = Status::Failed(error),
            }
        }
        (candidate != panes[target].view).then_some((target, candidate))
    }
}

#[cfg(test)]
mod tests;
