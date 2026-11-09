// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use crate::{mpr::MprSession, workspace::ViewMode};
use eframe::egui_wgpu::wgpu;
use orv_core::scene::frame_pixel_hash;
use orv_mcp::{broker::RequestBroker, consent::BearerToken, http::ServerStatus};

#[test]
fn local_gpu_frames_can_be_captured_but_inbound_review_waits_for_canonical_frames() {
    let ctx = egui::Context::default();
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        flags: orv_render::instance_flags(),
        backends: orv_render::configured_backends().unwrap(),
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapter = pollster::block_on(instance.request_adapter(&Default::default()))
        .expect("Actual GPU required");
    let (device, queue) =
        pollster::block_on(adapter.request_device(&orv_render::device_descriptor(&adapter)))
            .unwrap();
    let mut workspace = workspace();
    workspace.sharing.requests_disabled = true;
    workspace.mode = ViewMode::Mpr;
    workspace.mpr = Some(MprSession::start(
        Arc::new(workspace.study.as_ref().unwrap().series[0].clone()),
        workspace.view.window,
        ctx.clone(),
    ));
    workspace.mpr.as_ref().unwrap().offer_gpu(&device, &queue);
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        let mpr = workspace.mpr.as_mut().unwrap();
        mpr.poll();
        if mpr.is_settled()
            && mpr
                .snapshot
                .as_ref()
                .unwrap()
                .gpu_images
                .iter()
                .all(Option::is_some)
        {
            break;
        }
        assert!(
            Instant::now() < until,
            "Local GPU navigation did not settle"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(workspace.picture_ready());
    assert!(!workspace.canonical_rendered_view().unwrap().1);
    let mpr = workspace.mpr.as_mut().unwrap();
    mpr.picture_sizes = [Some([64, 64]); 3];
    let local = mpr.snapshot.as_ref().unwrap().images[mpr.active_index()].clone();
    let Some((crate::export::Pixels::Reformat(captured, _), _)) = mpr.picture() else {
        panic!("Actual GPU frame capture");
    };
    assert!(Arc::ptr_eq(&local, &captured));

    let broker = RequestBroker::start_on(0, Arc::new(|| {})).unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    let port = loop {
        match broker.status() {
            ServerStatus::Listening { port } => break port,
            ServerStatus::Starting if Instant::now() < until => {
                std::thread::sleep(Duration::from_millis(5))
            }
            state => panic!("Isolated broker did not start: {state:?}"),
        }
    };
    workspace.sharing.broker = Some(broker);
    workspace.sharing.broker_attempted = true;
    let token = BearerToken::generate().unwrap();
    let key = token.expose_for_configuration();
    let pending = serde_json::json!({"status":"pending"});
    assert_eq!(approval_priority::request(port, key, "POST"), pending);
    workspace.sharing_before_ui(&ctx, true);
    assert!(workspace.sharing.draft.is_none());
    assert!(
        workspace
            .sharing
            .broker
            .as_ref()
            .unwrap()
            .has_unseen_request()
    );
    let until = Instant::now() + Duration::from_secs(10);
    while workspace.sharing.draft.is_none() {
        assert_eq!(approval_priority::request(port, key, "GET"), pending);
        assert!(!workspace.sharing.is_active());
        workspace.sharing_before_ui(&ctx, false);
        assert!(Instant::now() < until, "Canonical review never opened");
        std::thread::sleep(Duration::from_millis(5));
    }
    let mpr = workspace.mpr.as_ref().unwrap();
    assert!(mpr.is_settled());
    let snapshot = mpr.snapshot.as_ref().unwrap();
    assert!(snapshot.gpu_images.iter().all(Option::is_none));
    assert!(!Arc::ptr_eq(&snapshot.images[mpr.active_index()], &local));
    assert!(workspace.canonical_rendered_view().unwrap().1);
    for (i, plane) in crate::mpr_worker::PLANES.into_iter().enumerate() {
        let expected = snapshot
            .volume
            .reformat(plane, snapshot.point, snapshot.reformat_side, || false)
            .unwrap();
        assert_eq!(
            frame_pixel_hash(&expected, || false).unwrap(),
            frame_pixel_hash(&snapshot.images[i], || false).unwrap()
        );
    }
    preview(&mut workspace.sharing, &ctx);
    assert!(
        !workspace.sharing.is_active(),
        "Canonical preparation is not consent"
    );
    let request = workspace
        .sharing
        .draft
        .as_ref()
        .unwrap()
        .request_id
        .unwrap();
    workspace.sharing.broker.as_ref().unwrap().deny(request);
    workspace.sharing_before_ui(&ctx, false);
    assert!(workspace.sharing.draft.is_none());
    assert_eq!(
        approval_priority::request(port, key, "GET"),
        serde_json::json!({"status":"denied"})
    );
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        workspace.sharing_before_ui(&ctx, false);
        let mpr = workspace.mpr.as_ref().unwrap();
        if mpr.is_settled()
            && mpr
                .snapshot
                .as_ref()
                .unwrap()
                .gpu_images
                .iter()
                .all(Option::is_some)
        {
            break;
        }
        assert!(
            Instant::now() < until,
            "Local GPU mode did not resume after denial"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    workspace.close();
    super::super::preview::finish_test_workers();
}
