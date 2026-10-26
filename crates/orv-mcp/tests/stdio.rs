// SPDX-License-Identifier: GPL-3.0-or-later
mod common;
use orv_mcp::http::{LocalServer, ServerStatus};
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

#[tokio::test]
async fn real_stdio_companion_uses_the_existing_grant_and_cannot_survive_revoke() {
    let (session, _) = common::session();
    let server = LocalServer::start(session.clone(), Arc::new(|| {})).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let port = loop {
        match server.status() {
            ServerStatus::Listening { port } => break port,
            ServerStatus::Starting if Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(5)).await
            }
            s => panic!("Transport did not start: {s:?}"),
        }
    };
    let mut child = Command::new(env!("CARGO_BIN_EXE_openradview-mcp"))
        .args(["stdio", "--url", &format!("http://127.0.0.1:{port}/mcp")])
        .env("OPENRADVIEW_TOKEN", server.token_for_configuration())
        .env("HTTP_PROXY", "http://198.51.100.1:9")
        .env("ALL_PROXY", "http://198.51.100.1:9")
        .env("NO_PROXY", "")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let transport = (child.stdout.take().unwrap(), child.stdin.take().unwrap());
    let mut client = tokio::time::timeout(
        Duration::from_secs(5),
        ClientConfig::default().serve(transport),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(client.list_all_tools().await.unwrap().len(), 7);
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
    let after = tokio::time::timeout(
        Duration::from_secs(3),
        client.call_tool(CallToolRequestParams::new("get_tags").with_arguments(Default::default())),
    )
    .await
    .unwrap();
    assert!(after.is_err() || after.unwrap().is_error == Some(true));
    client.close().await.unwrap();
    tokio::time::timeout(Duration::from_secs(3), child.wait())
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn companion_rejects_non_loopback_without_disclosing_arguments() {
    let output = Command::new(env!("CARGO_BIN_EXE_openradview-mcp"))
        .args(["stdio", "--url", "https://private-example.invalid/secret"])
        .env("OPENRADVIEW_TOKEN", "private-token-must-not-be-logged")
        .output()
        .await
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(!stderr.contains("private-example") && !stderr.contains("private-token"));
}
