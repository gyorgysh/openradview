// SPDX-License-Identifier: GPL-3.0-or-later
//! Session-owned, authenticated IPv4 loopback transport. No outbound connections.
use crate::{
    comparison::ComparisonTools,
    consent::{BearerToken, Session},
    tools::ViewerTools,
};
use axum::{
    Router,
    extract::{Request, State},
    http::{StatusCode, header},
    middleware::{self, Next},
    response::Response,
};
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
enum Access {
    Viewer(Session),
    Comparison(orv_agents::Recipient),
}
impl Access {
    fn is_revoked(&self) -> bool {
        match self {
            Self::Viewer(s) => s.is_revoked(),
            Self::Comparison(s) => s.is_revoked(),
        }
    }
    fn cancellation(&self) -> tokio_util::sync::CancellationToken {
        match self {
            Self::Viewer(s) => s.cancellation(),
            Self::Comparison(s) => s.cancellation(),
        }
    }
    fn revoke(&self) {
        match self {
            Self::Viewer(s) => s.revoke(),
            Self::Comparison(s) => s.revoke(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServerStatus {
    Starting,
    Listening { port: u16 },
    Stopped,
    Failed(&'static str),
}

/// Read-only lifecycle state, which can outlive the server owner after revocation.
#[derive(Clone)]
pub struct ServerStatusMonitor(Arc<Mutex<ServerStatus>>);
impl ServerStatusMonitor {
    pub fn status(&self) -> ServerStatus {
        self.0
            .lock()
            .map(|status| status.clone())
            .unwrap_or(ServerStatus::Failed("Local transport status unavailable"))
    }
}

/// Owned by the desktop UI. Dropping it revokes access, including cloned tools.
/// Startup/polling never waits for GPU work, client requests or thread shutdown.
pub struct LocalServer {
    session: Access,
    token: Arc<BearerToken>,
    status: Arc<Mutex<ServerStatus>>,
}

impl LocalServer {
    pub fn start(
        session: Session,
        notify_ui: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Self, &'static str> {
        Self::start_access(Access::Viewer(session), notify_ui)
    }
    /// Only a desktop-owned, already-consented recipient can start a listener.
    pub fn start_comparison(
        recipient: orv_agents::Recipient,
        notify_ui: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Self, &'static str> {
        Self::start_access(Access::Comparison(recipient), notify_ui)
    }
    fn start_access(
        session: Access,
        notify_ui: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Self, &'static str> {
        if session.is_revoked() {
            return Err("Cannot start a revoked session");
        }
        let token = Arc::new(BearerToken::generate()?);
        let status = Arc::new(Mutex::new(ServerStatus::Starting));
        let worker_session = session.clone();
        let worker_token = token.clone();
        let worker_status = status.clone();
        std::thread::Builder::new().name("orv-local-mcp".into()).spawn(move || {
            let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build();
            let result = runtime.as_ref().map_err(|_| "Could not start local transport runtime").and_then(|runtime| {
                runtime.block_on(async {
                    if worker_session.is_revoked() { return Ok(()); }
                    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await
                        .map_err(|_| "Could not bind local agent access")?;
                    let port = listener.local_addr().map_err(|_| "Could not read local listener address")?.port();
                    let router = router(worker_session.clone(), worker_token, port, notify_ui.clone());
                    if worker_session.is_revoked() { return Ok(()); }
                    set_status(&worker_status, ServerStatus::Listening { port });
                    notify_ui();
                    let cancellation = worker_session.cancellation();
                    // Drop the serving future on revocation, closing accepts and
                    // connections. Do not wait for a client to finish its request.
                    tokio::select! {
                        _ = cancellation.cancelled() => Ok(()),
                        value = axum::serve(listener, router) => value.map_err(|_| "Local agent transport stopped"),
                    }
                })
            });
            worker_session.revoke();
            // Finish bounded runtime shutdown before publishing terminal status.
            // Revocation itself remains immediate and never waits on the UI.
            if let Ok(runtime) = runtime { runtime.shutdown_timeout(std::time::Duration::from_secs(1)); }
            set_status(&worker_status, match result { Ok(()) => ServerStatus::Stopped, Err(e) => ServerStatus::Failed(e) });
            notify_ui();
        }).map_err(|_| "Could not start local transport worker")?;
        Ok(Self {
            session,
            token,
            status,
        })
    }
    pub fn status(&self) -> ServerStatus {
        self.status
            .lock()
            .map(|s| s.clone())
            .unwrap_or(ServerStatus::Failed("Local transport status unavailable"))
    }
    pub fn status_monitor(&self) -> ServerStatusMonitor {
        ServerStatusMonitor(self.status.clone())
    }
    /// Use only in an explicit user copy/configuration action.
    pub fn token_for_configuration(&self) -> &str {
        self.token.expose_for_configuration()
    }
    pub fn revoke(&self) {
        self.session.revoke();
    }
}
impl Drop for LocalServer {
    fn drop(&mut self) {
        self.session.revoke();
    }
}

fn set_status(status: &Mutex<ServerStatus>, value: ServerStatus) {
    if let Ok(mut status) = status.lock() {
        *status = value;
    }
}

#[derive(Clone)]
struct Authentication {
    session: Access,
    token: Arc<BearerToken>,
    authority: String,
    requests: Arc<tokio::sync::Semaphore>,
}

fn router(
    session: Access,
    token: Arc<BearerToken>,
    port: u16,
    notify_ui: Arc<dyn Fn() + Send + Sync>,
) -> Router {
    let authority = format!("127.0.0.1:{port}");
    let config = StreamableHttpServerConfig::default()
        .with_legacy_session_mode(false)
        .with_json_response(true)
        .with_allowed_hosts(vec![authority.clone()])
        .with_allowed_origins(vec![format!("http://{authority}")])
        .with_max_request_body_bytes(65_536)
        .with_cancellation_token(session.cancellation());
    // Separate routers ensure comparison tokens never expose viewer capabilities.
    let router = match session.clone() {
        Access::Viewer(session) => {
            let tools = ViewerTools::new(session, notify_ui);
            let service: StreamableHttpService<ViewerTools, LocalSessionManager> =
                StreamableHttpService::new(move || Ok(tools.clone()), Default::default(), config);
            Router::new().nest_service("/mcp", service)
        }
        Access::Comparison(recipient) => {
            let tools = ComparisonTools::new(recipient, notify_ui);
            let service: StreamableHttpService<ComparisonTools, LocalSessionManager> =
                StreamableHttpService::new(move || Ok(tools.clone()), Default::default(), config);
            Router::new().nest_service("/mcp", service)
        }
    };
    router.layer(middleware::from_fn_with_state(
        Authentication {
            session,
            token,
            authority,
            requests: Arc::new(tokio::sync::Semaphore::new(4)),
        },
        authenticate,
    ))
}

async fn authenticate(
    State(auth): State<Authentication>,
    request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    if auth.session.is_revoked() {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let headers = request.headers();
    if headers.get_all(header::HOST).iter().count() != 1
        || headers.get(header::HOST).and_then(|h| h.to_str().ok()) != Some(auth.authority.as_str())
        || headers.get_all(header::ORIGIN).iter().count() > 1
    {
        return Err(StatusCode::FORBIDDEN);
    }
    if headers.get_all(header::AUTHORIZATION).iter().count() != 1 {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .ok_or(StatusCode::UNAUTHORIZED)?;
    if !auth.token.matches(token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let _permit = auth
        .requests
        .try_acquire()
        .map_err(|_| StatusCode::TOO_MANY_REQUESTS)?;
    let response = next.run(request).await;
    if auth.session.is_revoked() {
        return Err(StatusCode::UNAUTHORIZED);
    }
    Ok(response)
}
