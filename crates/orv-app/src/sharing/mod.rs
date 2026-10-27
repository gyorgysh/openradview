// SPDX-License-Identifier: GPL-3.0-or-later
//! Desktop-owned, ephemeral consent. No network implementation lives in the UI.
mod client_config;
mod comparison_ui;
mod dialog;
mod panel;
mod preview;
#[cfg(all(test, feature = "gpu-tests"))]
mod tests;
mod workspace;

use eframe::egui;
use orv_core::{scene::SceneState, study::Series};
use orv_mcp::{
    catalog::{PreparedSeries, SeriesDescription},
    consent::{Consent, DataScope, Session, ToolScopes},
    http::{LocalServer, ServerStatus},
};
use preview::{Input, Preview, PreviewWorker};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

fn next_review_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

pub(super) use crate::rendered::ViewSpec;
impl Sharing {
    pub(crate) fn acquired_side(&self) -> u32 {
        self.live.as_ref().map_or(512, |live| live.acquired_side)
    }
}

impl ViewSpec {
    fn scene(&self, source: &SeriesDescription, size: [u32; 2]) -> SceneState {
        SceneState {
            schema_version: if self.reconstruction.is_some() { 2 } else { 1 },
            source_sha256: source.source_sha256.clone(),
            series_index: source.series_index,
            view: self.view.clone(),
            reconstruction: self.reconstruction.clone(),
            crosshair_mm: self.point,
            reformat_side: self.reformat_side,
            output_size: size,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Default)]
enum Transport {
    #[default]
    Http,
    Stdio,
}
impl Transport {
    fn label(self) -> &'static str {
        match self {
            Self::Http => "Local HTTP",
            Self::Stdio => "Stdio bridge via local HTTP",
        }
    }
}

struct ReadyPreview {
    data: Preview,
    texture: egui::TextureHandle,
    json: String,
}
struct Draft {
    request_id: Option<u64>,
    review_id: u64,
    worker: PreviewWorker,
    preview: Option<ReadyPreview>,
    error: Option<&'static str>,
    spec: ViewSpec,
    initial_spec: ViewSpec,
    size: [u32; 2],
    recipient: String,
    transport: Transport,
    tools: ToolScopes,
    full_series: bool,
    reviewed: bool,
    risk: bool,
    existing: bool,
    browser_slice: u32,
    zoom: bool,
}
impl Draft {
    fn can_confirm(&self) -> bool {
        self.preview.is_some()
            && self.error.is_none()
            && !self.recipient.trim().is_empty()
            && self.recipient.len() <= 128
            && !self.recipient.chars().any(char::is_control)
            && (self.tools.view_control
                || self.tools.snapshots
                || self.tools.metadata
                || self.tools.measurements)
    }
    fn request(&mut self, spec: ViewSpec) {
        self.spec = spec;
        self.preview = None;
        self.reviewed = false;
        self.error = None;
        self.worker.request(self.spec.clone(), self.size);
    }
    fn poll(&mut self, ctx: &egui::Context) {
        if let Some(result) = self.worker.take() {
            match result {
                Ok(data) => {
                    let rgba = &data.rgba;
                    let texture = ctx.load_texture(
                        "shared-image-preview",
                        egui::ColorImage::from_rgba_unmultiplied(
                            [rgba.width() as usize, rgba.height() as usize],
                            rgba.as_raw(),
                        ),
                        egui::TextureOptions::LINEAR,
                    );
                    let json = match serde_json::to_string_pretty(&data.snapshot.manifest) {
                        Ok(json) => json,
                        Err(_) => {
                            self.error = Some("Could not prepare the metadata review");
                            return;
                        }
                    };
                    self.preview = Some(ReadyPreview {
                        data,
                        texture,
                        json,
                    });
                }
                Err(error) => self.error = Some(error),
            }
        }
    }
    fn can_allow(&self) -> bool {
        if self.existing && self.full_series {
            return self.preview.is_some() && self.error.is_none();
        }
        self.preview.is_some()
            && self.error.is_none()
            && self.reviewed
            && self.risk
            && !self.recipient.trim().is_empty()
            && self.recipient.len() <= 128
            && !self.recipient.chars().any(char::is_control)
            && (self.tools.view_control
                || self.tools.snapshots
                || self.tools.metadata
                || self.tools.measurements)
    }
}

