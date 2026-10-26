// SPDX-License-Identifier: GPL-3.0-or-later
//! MCP tools for one recipient of a fixed comparison. No study/view capabilities.
use base64::{Engine, engine::general_purpose::STANDARD};
use orv_agents::{AgentReply, Recipient};
use rmcp::{
    ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{
        CallToolResult, ContentBlock as Content, Implementation, ServerCapabilities, ServerConfig,
    },
    tool, tool_handler, tool_router,
};
use serde::Deserialize;
use std::sync::Arc;

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EmptyRequest {}

#[derive(Clone)]
pub struct ComparisonTools {
    recipient: Recipient,
    notify_ui: Arc<dyn Fn() + Send + Sync>,
    tool_router: ToolRouter<Self>,
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for ComparisonTools {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("OpenRadView comparison", env!("CARGO_PKG_VERSION")))
            .with_instructions(format!("{}\n\n{}", "Read the one frozen comparison snapshot and its manifest, then submit plain experimental AI commentary with the exact scene/PNG hashes. Do not request other files or views. No other recipient's reply is available.", crate::guidance::AGENT_GUIDANCE))
    }
}

#[tool_router(router = tool_router)]
impl ComparisonTools {
    pub fn new(recipient: Recipient, notify_ui: Arc<dyn Fn() + Send + Sync>) -> Self {
        Self {
            recipient,
            notify_ui,
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        description = "Read the sole explicitly reviewed, frozen PNG and SceneState/geometry/hash manifest. Identical bytes are supplied to every recipient; no source files, other slices or live view control."
    )]
    pub fn comparison_snapshot(&self, Parameters(_): Parameters<EmptyRequest>) -> CallToolResult {
        let result = (|| {
            let shared = self.recipient.read_snapshot()?;
            let snapshot = shared.snapshot();
            let manifest = serde_json::to_value(&snapshot.manifest)
                .map_err(|_| "Could not encode comparison manifest")?;
            let mut response = CallToolResult::success(vec![
                Content::image(STANDARD.encode(&snapshot.png), "image/png"),
                Content::json(&manifest).map_err(|_| "Could not encode comparison manifest")?,
            ]);
            response.structured_content = Some(manifest);
            if self.recipient.is_revoked() {
                return Err("Comparison access has been revoked");
            }
            (self.notify_ui)();
            Ok(response)
        })();
        result.unwrap_or_else(failure)
    }

    #[tool(
        description = "Submit one final plain-text experimental AI commentary about the snapshot you read. Include its exact scene_sha256/png_sha256 and your self-reported model label. Maximum 32768 UTF-8 bytes; identical retries are allowed, replacements are denied. The desktop treats all commentary as untrusted text."
    )]
    pub fn submit_commentary(&self, Parameters(reply): Parameters<AgentReply>) -> CallToolResult {
        match self.recipient.submit(reply) {
            Ok(()) => {
                (self.notify_ui)();
                CallToolResult::structured(
                    serde_json::json!({"accepted":true,"label":"experimental AI commentary"}),
                )
            }
            Err(error) => failure(error),
        }
    }
}

fn failure(message: &'static str) -> CallToolResult {
    CallToolResult::error(vec![Content::text(message)])
}
