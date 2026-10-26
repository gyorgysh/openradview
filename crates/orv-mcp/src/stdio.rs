// SPDX-License-Identifier: GPL-3.0-or-later
//! Stdio adapter for a session already consented in the running desktop app.
//! It cannot open files, create a grant, redirect requests or use system proxies.
use rmcp::{
    ErrorData, Peer, RoleClient, RoleServer, ServerHandler, ServiceExt,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
        ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig,
    },
    service::RequestContext,
    transport::async_rw::AsyncRwTransport,
};
use zeroize::Zeroizing;

/// Exact canonical loopback URLs only. No DNS, alternate IP notation, userinfo,
/// query/fragment, encoded paths or redirects can widen the transport boundary.
pub fn validate_endpoint(url: &str) -> Result<u16, &'static str> {
    let port = url
        .strip_prefix("http://127.0.0.1:")
        .and_then(|v| v.strip_suffix("/mcp"))
        .and_then(|v| v.parse::<u16>().ok())
        .filter(|v| *v != 0)
        .ok_or("Use the exact http://127.0.0.1:PORT/mcp endpoint from the desktop app")?;
    if url != format!("http://127.0.0.1:{port}/mcp") {
        return Err("The local endpoint is not canonical");
    }
    Ok(port)
}

#[derive(Clone)]
struct Bridge {
    upstream: Peer<RoleClient>,
}

impl ServerHandler for Bridge {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("OpenRadView stdio bridge", env!("CARGO_PKG_VERSION")))
            .with_instructions("Experimental research viewer. Access is limited to the live desktop session grant. No filesystem access; revocation in the desktop app disables all tools.")
    }

    async fn list_tools(
        &self,
        request: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        self.upstream
            .list_tools(request)
            .await
            .map_err(|_| ErrorData::internal_error("Desktop session unavailable or revoked", None))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        // Do not forward the caller's protocol metadata/opaque state upstream.
        let mut clean = CallToolRequestParams::new(request.name);
        clean.arguments = request.arguments;
        match self.upstream.call_tool_once(clean).await {
            Ok(response) => Ok(response),
            Err(_) => Ok(CallToolResult::error(vec![ContentBlock::text(
                "Desktop session unavailable, request denied or access revoked",
            )])
            .into()),
        }
    }
}

pub async fn serve(url: String, token: Zeroizing<String>) -> Result<(), &'static str> {
    let upstream = crate::local_client::LocalClient::connect(url, &token).await?;
    let bridge = Bridge {
        upstream: upstream.peer().clone(),
    };
    let bounded_stdio = AsyncRwTransport::new_server(tokio::io::stdin(), tokio::io::stdout())
        .with_max_line_length(65_536);
    let downstream = bridge
        .serve(bounded_stdio)
        .await
        .map_err(|_| "Could not initialize stdio transport")?;
    tokio::select! {
        _ = upstream.waiting() => Err("Desktop session ended"),
        _ = downstream.waiting() => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn endpoint_cannot_escape_loopback() {
        assert_eq!(validate_endpoint("http://127.0.0.1:1234/mcp"), Ok(1234));
        for url in [
            "https://127.0.0.1:1234/mcp",
            "http://localhost:1234/mcp",
            "http://127.1:1234/mcp",
            "http://2130706433:1234/mcp",
            "http://127.0.0.1:1234/mcp?x=1",
            "http://127.0.0.1:1234/mcp#x",
            "http://127.0.0.1:0/mcp",
            "http://127.0.0.1:01234/mcp",
            "http://127.0.0.1:1234/mcp/",
            "http://127.0.0.1:1234@remote.invalid/mcp",
            "http://127.0.0.1:1234/%6dcp",
        ] {
            assert!(validate_endpoint(url).is_err(), "{url}");
        }
    }
}
