// SPDX-License-Identifier: GPL-3.0-or-later
//! Local manual smoke test. Open generated data, pass the bundled companion
//! path, approve its request in the app, then press Return here. No provider,
//! token output, disk image export or automatic re-request after denial.
use rmcp::{
    ServiceExt,
    model::{CallToolRequestParams, ClientConfig},
};
use std::process::Stdio;
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::Command,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let executable = std::env::args_os()
        .nth(1)
        .ok_or("Pass the MCP companion path; use generated study data only")?;
    let mut child = Command::new(executable)
        .arg("connect")
        .env_remove("OPENRADVIEW_TOKEN")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    let mut client = ClientConfig::default()
        .serve((
            child.stdout.take().ok_or("No protocol output")?,
            child.stdin.take().ok_or("No protocol input")?,
        ))
        .await?;
    let access = || {
        CallToolRequestParams::new("request_access").with_arguments(
            serde_json::json!({"recipient":"Synthetic local snapshot check"})
                .as_object()
                .cloned()
                .unwrap_or_default(),
        )
    };
    let first = client.call_tool(access()).await?;
    if first.is_error == Some(true) {
        return Err("Request denied or unavailable; do not retry automatically".into());
    }
    println!(
        "Review only your generated study in OpenRadView. Choose Allow after its preview is ready, then press Return here."
    );
    let mut line = String::new();
    BufReader::new(tokio::io::stdin())
        .read_line(&mut line)
        .await?;
    let approved = client.call_tool(access()).await?;
    if approved
        .structured_content
        .as_ref()
        .and_then(|v| v["status"].as_str())
        != Some("approved")
    {
        return Err("No approved connection; access remains private".into());
    }
    let study = client
        .call_tool(CallToolRequestParams::new("open_study").with_arguments(Default::default()))
        .await?;
    if study.is_error == Some(true) {
        return Err("Study scope unavailable".into());
    }
    let count = study
        .structured_content
        .as_ref()
        .and_then(|s| s["series"]["image_count"].as_u64())
        .ok_or("Missing scope")?;
    let snapshot = client
        .call_tool(CallToolRequestParams::new("render_snapshot").with_arguments(Default::default()))
        .await?;
    if snapshot.is_error == Some(true) {
        return Err("Snapshot refused or unavailable".into());
    }
    let value = serde_json::to_value(&snapshot)?;
    let image = value["content"]
        .as_array()
        .and_then(|items| items.iter().find(|v| v["type"] == "image"))
        .ok_or("No image content")?;
    let manifest = snapshot
        .structured_content
        .as_ref()
        .ok_or("No image manifest")?;
    println!(
        "Approved series images: {count}; PNG base64 bytes: {}; dimensions: {}",
        image["data"].as_str().ok_or("Missing image bytes")?.len(),
        manifest["image"]["dimensions"]
    );
    println!(
        "Revoke this synthetic session in the app when finished. No pixels/tokens were printed or exported."
    );
    client.close().await?;
    Ok(())
}
