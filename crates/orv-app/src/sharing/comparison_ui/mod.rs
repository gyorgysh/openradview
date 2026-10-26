// SPDX-License-Identifier: GPL-3.0-or-later
//! Native comparison review and results. Transport implementation stays in MCP.
mod dialog;
mod results;
#[cfg(all(test, feature = "gpu-tests"))]
mod tests;

use super::{
    ReadyPreview, ViewSpec, next_review_id,
    preview::{Input, PreviewWorker},
};
use eframe::egui;
use orv_agents::{Comparison, Consent, ReviewedSnapshot};
use orv_core::study::Series;
use orv_mcp::http::{LocalServer, ServerStatus};
use orv_render::snapshot::{Snapshot, SnapshotManifest};
use std::sync::Arc;

struct Draft {
    id: u64,
    worker: PreviewWorker,
    preview: Option<ReadyPreview>,
    recipients: Vec<String>,
    reviewed: bool,
    risk: bool,
    zoom: bool,
    error: Option<&'static str>,
}
impl Draft {
    fn consent(&self) -> Consent {
        Consent {
            recipients: self.recipients.clone(),
            preview_reviewed: self.reviewed,
            onward_sharing_accepted: self.risk,
        }
    }
    fn can_allow(&self) -> bool {
        self.preview.is_some() && self.error.is_none() && self.consent().validate().is_ok()
    }
}
struct Live {
    id: u64,
    comparison: Comparison,
    servers: Vec<LocalServer>,
    texture: egui::TextureHandle,
    manifest: SnapshotManifest,
    json: String,
    open: bool,
}
#[derive(Default)]
pub(super) struct ComparisonUi {
    draft: Option<Draft>,
    live: Option<Live>,
    pub(super) notice: Option<String>,
}
impl ComparisonUi {
    pub(super) fn is_active(&self) -> bool {
        self.live
            .as_ref()
            .is_some_and(|l| l.comparison.recipients().iter().any(|r| !r.is_revoked()))
    }
    pub(super) fn modal_open(&self) -> bool {
        self.draft.is_some()
    }
    pub(super) fn revoke(&mut self) {
        self.draft = None;
        self.live = None;
    }
    pub(super) fn show_results(&mut self) {
        if let Some(live) = &mut self.live {
            live.open = true;
        }
    }
    pub(super) fn open(
        &mut self,
        index: u32,
        series: Arc<Series>,
        spec: ViewSpec,
        ctx: &egui::Context,
    ) {
        self.revoke();
        self.notice = None;
        let size = spec.preferred_size.unwrap_or([512, 512]);
        self.draft = Some(Draft {
            id: next_review_id(),
            worker: PreviewWorker::start(Input::Source(index, series), spec, size, ctx.clone()),
            preview: None,
            recipients: vec![String::new(), String::new()],
            reviewed: false,
            risk: false,
            zoom: false,
            error: None,
        });
    }
    pub(super) fn poll(&mut self, ctx: &egui::Context) {
        if let Some(live) = &self.live
            && let Some(message) = live.servers.iter().find_map(|s| match s.status() {
                ServerStatus::Failed(e) => Some(e),
                ServerStatus::Stopped => Some("Comparison access ended."),
                _ => None,
            })
        {
            self.notice = Some(message.into());
            self.revoke();
        }
        if let Some(draft) = &mut self.draft
            && let Some(result) = draft.worker.take()
        {
            match result {
                Ok(data) => {
                    let rgba = &data.rgba;
                    let texture = ctx.load_texture(
                        "comparison-preview",
                        egui::ColorImage::from_rgba_unmultiplied(
                            [rgba.width() as usize, rgba.height() as usize],
                            rgba.as_raw(),
                        ),
                        egui::TextureOptions::LINEAR,
                    );
                    match serde_json::to_string_pretty(&data.snapshot.manifest) {
                        Ok(json) => {
                            draft.preview = Some(ReadyPreview {
                                data,
                                texture,
                                json,
                            })
                        }
                        Err(_) => draft.error = Some("Could not encode the comparison manifest"),
                    }
                }
                Err(error) => draft.error = Some(error),
            }
        }
    }
    fn allow(&mut self, ctx: &egui::Context) -> Result<(), &'static str> {
        let draft = self.draft.as_ref().ok_or("No comparison review is open")?;
        if !draft.can_allow() {
            return Err("Review the image, metadata and all recipients first");
        }
        let preview = draft
            .preview
            .as_ref()
            .ok_or("The comparison preview is not ready")?;
        let manifest = preview.data.snapshot.manifest.clone();
        let snapshot = ReviewedSnapshot::new(Snapshot {
            png: preview.data.snapshot.png.clone(),
            manifest: manifest.clone(),
        })?;
        let comparison = Comparison::grant(snapshot, draft.consent())?;
        let mut servers = Vec::new();
        for recipient in comparison.recipients() {
            let repaint = ctx.clone();
            servers.push(LocalServer::start_comparison(
                recipient.clone(),
                Arc::new(move || repaint.request_repaint()),
            )?);
        }
        self.live = Some(Live {
            id: draft.id,
            comparison,
            servers,
            texture: preview.texture.clone(),
            manifest,
            json: preview.json.clone(),
            open: true,
        });
        // Drops the worker, decoded review image and PreparedSeries. Only the
        // frozen PNG, allowlisted manifest and displayed texture survive.
        self.draft = None;
        Ok(())
    }
}
impl Drop for ComparisonUi {
    fn drop(&mut self) {
        self.revoke();
    }
}
