// SPDX-License-Identifier: GPL-3.0-or-later
mod common;
use orv_mcp::{
    broker::{Reply, RequestBroker},
    consent::BearerToken,
    http::{LocalServer, ServerStatus},
};
use rmcp::{
    ServiceExt,
    model::{CallToolRequestParams, ClientConfig},
};
use std::{
    process::Stdio,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::process::Command;

async fn listening(status: impl Fn() -> ServerStatus) -> u16 {
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        match status() {
            ServerStatus::Listening { port } => return port,
            ServerStatus::Starting if Instant::now() < until => {
                tokio::time::sleep(Duration::from_millis(5)).await
            }
            s => panic!("Listener failed: {s:?}"),
        }
    }
}

#[tokio::test]
async fn approval_endpoint_cannot_leak_grants_to_browsers_or_another_requester() {
    let broker = RequestBroker::start_on(0, Arc::new(|| {})).unwrap();
    let port = listening(|| broker.status()).await;
    let url = format!("http://127.0.0.1:{port}/request");
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let capability = BearerToken::generate().unwrap();
    let key = capability.expose_for_configuration();
    let post = || {
        http.post(&url)
            .bearer_auth(key)
            .json(&serde_json::json!({"recipient":"Synthetic test client"}))
    };
    assert_eq!(
        post()
            .header("Origin", "http://127.0.0.1")
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(
        post()
            .header("Host", "hostile.invalid")
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(
        http.post(&url)
            .json(&serde_json::json!({"recipient":"test"}))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    assert_eq!(
        http.post(&url)
            .bearer_auth(key)
            .header("Content-Type", "text/plain")
            .body("{}")
            .send()
            .await
            .unwrap()
            .status(),
        415
    );
    assert_eq!(
        post().body("x".repeat(4097)).send().await.unwrap().status(),
        413
    );
    let result = post()
        .send()
        .await
        .unwrap()
        .json::<serde_json::Value>()
        .await
        .unwrap();
    assert_eq!(result, serde_json::json!({"status":"pending"}));
    let other = BearerToken::generate().unwrap();
    assert_eq!(
        http.get(&url)
            .bearer_auth(other.expose_for_configuration())
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    assert_eq!(
        http.post(&url)
            .bearer_auth(other.expose_for_configuration())
            .json(&serde_json::json!({"recipient":"Other"}))
            .send()
            .await
            .unwrap()
            .status(),
        409
    );
    let pending = broker.take_request().unwrap();
    assert_eq!(pending.recipient, "Synthetic test client");
    assert!(broker.take_request().is_none());
    assert!(broker.is_pending(pending.id));
    let (session, _) = common::session();
    let server = LocalServer::start(session.clone(), Arc::new(|| {})).unwrap();
    let port = listening(|| server.status()).await;
    broker
        .approve(
            pending.id,
            session.clone(),
            format!("http://127.0.0.1:{port}/mcp"),
            server.token_for_configuration(),
        )
        .unwrap();
    assert_eq!(
        http.get(&url)
            .bearer_auth(other.expose_for_configuration())
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    // Revoking before collection must discard even an approved token.
    session.revoke();
    assert!(matches!(
        http.get(&url)
            .bearer_auth(key)
            .send()
            .await
            .unwrap()
            .json::<Reply>()
            .await
            .unwrap(),
        Reply::Denied
    ));
    assert_eq!(
        http.get(&url)
            .bearer_auth(key)
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
}

#[tokio::test]
async fn reusable_stdio_connector_requires_app_approval_then_revoke_disables_tools() {
    let broker = RequestBroker::start_on(0, Arc::new(|| {})).unwrap();
    let port = listening(|| broker.status()).await;
    let mut child = Command::new(env!("CARGO_BIN_EXE_openradview-mcp"))
        .args([
            "connect",
            "--broker-url",
            &format!("http://127.0.0.1:{port}/request"),
        ])
        .env_remove("OPENRADVIEW_TOKEN")
        .env("ALL_PROXY", "http://198.51.100.1:9")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let transport = (child.stdout.take().unwrap(), child.stdin.take().unwrap());
    let mut client = ClientConfig::default().serve(transport).await.unwrap();
    let tools = client.list_all_tools().await.unwrap();
    assert_eq!(tools.len(), 9);
    for name in [
        "open_study",
        "list_series",
        "get_tags",
        "render_snapshot",
        "set_view",
        "measure",
        "roi_stats",
    ] {
        let response = client
            .call_tool(CallToolRequestParams::new(name).with_arguments(Default::default()))
            .await
            .unwrap();
        assert_eq!(
            response.is_error,
            Some(true),
            "{name} must not read data before Allow"
        );
        assert!(
            broker.take_request().is_none(),
            "Data tools cannot request approval implicitly"
        );
    }
    let request = || {
        CallToolRequestParams::new("request_access").with_arguments(
            serde_json::json!({"recipient":"Synthetic local agent"})
                .as_object()
                .unwrap()
                .clone(),
        )
    };
    let pending = client.call_tool(request()).await.unwrap();
    assert_eq!(pending.structured_content.unwrap()["status"], "pending");
    let review = broker.take_request().unwrap();
    assert_eq!(review.recipient, "Synthetic local agent");
    let (session, _) = common::session();
    let server = LocalServer::start(session.clone(), Arc::new(|| {})).unwrap();
    let port = listening(|| server.status()).await;
    broker
        .approve(
            review.id,
            session.clone(),
            format!("http://127.0.0.1:{port}/mcp"),
            server.token_for_configuration(),
        )
        .unwrap();
    let approved = client.call_tool(request()).await.unwrap();
    assert_eq!(approved.structured_content.unwrap()["status"], "approved");
    let tags = client
        .call_tool(CallToolRequestParams::new("get_tags").with_arguments(Default::default()))
        .await
        .unwrap();
    assert_ne!(tags.is_error, Some(true));
    assert_eq!(
        tags.structured_content.unwrap()["image"]["dimensions"],
        serde_json::json!([256, 256])
    );
    session.revoke();
    let after = client
        .call_tool(CallToolRequestParams::new("open_study").with_arguments(Default::default()))
        .await
        .unwrap();
    assert_eq!(after.is_error, Some(true));
    client.close().await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), child.wait())
        .await
        .unwrap()
        .unwrap();
}
