// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use crate::workspace::Workspace;
use orv_core::{Plane, study::demo_study};
use std::time::{Duration, Instant};
mod approval_priority;
mod reconstruction;

fn workspace() -> Workspace {
    let mut workspace = Workspace::default();
    workspace.study = Some(Arc::new(demo_study()));
    workspace.view.slice = 24;
    workspace
}

fn preview(sharing: &mut Sharing, ctx: &egui::Context) {
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        sharing.poll(ctx);
        let draft = sharing.draft.as_ref().expect("Review remains open");
        assert!(draft.error.is_none(), "Preview failed");
        if draft.preview.is_some() {
            return;
        }
        assert!(Instant::now() < until, "Preview timed out");
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn open(workspace: &mut Workspace, ctx: &egui::Context) {
    let spec = workspace.rendered_view().unwrap().0;
    let series = Arc::new(workspace.study.as_ref().unwrap().series[0].clone());
    workspace.sharing.open(0, series, spec, ctx);
    workspace.sharing.draft.as_mut().unwrap().full_series = false;
    preview(&mut workspace.sharing, ctx);
}
fn approve(workspace: &mut Workspace, ctx: &egui::Context) {
    let draft = workspace.sharing.draft.as_mut().unwrap();
    draft.recipient = "Synthetic desktop test".into();
    draft.tools = ToolScopes {
        view_control: true,
        snapshots: true,
        metadata: true,
        measurements: true,
    };
    draft.reviewed = true;
    draft.risk = true;
    workspace.sharing.allow(ctx).unwrap();
}

#[test]
fn local_series_comparison_revokes_sharing_and_cannot_publish_or_apply_shared_views() {
    use crate::workspace::ViewMode;
    let ctx = egui::Context::default();
    let mut workspace = workspace();
    let mut source = orv_core::study::demo_study();
    source.series.push(source.series[0].clone());
    workspace.study = Some(Arc::new(source));
    open(&mut workspace, &ctx);
    approve(&mut workspace, &ctx);
    let session = workspace.sharing.live.as_ref().unwrap().session.clone();
    let mut scene = session.current().unwrap();
    scene.view.slice = 3;
    session.request_view(scene.clone()).unwrap();
    workspace.set_mode(ViewMode::Compare, &ctx);
    assert!(session.is_revoked());
    assert!(!workspace.sharing.is_active());
    assert!(workspace.sharing.draft.is_none());
    assert!(
        workspace.rendered_view().is_none(),
        "Local comparison has no agent composition"
    );
    workspace.sharing_before_ui(&ctx, false);
    workspace.apply_shared_scene(&scene, &ctx);
    assert_eq!(workspace.mode, ViewMode::Compare);
    assert!(session.request_view(scene).is_err());
    workspace.set_mode(ViewMode::Acquired, &ctx);
    assert!(!workspace.sharing.is_active());
    assert!(
        workspace.sharing.live.is_none(),
        "Returning to one series needs a fresh Allow"
    );
    workspace.close();
}

#[test]
fn provider_warning_and_allow_remain_visible_without_scrolling_at_minimum_window_size() {
    let ctx = egui::Context::default();
    crate::theme::install(&ctx);
    let mut workspace = workspace();
    open(&mut workspace, &ctx);
    let draft = workspace.sharing.draft.as_mut().unwrap();
    draft.recipient = "Local test recipient".into();
    draft.full_series = true;
    for size in [egui::vec2(1280., 840.), egui::vec2(860., 640.)] {
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
        for pass in 0..3 {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(screen),
                    ..Default::default()
                },
                |ui| workspace.sharing.ui(ui.ctx()),
            );
            output.textures_delta.clear();
            if pass < 2 {
                continue;
            }
            for expected in [crate::copy::AGENT_PRIVACY, "Allow for this session"] {
                let (clip, text) = output
                    .shapes
                    .iter()
                    .find_map(|clipped| match &clipped.shape {
                        egui::epaint::Shape::Text(text) if text.galley.job.text == expected => {
                            Some((clipped.clip_rect, text))
                        }
                        _ => None,
                    })
                    .expect("Consent footer must be painted");
                let bounds = text.galley.rect.translate(text.pos.to_vec2());
                assert!(
                    screen.contains_rect(bounds),
                    "{bounds:?} outside {screen:?}"
                );
                assert!(
                    clip.expand(1.).contains_rect(bounds),
                    "Consent footer clipped"
                );
            }
        }
    }
    assert!(!workspace.sharing.is_active(), "Painting is never a grant");
    workspace.close();
}

