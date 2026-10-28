// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use base64::Engine;
use orv_core::{ViewState, study::demo_study};
use orv_mcp::local_client::LocalClient;
use std::time::{Duration, Instant};

#[tokio::test]
async fn native_review_freezes_one_real_metal_image_for_both_clients_and_releases_source() {
    let ctx = egui::Context::default();
    let mut sharing = crate::sharing::Sharing::default();
    let series = Arc::new(demo_study().series.remove(0));
    sharing.comparison.open(
        0,
        series,
        ViewSpec {
            view: ViewState::default(),
            point: None,
            reformat_side: 512,
            reconstruction: None,
            preferred_size: None,
        },
        &ctx,
    );
    assert!(sharing.modal_open() && !sharing.is_active());
    // Software-rendered previews can take longer on shared native CI hosts.
    let until = Instant::now() + Duration::from_secs(60);
    while sharing.comparison.draft.as_ref().unwrap().preview.is_none() {
        sharing.poll(&ctx);
        assert!(sharing.comparison.draft.as_ref().unwrap().error.is_none());
        assert!(Instant::now() < until, "Metal preview did not finish");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let draft = sharing.comparison.draft.as_mut().unwrap();
    assert!(!draft.reviewed && !draft.risk && draft.recipients.iter().all(String::is_empty));
    draft.recipients = vec!["Synthetic A".into(), "Synthetic B".into()];
    let preview = draft.preview.as_ref().unwrap();
    let expected = serde_json::to_value(&preview.data.snapshot.manifest).unwrap();
    let pixels = preview.data.snapshot.png.clone();
    let source = Arc::downgrade(&preview.data.source);
    assert!(sharing.comparison.allow(&ctx).is_err());
    assert!(sharing.comparison.live.is_none());
    let draft = sharing.comparison.draft.as_mut().unwrap();
    draft.reviewed = true;
    assert!(!draft.can_allow());
    draft.risk = true;
    assert!(draft.can_allow());
    sharing.comparison.allow(&ctx).unwrap();
    assert!(!sharing.modal_open() && sharing.is_active());
    let until = Instant::now() + Duration::from_secs(5);
    while source.upgrade().is_some() {
        assert!(
            Instant::now() < until,
            "Frozen comparison retained source series"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let live = sharing.comparison.live.as_ref().unwrap();
    let recipients = live.comparison.recipients().to_vec();
    let mut clients = Vec::new();
    for server in &live.servers {
        let until = Instant::now() + Duration::from_secs(5);
        let port = loop {
            match server.status() {
                ServerStatus::Listening { port } => break port,
                ServerStatus::Starting if Instant::now() < until => {
                    tokio::time::sleep(Duration::from_millis(5)).await
                }
                other => panic!("Comparison failed: {other:?}"),
            }
        };
        clients.push(
            LocalClient::connect(
                format!("http://127.0.0.1:{port}/mcp"),
                server.token_for_configuration(),
            )
            .await
            .unwrap(),
        );
    }
    for (i, client) in clients.iter().enumerate() {
        let snapshot = client
            .call("comparison_snapshot", Default::default())
            .await
            .unwrap();
        assert_eq!(snapshot.structured_content, Some(expected.clone()));
        let image = serde_json::to_value(&snapshot.content[0]).unwrap();
        // Compare with the native-reviewed PNG without exposing image bytes/logs.
        let decoded = serde_json::from_value::<String>(image["data"].clone()).unwrap();
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(decoded)
                .unwrap(),
            pixels
        );
        let arguments = serde_json::json!({"scene_sha256":expected["scene_sha256"],"png_sha256":expected["png_sha256"],"model":"Synthetic integration fixture","commentary":format!("Experimental AI commentary {}: generated demo only.",i+1)});
        let submitted = client
            .call("submit_commentary", arguments.as_object().unwrap().clone())
            .await
            .unwrap();
        assert_ne!(submitted.is_error, Some(true));
    }
    assert!(
        recipients
            .iter()
            .all(|r| matches!(r.status(), orv_agents::RecipientStatus::Submitted(_)))
    );
    // Local two-series viewing invalidates frozen commentary grants too.
    sharing.enter_local_comparison();
    assert!(!sharing.is_active() && sharing.comparison.live.is_none());
    for (recipient, client) in recipients.iter().zip(&mut clients) {
        assert!(recipient.is_revoked() && recipient.read_snapshot().is_err());
        assert!(
            client
                .call("comparison_snapshot", Default::default())
                .await
                .is_err()
        );
        client.close().await;
    }
    crate::sharing::preview::finish_test_workers();
}
