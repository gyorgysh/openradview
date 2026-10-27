// SPDX-License-Identifier: GPL-3.0-or-later
//! MCP tools over already-approved in-memory data. No filesystem operations.
use crate::catalog::Resolved;
use crate::consent::{Operation, Session};
use base64::{Engine, engine::general_purpose::STANDARD};
use orv_core::{
    measurements,
    scene::{ImageMetadata, SceneState, VolumeMetadata},
};
use orv_render::snapshot::SnapshotRenderer;
use rmcp::{
    ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{
        CallToolResult, ContentBlock as Content, Implementation, ServerCapabilities, ServerConfig,
    },
    tool, tool_handler, tool_router,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use std::sync::{Arc, Mutex};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EmptyRequest {}

#[derive(Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SceneRequest {
    pub scene: Option<SceneState>,
}

#[derive(Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ViewRequest {
    pub scene: SceneState,
}

#[derive(Clone, Copy, Deserialize, JsonSchema)]
pub enum MeasurementKind {
    Distance,
    Angle,
}

#[derive(Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MeasureRequest {
    pub scene: Option<SceneState>,
    pub kind: MeasurementKind,
    /// Zero-based image pixel centres: two for distance; three for angle (vertex second).
    pub points: Vec<[f64; 2]>,
}

#[derive(Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RoiRequest {
    pub scene: Option<SceneState>,
    pub corners: [[f64; 2]; 2],
}

#[derive(Clone)]
pub struct ViewerTools {
    session: Session,
    renderer: Arc<Mutex<Option<SnapshotRenderer>>>,
    busy: Arc<Semaphore>,
    notify_ui: Arc<dyn Fn() + Send + Sync>,
    tool_router: ToolRouter<Self>,
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for ViewerTools {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("OpenRadView", env!("CARGO_PKG_VERSION")))
            .with_instructions(format!("{}\n\n{}", "Only the already-opened, explicitly approved series is accessible. No tool accepts a file path. Tag removal does not remove burned-in identifiers. New views may require desktop review before data is shared.", crate::guidance::AGENT_GUIDANCE))
    }
}

#[tool_router(router = tool_router)]
impl ViewerTools {
    pub fn new(session: Session, notify_ui: Arc<dyn Fn() + Send + Sync>) -> Self {
        Self {
            session,
            renderer: Arc::new(Mutex::new(None)),
            busy: Arc::new(Semaphore::new(1)),
            notify_ui,
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        description = "Describe the sole already-opened, user-approved study scope. This never opens a path or reads another study."
    )]
    pub fn open_study(&self, Parameters(_): Parameters<EmptyRequest>) -> CallToolResult {
        result((|| {
            Ok(CallToolResult::structured(json!({
                "series": self.session.description()?, "view": self.session.approved_current()?,
                "restriction": "Only this approved series is available; no arbitrary filesystem access."
            })))
        })())
    }

