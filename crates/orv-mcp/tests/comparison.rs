// SPDX-License-Identifier: GPL-3.0-or-later
use orv_agents::{AgentReply, Comparison, Consent, RecipientStatus, ReviewedSnapshot};
use orv_core::{
    ViewState,
    scene::{ImageMetadata, SceneState},
};
use orv_mcp::{
    http::{LocalServer, ServerStatus},
    local_client::LocalClient,
};
use orv_render::snapshot::{Snapshot, SnapshotManifest};
use serde_json::{Value, json};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

fn snapshot() -> Arc<ReviewedSnapshot> {
    use orv_core::scene::hex;
    use sha2::{Digest, Sha256};
    let mut png = std::io::Cursor::new(Vec::new());
    image::RgbaImage::from_pixel(2, 2, image::Rgba([100, 100, 100, 255]))
        .write_to(&mut png, image::ImageFormat::Png)
        .unwrap();
    let png = png.into_inner();
    let scene = SceneState {
        schema_version: 1,
        source_sha256: "0".repeat(64),
        series_index: 0,
        view: ViewState::default(),
        crosshair_mm: None,
        reformat_side: 64,
        output_size: [2, 2],
        reconstruction: None,
    };
    ReviewedSnapshot::new(Snapshot {
        manifest: SnapshotManifest {
            renderer_version: 1,
            scene_sha256: scene.hash().unwrap(),
            png_sha256: hex(&Sha256::digest(&png)),
            scene,
            image: Some(ImageMetadata {
                scalar: None,
                color: None,
                nifti: None,
                color_space: None,
                dimensions: [2, 2],
                monochrome1: false,
                geometry: None,
            }),
            volume: None,
            includes_ui_overlays: false,
        },
        png,
    })
    .unwrap()
}
async fn listening(server: &LocalServer) -> u16 {
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        match server.status() {
            ServerStatus::Listening { port } => return port,
            ServerStatus::Starting if Instant::now() < until => {
                tokio::time::sleep(Duration::from_millis(5)).await
            }
            status => panic!("Comparison listener did not start: {status:?}"),
        }
    }
}
async fn call(client: &LocalClient, name: &str, args: Value) -> rmcp::model::CallToolResult {
    client
        .call(name, args.as_object().unwrap().clone())
        .await
        .unwrap()
}