#[test]
fn desktop_consent_requires_explicit_confirmation_and_close_revokes_without_retaining_source() {
    let ctx = egui::Context::default();
    let mut workspace = workspace();
    open(&mut workspace, &ctx);
    assert!(!workspace.sharing.is_active());
    assert!(workspace.sharing.allow(&ctx).is_err());
    let draft = workspace.sharing.draft.as_mut().unwrap();
    draft.recipient = "Synthetic agent".into();
    draft.reviewed = true;
    draft.risk = true;
    assert!(
        draft.tools.view_control
            && draft.tools.snapshots
            && draft.tools.metadata
            && draft.tools.measurements,
        "Tool preferences are preselected, not granted"
    );
    assert!(draft.can_allow());
    draft.tools.snapshots = true;
    let source = Arc::downgrade(&draft.preview.as_ref().unwrap().data.source);
    approve(&mut workspace, &ctx);
    let session = workspace.sharing.live.as_ref().unwrap().session.clone();
    assert!(workspace.sharing.is_active());
    workspace.close();
    assert!(session.is_revoked());
    assert!(workspace.study.is_none());
    let until = Instant::now() + Duration::from_secs(3);
    while source.upgrade().is_some() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(source.upgrade().is_none());
}

#[test]
fn acquired_and_mpr_requests_apply_and_publish_only_completed_physical_views() {
    let ctx = egui::Context::default();
    let mut workspace = workspace();
    open(&mut workspace, &ctx);
    approve(&mut workspace, &ctx);
    let session = workspace.sharing.live.as_ref().unwrap().session.clone();
    let mut scene = session.current().unwrap();
    scene.view.slice = 7;
    scene.view.invert = true;
    session.request_view(scene.clone()).unwrap();
    workspace.sharing_before_ui(&ctx, false);
    assert_eq!(workspace.view, scene.view);
    assert_eq!(workspace.mode, crate::workspace::ViewMode::Acquired);
    assert!(
        session
            .authorize(&scene, orv_mcp::consent::Operation::Snapshot)
            .is_err(),
        "View control cannot approve disclosure"
    );
    scene.view.plane = Plane::Coronal;
    scene.view.slice = 0;
    scene.crosshair_mm = Some([110.0, 100.0, 40.0]);
    scene.reformat_side = 128;
    session.request_view(scene.clone()).unwrap();
    workspace.sharing_before_ui(&ctx, false);
    assert_eq!(workspace.mode, crate::workspace::ViewMode::Mpr);
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        workspace.mpr.as_mut().unwrap().poll();
        if workspace.rendered_view().is_some_and(|(_, ready)| ready) {
            break;
        }
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(5));
    }
    let (spec, ready) = workspace.rendered_view().unwrap();
    assert_eq!(spec.point, scene.crosshair_mm);
    assert_eq!(spec.reformat_side, 128);
    assert_eq!(spec.view.plane, Plane::Coronal);
    workspace.sharing.publish(spec, ready);
    assert_eq!(session.current().unwrap(), scene);
    workspace.sharing.review(&ctx);
    preview(&mut workspace.sharing, &ctx);
    let draft = workspace.sharing.draft.as_mut().unwrap();
    assert_eq!(
        draft.preview.as_ref().unwrap().data.snapshot.manifest.scene,
        scene
    );
    draft.reviewed = true;
    draft.risk = true;
    workspace.sharing.allow(&ctx).unwrap();
    assert!(
        session
            .authorize(&scene, orv_mcp::consent::Operation::Snapshot)
            .is_ok()
    );
    workspace.close();
}

