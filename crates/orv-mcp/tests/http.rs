// SPDX-License-Identifier: GPL-3.0-or-later
mod common;
use orv_mcp::http::{LocalServer, ServerStatus};
use reqwest::{Client, StatusCode};
use serde_json::{Value, json};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

async fn listening(server: &LocalServer) -> u16 {
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        match server.status() {
            ServerStatus::Listening { port } => return port,
            ServerStatus::Starting if Instant::now() < until => {
                tokio::time::sleep(Duration::from_millis(5)).await
            }
            status => panic!("Server did not start: {status:?}"),
        }
    }
}

fn client() -> Client {
    Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap()
}
fn request(
    client: &Client,
    port: u16,
    token: &str,
    method: &str,
    params: Value,
) -> reqwest::RequestBuilder {
    client
        .post(format!("http://127.0.0.1:{port}/mcp"))
        .bearer_auth(token)
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2025-11-25")
        .json(&json!({"jsonrpc":"2.0", "id":1, "method":method, "params":params}))
}
async fn call(client: &Client, port: u16, token: &str, name: &str, args: Value) -> Value {
    let response = request(
        client,
        port,
        token,
        "tools/call",
        json!({"name":name,"arguments":args}),
    )
    .send()
    .await
    .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    response.json().await.unwrap()
}

#[tokio::test]
async fn authenticated_tools_are_scoped_and_revoke_closes_the_listener() {
    let (session, scene) = common::session();
    let (notify, mut notifications) = tokio::sync::mpsc::unbounded_channel();
    let server = LocalServer::start(
        session.clone(),
        Arc::new(move || {
            let _ = notify.send(());
        }),
    )
    .unwrap();
    let port = listening(&server).await;
    let token = server.token_for_configuration();
    let client = client();
    let init = request(&client, port, token, "initialize", json!({"protocolVersion":"2025-11-25", "capabilities":{}, "clientInfo":{"name":"synthetic-tests","version":"1"}}))
        .send().await.unwrap();
    assert_eq!(init.status(), StatusCode::OK);
    assert_eq!(
        init.json::<Value>().await.unwrap()["result"]["serverInfo"]["name"],
        "OpenRadView"
    );
    let list = request(&client, port, token, "tools/list", json!({}))
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
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    for required in [
        "open_study",
        "list_series",
        "set_view",
        "render_snapshot",
        "get_tags",
        "measure",
        "roi_stats",
    ] {
        assert!(names.contains(&required));
    }
    assert_eq!(names.len(), 7);
    let open = call(&client, port, token, "open_study", json!({})).await;
    assert_eq!(
        open["result"]["structuredContent"]["series"]["image_count"],
        3
    );
    let denied = call(
        &client,
        port,
        token,
        "open_study",
        json!({"path":"/not-approved"}),
    )
    .await;
    assert!(denied.get("error").is_some() || denied["result"]["isError"] == true);
    let tags = call(&client, port, token, "get_tags", json!({})).await;
    let encoded = serde_json::to_string(&tags).unwrap();
    assert!(
        !encoded.contains("2.25.314159")
            && !encoded.contains("FrameOfReferenceUID")
            && !encoded.contains("PatientName")
    );
    assert_eq!(
        tags["result"]["structuredContent"]["image"]["dimensions"],
        json!([256, 256])
    );
    let measurement = call(
        &client,
        port,
        token,
        "measure",
        json!({"kind":"Distance", "points":[[0,0],[3,4]]}),
    )
    .await;
    assert_eq!(measurement["result"]["structuredContent"]["value"], 5.0);
    let roi = call(
        &client,
        port,
        token,
        "roi_stats",
        json!({"corners":[[0,0],[1,1]]}),
    )
    .await;
    assert_eq!(roi["result"]["structuredContent"]["statistics"]["count"], 4);
    #[cfg(feature = "gpu-tests")]
    {
        use base64::Engine;
        let capture = call(&client, port, token, "render_snapshot", json!({})).await;
        let image = &capture["result"]["content"][0];
        assert_eq!(image["mimeType"], "image/png");
        let png = base64::engine::general_purpose::STANDARD
            .decode(image["data"].as_str().unwrap())
            .unwrap();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(
            capture["result"]["structuredContent"]["scene_sha256"],
            scene.hash().unwrap()
        );
    }
    let mut next = scene.clone();
    next.view.slice = 1;
    let view = call(&client, port, token, "set_view", json!({"scene":next})).await;
    assert_eq!(
        view["result"]["structuredContent"]["queued"]["view"]["slice"],
        1
    );
    assert_eq!(session.take_requested_view().unwrap(), Some(next.clone()));
    let denied = call(&client, port, token, "get_tags", json!({})).await;
    assert_eq!(denied["result"]["isError"], true);
    let context = call(&client, port, token, "open_study", json!({})).await;
    assert!(context["result"]["structuredContent"]["view"].is_null());
    session.approve_preview(&next).unwrap();
    let tags = call(&client, port, token, "get_tags", json!({})).await;
    assert_ne!(tags["result"]["isError"], true);
    // Completed requests may have notified about view changes. After dropping
    // the server, await the worker's terminal notification and runtime cleanup.
    while notifications.try_recv().is_ok() {}
    let monitor = server.status_monitor();
    drop(server);
    assert!(session.is_revoked());
    tokio::time::timeout(Duration::from_secs(3), notifications.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(monitor.status(), ServerStatus::Stopped);
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port)).await {
            Err(_) => break,
            Ok(_) if Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(5)).await
            }
            Ok(_) => panic!("Revocation did not close the listener"),
        }
    }
}

#[tokio::test]
async fn transport_rejects_wrong_tokens_hosts_origins_and_oversized_bodies() {
    let (session, _) = common::session();
    let server = LocalServer::start(session, Arc::new(|| {})).unwrap();
    let port = listening(&server).await;
    let token = server.token_for_configuration();
    let client = client();
    for bad in ["", "invalid", &"0".repeat(64)] {
        let response = request(&client, port, bad, "tools/list", json!({}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
    for (header, value) in [
        ("Host", "attacker.invalid"),
        ("Host", "127.0.0.1:1"),
        ("Origin", "https://attacker.invalid"),
        ("Origin", "null"),
        ("Origin", "http://127.0.0.1:1"),
    ] {
        let response = request(&client, port, token, "tools/list", json!({}))
            .header(header, value)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{header}");
    }
    let response = request(
        &client,
        port,
        token,
        "tools/list",
        json!({"oversized":"x".repeat(70_000)}),
    )
    .send()
    .await
    .unwrap();
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let response = request(&client, port, token, "tools/list", json!({}))
        .header("Origin", format!("http://127.0.0.1:{port}"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}