    #[tool(
        description = "List only the series explicitly approved for this session; no original UIDs, descriptions, names or paths."
    )]
    pub fn list_series(&self, Parameters(_): Parameters<EmptyRequest>) -> CallToolResult {
        result(
            self.session
                .description()
                .map(|series| CallToolResult::structured(json!({"series": [series]}))),
        )
    }

    #[tool(
        description = "Queue an explicit acquired, LPS MPR, oblique or volume view in the desktop app. Changing the view does not expand data-sharing consent."
    )]
    pub async fn set_view(&self, Parameters(request): Parameters<ViewRequest>) -> CallToolResult {
        let notify = self.notify_ui.clone();
        self.compute(Some(request.scene), Operation::SetView, move |_, scene, session, _| {
            session.request_view(scene.clone())?;
            notify();
            Ok(CallToolResult::structured(json!({"queued": scene,
                "status": "Queued for desktop display; sharing may require review of this exact view."})))
        }).await
    }

    #[tool(
        description = "Render a consented exact view as PNG plus SceneState/geometry/hash manifest. Contains image pixels, without UI labels or annotations."
    )]
    pub async fn render_snapshot(
        &self,
        Parameters(request): Parameters<SceneRequest>,
    ) -> CallToolResult {
        let renderer = self.renderer.clone();
        self.compute(
            request.scene,
            Operation::Snapshot,
            move |frame, scene, _, cancel| {
                let mut guard = renderer
                    .lock()
                    .map_err(|_| "Snapshot renderer unavailable")?;
                if guard.is_none() {
                    *guard = Some(pollster_new_renderer()?);
                }
                let renderer = guard.as_mut().ok_or("Snapshot renderer unavailable")?;
                let snapshot = match frame {
                    Resolved::Image(frame) => {
                        renderer.capture(frame, scene, || cancel.is_cancelled())?
                    }
                    Resolved::Volume(volume) => {
                        renderer.capture_volume(volume.clone(), scene, || cancel.is_cancelled())?
                    }
                };
                let manifest = serde_json::to_value(&snapshot.manifest)
                    .map_err(|_| "Could not encode snapshot manifest")?;
                let mut response = CallToolResult::success(vec![
                    Content::image(STANDARD.encode(&snapshot.png), "image/png"),
                    Content::json(&manifest).map_err(|_| "Could not encode snapshot manifest")?,
                ]);
                response.structured_content = Some(manifest);
                Ok(response)
            },
        )
        .await
    }

    #[tool(
        description = "Get allowlisted dimensions/photometric/plane geometry or full affine volume geometry for the consented view. Raw DICOM tags and identifiers are never returned."
    )]
    pub async fn get_tags(&self, Parameters(request): Parameters<SceneRequest>) -> CallToolResult {
        self.compute(
            request.scene,
            Operation::Metadata,
            |resolved, scene, _, _| {
                let metadata = match resolved {
                    Resolved::Image(frame) => {
                        let mut image = ImageMetadata::from_frame(frame);
                        image.scalar = None;
                        json!({"scene_sha256":scene.hash()?,"image":image})
                    }
                    Resolved::Volume(volume) => {
                        let mut source = VolumeMetadata::from_volume(volume);
                        source.scalar = None;
                        json!({"scene_sha256":scene.hash()?,"volume":source})
                    }
                };
                Ok(CallToolResult::structured(metadata))
            },
        )
        .await
    }

    #[tool(
        description = "Research distance in mm or angle in degrees, from finite pixel-centre coordinates in a consented image with valid physical geometry."
    )]
    pub async fn measure(&self, Parameters(request): Parameters<MeasureRequest>) -> CallToolResult {
        let expected = match request.kind {
            MeasurementKind::Distance => 2,
            MeasurementKind::Angle => 3,
        };
        if request.points.len() != expected {
            return failure("Incorrect number of measurement points");
        }
        self.compute(request.scene, Operation::Measurement, move |frame, scene, _, _| {
            let frame=frame.image()?;
            let g = frame.geometry.as_ref().ok_or("Physical image geometry is unavailable")?;
            let mut world = Vec::with_capacity(expected);
            for point in request.points {
                if point.iter().any(|v| !v.is_finite()) || point[0] < 0.0 || point[1] < 0.0
                    || point[0] > (frame.width - 1) as f64 || point[1] > (frame.height - 1) as f64 {
                    return Err("Measurement points must lie inside the image");
                }
                world.push(g.pixel_to_world(point).map_err(|_| "Physical image geometry is invalid")?);
            }
            let (value, unit) = match request.kind {
                MeasurementKind::Distance => (measurements::distance_mm(world[0], world[1])?, "mm"),
                MeasurementKind::Angle => (measurements::angle_degrees(world[0], world[1], world[2])?, "degrees"),
            };
            Ok(CallToolResult::structured(json!({"scene_sha256": scene.hash()?, "value": value, "unit": unit, "research_only": true})))
        }).await
    }

    #[tool(
        description = "Compute inclusive rectangular ROI scalar statistics in modality units for a consented image. Coordinates are zero-based pixel centres; no-data samples are ignored."
    )]
    pub async fn roi_stats(&self, Parameters(request): Parameters<RoiRequest>) -> CallToolResult {
        self.compute(request.scene, Operation::Measurement, move |frame, scene, _, _| {
            let frame=frame.image()?;
            Ok(CallToolResult::structured(json!({"scene_sha256": scene.hash()?,
                "statistics": measurements::roi_stats(frame, request.corners)?, "research_only": true})))
        }).await
    }
}

impl ViewerTools {
    pub fn definitions() -> Vec<rmcp::model::Tool> {
        Self::tool_router().list_all()
    }

    async fn compute(
        &self,
        scene: Option<SceneState>,
        operation: Operation,
        work: impl FnOnce(
            &Resolved,
            &SceneState,
            &Session,
            &CancellationToken,
        ) -> Result<CallToolResult, &'static str>
        + Send
        + 'static,
    ) -> CallToolResult {
        let scene = match scene.map(Ok).unwrap_or_else(|| self.session.current()) {
            Ok(v) => v,
            Err(e) => return failure(e),
        };
        let source = match self.session.authorize(&scene, operation) {
            Ok(v) => v,
            Err(e) => return failure(e),
        };
        let permit = match self.busy.clone().try_acquire_owned() {
            Ok(v) => v,
            Err(_) => return failure("Viewer is busy; retry after the current request"),
        };
        let cancel = self.session.cancellation().child_token();
        let _cancel_on_return = cancel.clone().drop_guard();
        let worker_cancel = cancel.clone();
        let session = self.session.clone();
        let after_scene = scene.clone();
        let task = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let frame = source.resolve_scene(&scene, || worker_cancel.is_cancelled())?;
            if worker_cancel.is_cancelled() {
                return Err("Request cancelled");
            }
            work(&frame, &scene, &session, &worker_cancel)
        });
        let output = tokio::select! {
            _ = cancel.cancelled() => Err("Session access has been revoked"),
            result = tokio::time::timeout(std::time::Duration::from_secs(15), task) => match result {
                Ok(Ok(value)) => value,
                Ok(Err(_)) => Err("Viewer worker stopped unexpectedly"),
                Err(_) => Err("Viewer request timed out"),
            }
        };
        // Discard even a finished PNG if consent changed while work was running.
        result(self.session.authorize(&after_scene, operation).and(output))
    }
}

