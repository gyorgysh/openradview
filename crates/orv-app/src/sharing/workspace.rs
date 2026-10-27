// SPDX-License-Identifier: GPL-3.0-or-later
use crate::{
    mpr::MprSession,
    workspace::{ViewMode, Workspace},
};
use eframe::egui;
use orv_core::{
    Plane,
    scene::{Reconstruction, SceneState},
};
use std::sync::Arc;

impl Workspace {
    pub(crate) fn sharing_before_ui(&mut self, ctx: &egui::Context, external_dialog_open: bool) {
        self.sharing.poll(ctx);
        // Leave unseen requests in the bounded broker while the user reviews a
        // local dialog. Closing it triggers another frame; no polling is added.
        if !external_dialog_open
            && !self.local_dialog_open()
            && let Some(request) = self.sharing.broker.as_ref().and_then(|b| b.take_request())
        {
            if self.mode == ViewMode::Compare
                || self.sharing.comparison.is_active()
                || self.sharing.modal_open()
                || self.loading.is_some()
            {
                if let Some(broker) = &self.sharing.broker {
                    broker.deny(request.id);
                }
            } else if let Some((spec, true)) = self.rendered_view()
                && let Some(series) = self
                    .study
                    .as_ref()
                    .and_then(|s| s.series.get(self.selected))
            {
                self.sharing
                    .open(self.selected as u32, Arc::new(series.clone()), spec, ctx);
                if let Some(draft) = &mut self.sharing.draft {
                    draft.request_id = Some(request.id);
                    draft.recipient = request.recipient;
                    draft.transport = super::Transport::Stdio;
                }
                self.sharing.panel_open = false;
            } else if let Some(broker) = &self.sharing.broker {
                broker.deny(request.id);
                self.sharing.notice =
                    Some("Open a series before approving an agent connection.".into());
            }
        }
        if let Some(scene) = self.sharing.take_requested_view() {
            self.apply_shared_scene(&scene, ctx);
        }
    }
    pub(crate) fn sharing_after_ui(&mut self, ctx: &egui::Context) {
        if let Some((spec, ready)) = self.rendered_view() {
            self.sharing.publish(spec, ready);
        }
        if !self.sharing.modal_open() {
            self.agent_panel(ctx);
        }
        self.sharing.ui(ctx);
    }
    pub(crate) fn agent_toolbar(&mut self, ui: &mut egui::Ui) {
        if ui
            .add_enabled(
                self.mode != ViewMode::Compare,
                egui::Button::new(if self.sharing.is_active() {
                    "Agents · connected"
                } else {
                    "Agents"
                }),
            )
            .on_hover_text("Sharing requires a single series view and a separate Allow review.")
            .clicked()
        {
            self.sharing.panel_open = true;
        }
    }
    fn agent_panel(&mut self, ctx: &egui::Context) {
        let (manual, compare) = self.sharing.panel(ctx);
        if (manual || compare)
            && !self.sharing.is_active()
            && self.sharing.draft.is_none()
            && self.loading.is_none()
            && let Some((spec, true)) = self.rendered_view()
            && let Some(series) = self
                .study
                .as_ref()
                .and_then(|s| s.series.get(self.selected))
        {
            if compare {
                self.sharing.comparison.open(
                    self.selected as u32,
                    Arc::new(series.clone()),
                    spec,
                    ctx,
                );
            } else {
                self.sharing
                    .open(self.selected as u32, Arc::new(series.clone()), spec, ctx);
            }
        } else if manual || compare {
            self.sharing.notice =
                Some("Open a series and wait for its image to finish loading first.".into());
        }
    }
    pub(super) fn apply_shared_scene(&mut self, scene: &SceneState, ctx: &egui::Context) {
        if self.mode == ViewMode::Compare {
            self.sharing.revoke();
            return;
        }
        self.pause_playback();
        if scene.validate().is_err() {
            self.sharing.notice = Some("The requested view is invalid".into());
            return;
        }
        if self.selected != scene.series_index as usize {
            self.sharing.revoke();
            return;
        }
        let mode = match &scene.reconstruction {
            Some(Reconstruction::Oblique(_) | Reconstruction::Slab(_)) => ViewMode::Oblique,
            Some(Reconstruction::Volume(_)) => ViewMode::Volume,
            None if scene.view.plane == Plane::Acquired => ViewMode::Acquired,
            None => ViewMode::Mpr,
        };
        if self.mode != mode {
            self.mode = mode;
            self.discard_inactive_views();
        }
        if mode == ViewMode::Volume || mode == ViewMode::Oblique {
            let Some(frames) = self
                .study
                .as_ref()
                .and_then(|s| s.series.get(self.selected))
                .map(|s| Arc::new(s.clone()))
            else {
                return;
            };
            if mode == ViewMode::Volume {
                let volume = self
                    .volume
                    .get_or_insert_with(|| crate::volume_ui::VolumeSession::start(frames));
                volume.apply_shared_view(scene);
            } else {
                let oblique = self.oblique.get_or_insert_with(|| {
                    crate::oblique_ui::ObliqueSession::start(frames, scene.view.window, ctx.clone())
                });
                oblique.apply_shared_view(scene);
            }
        } else if mode == ViewMode::Acquired {
            self.view = scene.view.clone();
            self.annotations.clear();
            self.scroll = 0.0;
        } else {
            if self.mpr.is_none()
                && let Some(series) = self
                    .study
                    .as_ref()
                    .and_then(|s| s.series.get(self.selected))
            {
                self.mpr = Some(MprSession::start(
                    Arc::new(series.clone()),
                    scene.view.window,
                    ctx.clone(),
                ));
            }
            if let Some(mpr) = &mut self.mpr {
                mpr.apply_shared_view(scene);
            }
        }
    }
}
