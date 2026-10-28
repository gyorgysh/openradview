// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use crate::workspace::ViewMode;
use eframe::egui_wgpu::wgpu;
use orv_core::{
    oblique::ObliquePlane,
    scene::Reconstruction,
    volume::Volume,
    volume_view::{Projection, Quality, VolumeView},
};

fn device() -> (wgpu::Device, wgpu::Queue) {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: orv_render::configured_backends().expect("Native backend selection"),
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapter = pollster::block_on(instance.request_adapter(&Default::default()))
        .expect("Real GPU required");
    pollster::block_on(adapter.request_device(&orv_render::device_descriptor(&adapter))).unwrap()
}
async fn client(workspace: &Workspace) -> orv_mcp::local_client::LocalClient {
    let until = Instant::now() + Duration::from_secs(5);
    let live = workspace.sharing.live.as_ref().unwrap();
    let port = loop {
        match live.server.status() {
            ServerStatus::Listening { port } => break port,
            ServerStatus::Starting if Instant::now() < until => {
                tokio::time::sleep(Duration::from_millis(5)).await
            }
            _ => panic!("Transport did not start"),
        }
    };
    orv_mcp::local_client::LocalClient::connect(
        format!("http://127.0.0.1:{port}/mcp"),
        live.server.token_for_configuration(),
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn exact_oblique_and_volume_requests_survive_desktop_review_and_deliver_identical_pngs() {
    let ctx = egui::Context::default();
    let (device, queue) = device();
    let mut workspace = workspace();
    let weak = Arc::downgrade(&workspace.study.as_ref().unwrap().series[0].frames[0]);
    let volume = Volume::new(workspace.study.as_ref().unwrap().series[0].frames.clone()).unwrap();
    let mut plane = ObliquePlane::from_angles(volume.center(), [31., -17., 11.]).unwrap();
    plane.center_mm[0] += 3.1425;
    plane.center_mm[1] -= 1.09;
    plane.center_mm[2] += 4.024;
    let mut volume_state = VolumeView::fit(&volume);
    volume_state.camera.yaw_degrees = 27.3125;
    volume_state.camera.pitch_degrees = -13.25;
    volume_state.camera.roll_degrees = 8.125;
    volume_state.camera.target_mm[0] += 0.03125;
    volume_state.projection = Projection::Composite;
    let mut preview_state = volume_state.clone();
    preview_state.quality = Quality::Interactive;
    preview_state.projection = Projection::Maximum;
    preview_state.clip = Some(orv_core::volume_view::PhysicalClip {
        minimum_mm: [50., 60., 20.],
        maximum_mm: [190., 210., 75.],
    });
    drop(volume);
    open(&mut workspace, &ctx);
    approve(&mut workspace, &ctx);
    let monitor = monitor_server(&workspace);
    let mut client = client(&workspace).await;
    let session = workspace.sharing.live.as_ref().unwrap().session.clone();
    for reconstruction in [
        Reconstruction::Oblique(plane.clone()),
        Reconstruction::Slab(orv_core::slab::SlabPlane {
            plane: plane.clone(),
            slab: orv_core::slab::Slab {
                projection: orv_core::slab::SlabProjection::Minimum,
                thickness_mm: 10.,
                intervals: 20,
            },
        }),
        Reconstruction::Volume(volume_state),
        Reconstruction::Volume(preview_state),
    ] {
        let mut scene = session.current().unwrap();
        scene.schema_version = 2;
        scene.view = orv_core::ViewState {
            zoom: 1.25,
            pan: [0.03125, -0.015625],
            invert: true,
            window: orv_core::WindowLevel::BONE,
            ..Default::default()
        };
        scene.crosshair_mm = None;
        scene.reformat_side = if matches!(reconstruction, Reconstruction::Volume(_)) {
            2
        } else {
            37
        };
        scene.output_size = [96, 64];
        scene.reconstruction = Some(reconstruction.clone());
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
        assert!(
            !workspace.rendered_view().is_some_and(|(_, ready)| ready),
            "Pending state cannot be published as a rendered image"
        );
        if matches!(reconstruction, Reconstruction::Volume(_)) {
            assert_eq!(workspace.mode, ViewMode::Volume);
            workspace.volume.as_mut().unwrap().start_upload(
                device.clone(),
                queue.clone(),
                wgpu::TextureFormat::Rgba8Unorm,
                ctx.clone(),
            );
        } else {
            assert_eq!(workspace.mode, ViewMode::Oblique);
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(oblique) = &mut workspace.oblique {
                oblique.poll();
            }
            if let Some(volume) = &mut workspace.volume {
                volume.prepare_test_frame(&device, &queue, &ctx);
            }
            if workspace.rendered_view().is_some_and(|(_, ready)| ready) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "Requested image did not complete"
            );
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        if let Some(oblique) = &mut workspace.oblique {
            let source =
                Volume::new(workspace.study.as_ref().unwrap().series[0].frames.clone()).unwrap();
            assert_eq!(
                oblique.plane_geometry(&source).unwrap().0,
                plane,
                "Idle UI must preserve exact requested axes and pivot"
            );
        }
        let (spec, ready) = workspace.rendered_view().unwrap();
        workspace.sharing.publish(spec, ready);
        assert_eq!(
            session.current().unwrap(),
            scene,
            "Display defaults or rounding must not change the requested scene"
        );
        assert!(session.approved_current().unwrap().is_none());
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
        let manifest = draft
            .preview
            .as_ref()
            .unwrap()
            .data
            .snapshot
            .manifest
            .clone();
        assert_eq!(manifest.scene, scene);
        draft.reviewed = true;
        draft.risk = true;
        workspace.sharing.allow(&ctx).unwrap();
        let response = client.call("render_snapshot", arguments).await.unwrap();
        assert_ne!(response.is_error, Some(true));
        let delivered = response.structured_content.unwrap();
        assert_eq!(delivered["png_sha256"], manifest.png_sha256);
        assert_eq!(delivered["scene_sha256"], scene.hash().unwrap());
    }
    workspace.close();
    client.close().await;
    wait_for_server(&monitor);
    let deadline = Instant::now() + Duration::from_secs(5);
    while weak.upgrade().is_some() {
        assert!(
            Instant::now() < deadline,
            "Revoked session retained source pixels"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    crate::sharing::preview::finish_test_workers();
}

#[tokio::test]
async fn selected_series_scope_includes_explicit_new_views_but_never_another_source() {
    let ctx = egui::Context::default();
    let mut workspace = workspace();
    let volume = Volume::new(workspace.study.as_ref().unwrap().series[0].frames.clone()).unwrap();
    let state = VolumeView::fit(&volume);
    let plane = ObliquePlane::from_angles(volume.center(), [35., -18., 12.]).unwrap();
    drop(volume);
    open(&mut workspace, &ctx);
    workspace.sharing.draft.as_mut().unwrap().full_series = true;
    approve(&mut workspace, &ctx);
    let monitor = monitor_server(&workspace);
    let mut client = client(&workspace).await;
    let mut scene = workspace
        .sharing
        .live
        .as_ref()
        .unwrap()
        .session
        .current()
        .unwrap();
    scene.schema_version = 2;
    scene.view.slice = 0;
    scene.reformat_side = 2;
    scene.output_size = [24, 16];
    for reconstruction in [
        Reconstruction::Volume(state),
        Reconstruction::Slab(orv_core::slab::SlabPlane {
            plane: plane.clone(),
            slab: orv_core::slab::Slab {
                projection: orv_core::slab::SlabProjection::Maximum,
                thickness_mm: 10.,
                intervals: 20,
            },
        }),
        Reconstruction::Oblique(plane),
    ] {
        scene.reformat_side = if matches!(reconstruction, Reconstruction::Volume(_)) {
            2
        } else {
            32
        };
        scene.reconstruction = Some(reconstruction);
        let response = client
            .call(
                "render_snapshot",
                serde_json::json!({"scene":scene})
                    .as_object()
                    .unwrap()
                    .clone(),
            )
            .await
            .unwrap();
        assert_ne!(response.is_error, Some(true));
    }
    scene.series_index = 1;
    let denied = client
        .call(
            "render_snapshot",
            serde_json::json!({"scene":scene})
                .as_object()
                .unwrap()
                .clone(),
        )
        .await
        .unwrap();
    assert_eq!(denied.is_error, Some(true));
    workspace.close();
    client.close().await;
    wait_for_server(&monitor);
    crate::sharing::preview::finish_test_workers();
}