#[tokio::test]
async fn real_mcp_request_drives_desktop_then_reviewed_png_matches_the_preview() {
    let ctx = egui::Context::default();
    let mut workspace = workspace();
    open(&mut workspace, &ctx);
    approve(&mut workspace, &ctx);
    let until = Instant::now() + Duration::from_secs(5);
    let port = loop {
        match workspace.sharing.live.as_ref().unwrap().server.status() {
            ServerStatus::Listening { port } => break port,
            ServerStatus::Starting if Instant::now() < until => {
                tokio::time::sleep(Duration::from_millis(5)).await
            }
            _ => panic!("Local transport did not start"),
        }
    };
    let live = workspace.sharing.live.as_ref().unwrap();
    let mut client = orv_mcp::local_client::LocalClient::connect(
        format!("http://127.0.0.1:{port}/mcp"),
        live.server.token_for_configuration(),
    )
    .await
    .unwrap();
    let mut scene = live.session.current().unwrap();
    scene.view.slice = 9;
    scene.view.zoom = 1.25;
    scene.view.window = orv_core::WindowLevel::LUNG;
    scene.reformat_side = 64;
    scene.output_size = [320, 128];
    let arguments = serde_json::json!({"scene":scene})
        .as_object()
        .unwrap()
        .clone();
    assert_ne!(
        client
            .call("set_view", arguments.clone())
            .await
            .unwrap()
            .is_error,
        Some(true)
    );
    workspace.sharing_before_ui(&ctx, false);
    assert_eq!(workspace.view, scene.view);
    let (spec, ready) = workspace.rendered_view().unwrap();
    workspace.sharing.publish(spec, ready);
    assert_eq!(
        workspace
            .sharing
            .live
            .as_ref()
            .unwrap()
            .session
            .current()
            .unwrap(),
        scene
    );
    assert_eq!(
        client
            .call("render_snapshot", arguments.clone())
            .await
            .unwrap()
            .is_error,
        Some(true)
    );
    workspace.sharing.review(&ctx);
    preview(&mut workspace.sharing, &ctx);
    let draft = workspace.sharing.draft.as_mut().unwrap();
    let reviewed_hash = draft
        .preview
        .as_ref()
        .unwrap()
        .data
        .snapshot
        .manifest
        .png_sha256
        .clone();
    draft.reviewed = true;
    draft.risk = true;
    workspace.sharing.allow(&ctx).unwrap();
    let capture = client.call("render_snapshot", arguments).await.unwrap();
    assert_ne!(capture.is_error, Some(true));
    let manifest = capture.structured_content.unwrap();
    assert_eq!(
        manifest["png_sha256"], reviewed_hash,
        "Delivered image must match the desktop preview"
    );
    assert_eq!(manifest["scene_sha256"], scene.hash().unwrap());
    workspace.close();
    let denied = client.call("get_tags", Default::default()).await;
    assert!(denied.is_err() || denied.unwrap().is_error == Some(true));
    client.close().await;
}

#[test]
fn failed_preview_worker_reports_errors_for_later_requests_instead_of_waiting_forever() {
    let ctx = egui::Context::default();
    let mut series = demo_study().series.remove(0);
    series.frames.clear();
    let spec = ViewSpec {
        view: Default::default(),
        reconstruction: None,
        preferred_size: None,
        point: None,
        reformat_side: 512,
    };
    let worker = PreviewWorker::start(
        Input::Source(0, Arc::new(series)),
        spec.clone(),
        [32, 32],
        ctx,
    );
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(result) = worker.take() {
            assert!(result.is_err());
            break;
        }
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(5));
    }
    worker.request(spec, [32, 32]);
    assert!(worker.take().is_some_and(|result| result.is_err()));
}