fn pollster_new_renderer() -> Result<SnapshotRenderer, &'static str> {
    // This is called exclusively inside spawn_blocking, never on the UI thread.
    pollster::block_on(SnapshotRenderer::new())
}
fn failure(message: &'static str) -> CallToolResult {
    CallToolResult::error(vec![Content::text(message)])
}
fn result(value: Result<CallToolResult, &'static str>) -> CallToolResult {
    value.unwrap_or_else(failure)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        catalog::PreparedSeries,
        consent::{Consent, DataScope, ToolScopes},
    };
    use orv_core::{ViewState, study::demo_study};

    #[tokio::test]
    async fn geometry_permission_does_not_add_source_intensity_metadata() {
        use orv_core::scalar::{ScalarData, ScalarInfo, Storage, Unit, ValueType, VoiChoices};
        let mut series = demo_study().series.remove(0);
        series.frames.truncate(1);
        Arc::make_mut(&mut series.frames[0]).scalar = Some(
            ScalarData::new(
                ScalarInfo {
                    value_type: ValueType::F32,
                    storage: Storage::SignedInteger,
                    stored_bits: 16,
                    unit: Unit::Hounsfield,
                    rescale: None,
                    resampled: false,
                },
                VoiChoices::new(Vec::new(), Vec::new()).unwrap(),
            )
            .unwrap(),
        );
        let source = Arc::new(PreparedSeries::new(0, Arc::new(series), || false).unwrap());
        let scene = SceneState {
            schema_version: 1,
            source_sha256: source.description().source_sha256.clone(),
            series_index: 0,
            view: ViewState::default(),
            reconstruction: None,
            crosshair_mm: None,
            reformat_side: 64,
            output_size: [64, 64],
        };
        let mut invalid = scene.clone();
        invalid.view.window = orv_core::WindowLevel::lookup(0);
        assert!(source.check_scene(&invalid).is_err());
        let consent = Consent {
            recipient: "Generated scope test".into(),
            data: DataScope::ReviewedScenes,
            tools: ToolScopes {
                view_control: false,
                snapshots: false,
                metadata: true,
                measurements: false,
            },
            preview_reviewed: true,
            burned_in_risk_accepted: true,
        };
        let session = Session::grant(source, consent, scene).unwrap();
        let tools = ViewerTools::new(session, Arc::new(|| {}));
        let response = tools
            .get_tags(Parameters(SceneRequest { scene: None }))
            .await;
        assert_ne!(response.is_error, Some(true));
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("dimensions"));
        for field in ["scalar", "stored_bits", "hounsfield", "value_type"] {
            assert!(!json.contains(field));
        }
    }

    #[tokio::test]
    async fn revocation_discards_in_flight_data_and_cancels_the_worker() {
        let mut study = demo_study();
        let mut series = study.series.remove(0);
        series.frames.truncate(2);
        let source = Arc::new(PreparedSeries::new(0, Arc::new(series), || false).unwrap());
        let scene = SceneState {
            schema_version: 1,
            source_sha256: source.description().source_sha256.clone(),
            series_index: 0,
            view: ViewState::default(),
            reconstruction: None,
            crosshair_mm: None,
            reformat_side: 64,
            output_size: [64, 64],
        };
        let consent = Consent {
            recipient: "Synthetic worker test".into(),
            data: DataScope::ReviewedScenes,
            tools: ToolScopes {
                view_control: true,
                snapshots: true,
                metadata: true,
                measurements: true,
            },
            preview_reviewed: true,
            burned_in_risk_accepted: true,
        };
        let session = Session::grant(source, consent, scene.clone()).unwrap();
        let tools = ViewerTools::new(session.clone(), Arc::new(|| {}));
        let (started, ready) = tokio::sync::oneshot::channel();
        let (stopped, done) = tokio::sync::oneshot::channel();
        let worker = tools.clone();
        let task = tokio::spawn(async move {
            worker
                .compute(Some(scene), Operation::Metadata, move |_, _, _, cancel| {
                    let _ = started.send(());
                    while !cancel.is_cancelled() {
                        std::thread::sleep(std::time::Duration::from_millis(1));
                    }
                    let _ = stopped.send(());
                    Ok(CallToolResult::structured(
                        json!({"private_result":"must not be delivered"}),
                    ))
                })
                .await
        });
        tokio::time::timeout(std::time::Duration::from_secs(3), ready)
            .await
            .unwrap()
            .unwrap();
        let busy = tools
            .get_tags(Parameters(SceneRequest { scene: None }))
            .await;
        assert_eq!(
            busy.is_error,
            Some(true),
            "Concurrent work must fail promptly instead of queuing"
        );
        session.revoke();
        let response = tokio::time::timeout(std::time::Duration::from_secs(3), task)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(response.is_error, Some(true));
        assert!(
            !serde_json::to_string(&response)
                .unwrap()
                .contains("private_result")
        );
        tokio::time::timeout(std::time::Duration::from_secs(3), done)
            .await
            .unwrap()
            .unwrap();
    }
}
