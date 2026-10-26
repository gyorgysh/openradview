// SPDX-License-Identifier: GPL-3.0-or-later
//! Reusable stdio MCP entry point. No grant or token is stored in client settings.
use crate::{
    broker::{AccessRequest, Reply},
    consent::BearerToken,
    discovery::{Instance, Registry},
    local_client::LocalClient,
};
use rmcp::{
    ErrorData, RoleServer, ServerHandler, ServiceExt,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
        ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool,
    },
    service::RequestContext,
    transport::async_rw::AsyncRwTransport,
};
use serde_json::json;
use std::{sync::Arc, time::Duration};
use tokio::sync::Mutex;

pub fn validate_endpoint(url: &str) -> Result<(), &'static str> {
    let port = url
        .strip_suffix("/request")
        .and_then(|s| s.strip_prefix("http://127.0.0.1:"))
        .and_then(|s| s.parse::<u16>().ok())
        .filter(|p| *p != 0)
        .ok_or("Invalid local approval endpoint")?;
    if url != format!("http://127.0.0.1:{port}/request") {
        return Err("Invalid local approval endpoint");
    }
    Ok(())
}
struct Connection {
    capability: BearerToken,
    requested: bool,
    upstream: Option<LocalClient>,
    selected: Option<Target>,
}
#[derive(Clone)]
struct Target {
    url: String,
    instance: Option<Instance>,
}
#[derive(Clone)]
enum Discovery {
    Automatic(Registry),
    Explicit(String),
}
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Access {
    recipient: String,
    instance_id: Option<String>,
}
#[derive(Clone)]
struct Connector {
    discovery: Discovery,
    http: reqwest::Client,
    state: Arc<Mutex<Connection>>,
}
impl Connector {
    fn new(url: Option<String>) -> Result<Self, &'static str> {
        let discovery = match url {
            Some(url) => {
                validate_endpoint(&url)?;
                Discovery::Explicit(url)
            }
            None => Discovery::Automatic(Registry::local()?),
        };
        let http = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(2))
            .timeout(Duration::from_secs(3))
            .build()
            .map_err(|_| "Could not initialize the local connector")?;
        Ok(Self {
            discovery,
            http,
            state: Arc::new(Mutex::new(Connection {
                capability: BearerToken::generate()?,
                requested: false,
                upstream: None,
                selected: None,
            })),
        })
    }
    fn list(&self) -> Result<Vec<Instance>, &'static str> {
        match &self.discovery {
            Discovery::Automatic(r) => r.list(),
            Discovery::Explicit(_) => Ok(Vec::new()),
        }
    }
    fn select(&self, id: Option<&str>) -> Result<Target, &'static str> {
        match &self.discovery {
            Discovery::Explicit(url) if id.is_none() => Ok(Target {
                url: url.clone(),
                instance: None,
            }),
            Discovery::Explicit(_) => {
                Err("instance_id cannot be combined with a manual --broker-url")
            }
            Discovery::Automatic(r) => {
                let instances = r.list()?;
                let instance = if let Some(id) = id {
                    instances
                        .into_iter()
                        .find(|i| i.instance_id == id)
                        .ok_or("Selected instance is unavailable; call list_instances again")?
                } else {
                    if instances.len() > 1 {
                        return Err(
                            "Several apps are running; call list_instances, ask which app to use, then pass its instance_id to request_access",
                        );
                    }
                    instances.into_iter().next().ok_or("Open OpenRadView, enter the workspace and enable connection requests in Agents")?
                };
                Ok(Target {
                    url: instance.endpoint(),
                    instance: Some(instance),
                })
            }
        }
    }
    async fn request(
        &self,
        request: CallToolRequestParams,
    ) -> Result<CallToolResult, &'static str> {
        let access: Access = serde_json::from_value(serde_json::Value::Object(
            request.arguments.unwrap_or_default(),
        ))
        .map_err(|_| "request_access requires a recipient label and optional instance_id")?;
        if access.recipient.trim().is_empty()
            || access.recipient.len() > 128
            || access.recipient.chars().any(char::is_control)
        {
            return Err("Use a recipient label of 1–128 bytes without control characters");
        }
        if access
            .instance_id
            .as_ref()
            .is_some_and(|id| !orv_core::scene::is_sha256(id))
        {
            return Err("Invalid instance_id; call list_instances first");
        }
        let mut state = self
            .state
            .try_lock()
            .map_err(|_| "Connector busy; retry after the current request")?;
        if let (Some(id), Some(target)) = (&access.instance_id, &state.selected)
            && target
                .instance
                .as_ref()
                .is_none_or(|i| &i.instance_id != id)
        {
            return Err(
                "This connector is already bound to another instance; finish or revoke that session and reconnect before changing instances",
            );
        }
        if let Some(client) = &state.upstream {
            if client
                .call("open_study", Default::default())
                .await
                .is_ok_and(|r| r.is_error != Some(true))
            {
                return Ok(CallToolResult::structured(
                    json!({"status":"approved", "next":"Call open_study for the approved scope and current scene."}),
                ));
            }
            state.upstream = None;
            state.requested = false;
            state.capability = BearerToken::generate()?;
            state.selected = None;
        }
        if state.selected.is_none() {
            state.selected = Some(self.select(access.instance_id.as_deref())?);
        }
        let target = state.selected.clone().ok_or("No app instance selected")?;
        if let Some(instance) = &target.instance {
            let response = self
                .http
                .get(format!("http://127.0.0.1:{}/instance", instance.port))
                .bearer_auth(state.capability.expose_for_configuration())
                .send()
                .await;
            let identity = match response {
                Ok(r) if r.status().is_success() => body::<Instance>(r).await.ok(),
                _ => None,
            };
            if identity.as_ref() != Some(instance) {
                state.selected = None;
                state.requested = false;
                state.capability = BearerToken::generate()?;
                return Err(
                    "The selected app stopped or changed; call list_instances before requesting a fresh approval",
                );
            }
        }
        let builder = if state.requested {
            self.http.get(&target.url)
        } else {
            self.http.post(&target.url).json(&AccessRequest {
                recipient: access.recipient,
            })
        };
        let response = builder.bearer_auth(state.capability.expose_for_configuration()).send().await
            .map_err(|_| "Open the MCP edition of OpenRadView, enter the workspace and enable connection requests in Agents")?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            state.requested = false;
            return Err(
                "Request ended or expired; call request_access again for a new desktop approval",
            );
        }
        if !response.status().is_success() {
            return Err(
                "Desktop request unavailable or busy; review any current request in the app before retrying",
            );
        }
        let reply: Reply = body(response).await?;
        state.requested = true;
        match reply {
            Reply::Pending => Ok(CallToolResult::structured(
                json!({"status":"pending", "next":"Ask the user to review Allow agent access in OpenRadView. Do not poll repeatedly; call request_access again after they respond. No data has been shared."}),
            )),
            Reply::Denied => {
                state.requested = false;
                Err(
                    "The user denied access or the request expired. Do not retry without their instruction.",
                )
            }
            Reply::Approved { url, token } => {
                let token = zeroize::Zeroizing::new(token);
                state.upstream = Some(LocalClient::connect(url, &token).await?);
                Ok(CallToolResult::structured(
                    json!({"status":"approved", "next":"Call open_study and list_series to inspect the exact session scope, then render_snapshot. Revoke or a study/series change ends access."}),
                ))
            }
        }
    }
}
impl ServerHandler for Connector {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("OpenRadView connector", env!("CARGO_PKG_VERSION")))
            .with_instructions(format!("{}\n\n{}", "Call list_instances to discover local apps, then request_access (with instance_id if several are running). The user must allow this session in the desktop app. Images and metadata are unavailable before approval. No tool provides arbitrary filesystem access.", crate::guidance::AGENT_GUIDANCE))
    }
    async fn list_tools(
        &self,
        _: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let mut tools = crate::tools::ViewerTools::definitions();
        let schema = json!({"type":"object", "properties":{"instance_id":{"type":"string", "pattern":"^[a-f0-9]{64}$"},"recipient":{"type":"string", "minLength":1, "maxLength":128}}, "required":["recipient"], "additionalProperties":false});
        tools.insert(0, Tool::new("request_access", "Request explicit desktop approval for this local client/session. Call again after the user responds to collect approval; no patient data is returned before Allow.", schema.as_object().cloned().unwrap_or_default()));
        tools.insert(0, Tool::new("list_instances", "List running local OpenRadView app identities, versions, process IDs and approval ports. No study data, paths or session tokens. Use instance_id to select when several apps run.", json!({"type":"object","properties":{},"additionalProperties":false}).as_object().cloned().unwrap_or_default()));
        Ok(ListToolsResult::with_all_items(tools))
    }
    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let result = if request.name == "list_instances" {
            if request.arguments.as_ref().is_some_and(|a| !a.is_empty()) {
                Err("list_instances takes no arguments")
            } else {
                self.list()
                    .map(|instances| CallToolResult::structured(json!({"instances":instances})))
            }
        } else if request.name == "request_access" {
            self.request(request).await
        } else {
            let state = self.state.try_lock();
            match state {
                Err(_) => Err("Connector busy; retry after the current request"),
                Ok(state) => match &state.upstream {
                    None => Err(
                        "Access is private. Call request_access and wait for explicit approval in OpenRadView first.",
                    ),
                    Some(client) => {
                        client
                            .call(&request.name, request.arguments.unwrap_or_default())
                            .await
                    }
                },
            }
        };
        Ok(result
            .unwrap_or_else(|e| CallToolResult::error(vec![ContentBlock::text(e)]))
            .into())
    }
}

// Bound every local response, including identity probes. No redirects/proxies.
async fn body<T: serde::de::DeserializeOwned>(
    mut response: reqwest::Response,
) -> Result<T, &'static str> {
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "Approval response unavailable")?
    {
        if bytes.len() + chunk.len() > 4096 {
            return Err("Approval response exceeded its limit");
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| "Invalid approval response")
}

pub async fn serve(url: Option<String>) -> Result<(), &'static str> {
    let connector = Connector::new(url)?;
    let bounded = AsyncRwTransport::new_server(tokio::io::stdin(), tokio::io::stdout())
        .with_max_line_length(65_536);
    let service = connector
        .serve(bounded)
        .await
        .map_err(|_| "Could not initialize stdio connector")?;
    service
        .waiting()
        .await
        .map_err(|_| "Local connector ended")?;
    Ok(())
}

#[cfg(test)]
mod tests;
