// SPDX-License-Identifier: GPL-3.0-or-later
//! Shared loopback-only client for the stdio companion and local integrations.
use rmcp::{
    Peer, RoleClient, ServiceExt,
    model::{CallToolRequestParams, CallToolResult, ClientConfig},
    service::RunningService,
    transport::streamable_http_client::{
        StreamableHttpClientTransport, StreamableHttpClientTransportConfig,
    },
};
use std::time::Duration;

pub struct LocalClient {
    service: RunningService<RoleClient, ClientConfig>,
}
impl LocalClient {
    pub async fn connect(url: String, token: &str) -> Result<Self, &'static str> {
        crate::stdio::validate_endpoint(&url)?;
        if !orv_core::scene::is_sha256(token) {
            return Err("A valid session token is required");
        }
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(2))
            .timeout(Duration::from_secs(20))
            .build()
            .map_err(|_| "Could not create local transport client")?;
        let config = StreamableHttpClientTransportConfig::with_uri(url)
            .auth_header(token)
            .max_concurrent_requests(1);
        let transport = StreamableHttpClientTransport::with_client(client, config);
        let service = ClientConfig::default()
            .serve(transport)
            .await
            .map_err(|_| "Desktop session unavailable or access denied")?;
        Ok(Self { service })
    }
    pub async fn call(
        &self,
        name: &str,
        arguments: serde_json::Map<String, serde_json::Value>,
    ) -> Result<CallToolResult, &'static str> {
        self.service
            .call_tool(CallToolRequestParams::new(name.to_owned()).with_arguments(arguments))
            .await
            .map_err(|_| "Desktop request unavailable, denied or revoked")
    }
    pub(crate) fn peer(&self) -> &Peer<RoleClient> {
        self.service.peer()
    }
    pub(crate) async fn waiting(self) {
        let _ = self.service.waiting().await;
    }
    pub async fn close(&mut self) {
        let _ = self.service.close().await;
    }
}