#[tokio::test]
async fn real_mcp_clients_receive_identical_frozen_data_and_isolated_replies() {
    let shared = snapshot();
    let weak = Arc::downgrade(&shared);
    let manifest = shared.snapshot().manifest.clone();
    let comparison = Comparison::grant(
        shared,
        Consent {
            recipients: vec!["Synthetic client A".into(), "Synthetic client B".into()],
            preview_reviewed: true,
            onward_sharing_accepted: true,
        },
    )
    .unwrap();
    let servers: Vec<_> = comparison
        .recipients()
        .iter()
        .map(|r| LocalServer::start_comparison(r.clone(), Arc::new(|| {})).unwrap())
        .collect();
    let pa = listening(&servers[0]).await;
    let pb = listening(&servers[1]).await;
    let mut a = LocalClient::connect(
        format!("http://127.0.0.1:{pa}/mcp"),
        servers[0].token_for_configuration(),
    )
    .await
    .unwrap();
    let mut b = LocalClient::connect(
        format!("http://127.0.0.1:{pb}/mcp"),
        servers[1].token_for_configuration(),
    )
    .await
    .unwrap();
    assert!(servers[0].token_for_configuration() != servers[1].token_for_configuration());
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let wrong = http
        .post(format!("http://127.0.0.1:{pb}/mcp"))
        .bearer_auth(servers[0].token_for_configuration())
        .json(&json!({"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}))
        .send()
        .await
        .unwrap();
    assert_eq!(wrong.status(), reqwest::StatusCode::UNAUTHORIZED);
    let list = http
        .post(format!("http://127.0.0.1:{pa}/mcp"))
        .bearer_auth(servers[0].token_for_configuration())
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2025-11-25")
        .json(&json!({"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    let names: Vec<_> = list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["name"].as_str().unwrap())
        .collect();
    assert_eq!(names.len(), 2);
    assert!(names.contains(&"comparison_snapshot") && names.contains(&"submit_commentary"));
    let reply = json!({"scene_sha256":manifest.scene_sha256,"png_sha256":manifest.png_sha256,"model":"Synthetic MCP fixture","commentary":"Experimental AI commentary: gray squares for transport verification only."});
    assert_eq!(
        call(&a, "submit_commentary", reply.clone()).await.is_error,
        Some(true)
    );
    assert_eq!(
        call(&a, "comparison_snapshot", json!({"scene":manifest.scene}))
            .await
            .is_error,
        Some(true)
    );
    let sa = call(&a, "comparison_snapshot", json!({})).await;
    let sb = call(&b, "comparison_snapshot", json!({})).await;
    assert_ne!(sa.is_error, Some(true));
    assert_eq!(sa.content, sb.content);
    assert_eq!(sa.structured_content, sb.structured_content);
    assert_eq!(
        sa.structured_content.unwrap(),
        serde_json::to_value(&manifest).unwrap()
    );
    let mut bad = reply.clone();
    bad["png_sha256"] = json!("1".repeat(64));
    assert_eq!(
        call(&a, "submit_commentary", bad).await.is_error,
        Some(true)
    );
    assert_ne!(
        call(&a, "submit_commentary", reply.clone()).await.is_error,
        Some(true)
    );
    let expected: AgentReply = serde_json::from_value(reply.clone()).unwrap();
    assert_eq!(
        comparison.recipients()[0].status(),
        RecipientStatus::Submitted(expected)
    );
    assert_eq!(
        comparison.recipients()[1].status(),
        RecipientStatus::SnapshotRead
    );
    assert_ne!(
        call(&a, "submit_commentary", reply.clone()).await.is_error,
        Some(true)
    );
    let mut replacement = reply;
    replacement["commentary"] = json!("replacement denied");
    assert_eq!(
        call(&a, "submit_commentary", replacement).await.is_error,
        Some(true)
    );
    comparison.recipients()[0].revoke();
    assert!(
        a.call("comparison_snapshot", serde_json::Map::new())
            .await
            .is_err()
    );
    assert_ne!(
        call(&b, "comparison_snapshot", json!({})).await.is_error,
        Some(true)
    );
    drop(comparison);
    assert!(
        weak.upgrade().is_none(),
        "All snapshot references released on revoke"
    );
    assert!(
        b.call("comparison_snapshot", serde_json::Map::new())
            .await
            .is_err()
    );
    a.close().await;
    b.close().await;
    for port in [pa, pb] {
        let until = Instant::now() + Duration::from_secs(3);
        while tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port))
            .await
            .is_ok()
        {
            assert!(Instant::now() < until, "Revocation must close listener");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
}

#[tokio::test]
async fn stdio_companion_exposes_only_comparison_tools_and_returns_commentary() {
    use rmcp::{
        ServiceExt,
        model::{CallToolRequestParams, ClientConfig},
    };
    use std::process::Stdio;
    use tokio::process::Command;
    let shared = snapshot();
    let manifest = shared.snapshot().manifest.clone();
    let comparison = Comparison::grant(
        shared,
        Consent {
            recipients: vec!["Stdio fixture".into(), "Other fixture".into()],
            preview_reviewed: true,
            onward_sharing_accepted: true,
        },
    )
    .unwrap();
    let recipient = comparison.recipients()[0].clone();
    let server = LocalServer::start_comparison(recipient.clone(), Arc::new(|| {})).unwrap();
    let port = listening(&server).await;
    let mut child = Command::new(env!("CARGO_BIN_EXE_openradview-mcp"))
        .args(["stdio", "--url", &format!("http://127.0.0.1:{port}/mcp")])
        .env("OPENRADVIEW_TOKEN", server.token_for_configuration())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut client = tokio::time::timeout(
        Duration::from_secs(5),
        ClientConfig::default().serve((child.stdout.take().unwrap(), child.stdin.take().unwrap())),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(client.list_all_tools().await.unwrap().len(), 2);
    let response = client
        .call_tool(
            CallToolRequestParams::new("comparison_snapshot").with_arguments(Default::default()),
        )
        .await
        .unwrap();
    assert_eq!(
        response.structured_content,
        Some(serde_json::to_value(&manifest).unwrap())
    );
    let reply = json!({"scene_sha256":manifest.scene_sha256,"png_sha256":manifest.png_sha256,"model":"Synthetic stdio fixture","commentary":"Experimental AI commentary: stdio roundtrip using generated pixels."});
    let result = client
        .call_tool(
            CallToolRequestParams::new("submit_commentary")
                .with_arguments(reply.as_object().unwrap().clone()),
        )
        .await
        .unwrap();
    assert_ne!(result.is_error, Some(true));
    assert!(matches!(recipient.status(), RecipientStatus::Submitted(_)));
    assert_eq!(
        comparison.recipients()[1].status(),
        RecipientStatus::Waiting
    );
    comparison.revoke();
    let after = tokio::time::timeout(
        Duration::from_secs(3),
        client.call_tool(
            CallToolRequestParams::new("comparison_snapshot").with_arguments(Default::default()),
        ),
    )
    .await
    .unwrap();
    assert!(after.is_err() || after.unwrap().is_error == Some(true));
    let _ = client.close().await;
    tokio::time::timeout(Duration::from_secs(3), child.wait())
        .await
        .unwrap()
        .unwrap();
}