struct Live {
    request_id: Option<u64>,
    session: Session,
    server: LocalServer,
    source: Arc<PreparedSeries>,
    recipient: String,
    transport: Transport,
    tools: ToolScopes,
    full_series: bool,
    output_size: [u32; 2],
    acquired_side: u32,
    last_published: Option<SceneState>,
}

#[derive(Default)]
pub(super) struct Sharing {
    broker: Option<orv_mcp::broker::RequestBroker>,
    broker_attempted: bool,
    requests_disabled: bool,
    panel_open: bool,
    draft: Option<Draft>,
    live: Option<Live>,
    connection_open: bool,
    notice: Option<String>,
    comparison: comparison_ui::ComparisonUi,
}
impl Sharing {
    pub fn is_active(&self) -> bool {
        self.live.as_ref().is_some_and(|l| !l.session.is_revoked()) || self.comparison.is_active()
    }
    pub fn modal_open(&self) -> bool {
        self.draft.is_some() || self.comparison.modal_open()
    }
    pub fn revoke(&mut self) {
        if let Some(broker) = &self.broker {
            broker.cancel_requests();
        }
        self.comparison.revoke();
        self.draft = None;
        if let Some(live) = self.live.take() {
            live.session.revoke();
        }
        self.connection_open = false;
    }
    pub(crate) fn enter_local_comparison(&mut self) {
        self.revoke();
        self.panel_open = false;
    }
    fn open(&mut self, index: u32, series: Arc<Series>, spec: ViewSpec, ctx: &egui::Context) {
        self.notice = None;
        let size = spec.preferred_size.unwrap_or([512, 512]);
        self.draft = Some(Draft {
            request_id: None,
            review_id: next_review_id(),
            worker: PreviewWorker::start(
                Input::Source(index, series),
                spec.clone(),
                size,
                ctx.clone(),
            ),
            preview: None,
            error: None,
            initial_spec: spec.clone(),
            spec: spec.clone(),
            size,
            recipient: String::new(),
            transport: Transport::Http,
            tools: ToolScopes {
                view_control: true,
                snapshots: true,
                metadata: true,
                measurements: true,
            },
            full_series: true,
            reviewed: false,
            risk: false,
            existing: false,
            browser_slice: if spec.view.plane == orv_core::Plane::Acquired {
                spec.view.slice
            } else {
                0
            },
            zoom: false,
        });
    }
    fn review(&mut self, ctx: &egui::Context) {
        let Some(live) = &self.live else {
            return;
        };
        let Ok(scene) = live.session.current() else {
            return;
        };
        let spec = ViewSpec {
            view: scene.view,
            point: scene.crosshair_mm,
            reformat_side: scene.reformat_side,
            reconstruction: scene.reconstruction,
            preferred_size: None,
        };
        self.draft = Some(Draft {
            request_id: None,
            review_id: next_review_id(),
            worker: PreviewWorker::start(
                Input::Prepared(live.source.clone()),
                spec.clone(),
                scene.output_size,
                ctx.clone(),
            ),
            preview: None,
            error: None,
            initial_spec: spec.clone(),
            spec: spec.clone(),
            size: scene.output_size,
            recipient: live.recipient.clone(),
            transport: live.transport,
            tools: live.tools,
            full_series: live.full_series,
            reviewed: false,
            risk: false,
            existing: true,
            browser_slice: if spec.view.plane == orv_core::Plane::Acquired {
                spec.view.slice
            } else {
                0
            },
            zoom: false,
        });
    }
    fn allow(&mut self, ctx: &egui::Context) -> Result<(), &'static str> {
        let draft = self.draft.as_ref().ok_or("No image review is open")?;
        if !draft.can_allow() {
            return Err("Review the preview, recipient, tool scope and risks first");
        }
        let preview = draft.preview.as_ref().ok_or("The preview is not ready")?;
        if draft.existing {
            let live = self.live.as_ref().ok_or("The session has ended")?;
            if !live.full_series {
                live.session
                    .approve_preview(&preview.data.snapshot.manifest.scene)?;
            }
        } else {
            let source = preview.data.source.clone();
            let scene = preview.data.snapshot.manifest.scene.clone();
            let consent = Consent {
                recipient: draft.recipient.trim().into(),
                data: if draft.full_series {
                    DataScope::SelectedSeries
                } else {
                    DataScope::ReviewedScenes
                },
                tools: draft.tools,
                preview_reviewed: draft.reviewed,
                burned_in_risk_accepted: draft.risk,
            };
            let session = Session::grant(source.clone(), consent, scene.clone())?;
            let repaint = ctx.clone();
            let server = match LocalServer::start(
                session.clone(),
                Arc::new(move || repaint.request_repaint()),
            ) {
                Ok(server) => server,
                Err(error) => {
                    session.revoke();
                    return Err(error);
                }
            };
            self.live = Some(Live {
                request_id: draft.request_id,
                session,
                server,
                source,
                recipient: draft.recipient.trim().into(),
                transport: draft.transport,
                tools: draft.tools,
                full_series: draft.full_series,
                output_size: scene.output_size,
                acquired_side: scene.reformat_side,
                last_published: Some(scene),
            });
            self.panel_open = draft.request_id.is_none();
        }
        self.draft = None;
        Ok(())
    }
    fn take_requested_view(&mut self) -> Option<SceneState> {
        if self.modal_open() {
            return None;
        }
        let live = self.live.as_mut()?;
        let scene = live.session.take_requested_view().ok()??;
        live.output_size = scene.output_size;
        if scene.reconstruction.is_none() && scene.view.plane == orv_core::Plane::Acquired {
            live.acquired_side = scene.reformat_side;
        }
        live.last_published = None;
        Some(scene)
    }
    fn publish(&mut self, spec: ViewSpec, ready: bool) {
        if !ready {
            return;
        }
        if let Some(live) = &mut self.live {
            let scene = spec.scene(live.source.description(), live.output_size);
            if live.last_published.as_ref() != Some(&scene) {
                if let Err(error) = live.session.publish_view(scene.clone()) {
                    self.notice = Some(error.into());
                }
                live.last_published = Some(scene);
            }
        }
    }
    fn poll(&mut self, ctx: &egui::Context) {
        if !self.requests_disabled && !self.broker_attempted {
            self.broker_attempted = true;
            let repaint = ctx.clone();
            match orv_mcp::broker::RequestBroker::start(Arc::new(move || repaint.request_repaint()))
            {
                Ok(broker) => self.broker = Some(broker),
                Err(e) => self.notice = Some(e.into()),
            }
        }
        if let Some(id) = self.draft.as_ref().and_then(|d| d.request_id)
            && !self.broker.as_ref().is_some_and(|b| b.is_pending(id))
        {
            self.draft = None;
            self.notice =
                Some("Agent request expired. Ask the agent to request access again.".into());
        }
        if let Some(live) = &mut self.live
            && let Some(id) = live.request_id
            && let ServerStatus::Listening { port } = live.server.status()
        {
            let result = self
                .broker
                .as_ref()
                .ok_or("Agent requests stopped")
                .and_then(|b| {
                    b.approve(
                        id,
                        live.session.clone(),
                        format!("http://127.0.0.1:{port}/mcp"),
                        live.server.token_for_configuration(),
                    )
                });
            live.request_id = None;
            if let Err(e) = result {
                self.notice = Some(e.into());
                self.revoke();
            }
        }
        self.comparison.poll(ctx);
        if let Some(live) = &self.live
            && live.session.is_revoked()
        {
            let message = match live.server.status() {
                ServerStatus::Failed(e) => e,
                _ => "Agent access ended. Images are private.",
            };
            self.notice = Some(message.into());
            self.revoke();
        }
        if let Some(draft) = &mut self.draft {
            draft.poll(ctx);
        }
    }
}
impl Drop for Sharing {
    fn drop(&mut self) {
        self.revoke();
    }
}
