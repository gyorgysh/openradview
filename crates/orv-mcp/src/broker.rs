// SPDX-License-Identifier: GPL-3.0-or-later
//! Approval-only rendezvous. This listener has no study/catalog/renderer access.
//! A requester chooses an unguessable capability; only its holder can collect
//! credentials created by a later, explicit desktop approval. Grants never persist.
//! Discovery records contain only transient app identity and a loopback port.
use crate::{consent::Session, http::ServerStatus};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Request, State},
    http::{HeaderMap, StatusCode, header},
    middleware::{self, Next},
    response::Response,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use std::{
    future::IntoFuture,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use subtle::ConstantTimeEq;
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;

const LIFETIME: Duration = Duration::from_secs(180);

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AccessRequest {
    /// Self-reported client label, not a verified account identity.
    pub recipient: String,
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum Reply {
    Pending,
    Approved { url: String, token: String },
    Denied,
}

pub struct PendingRequest {
    pub id: u64,
    pub recipient: String,
}
struct Grant {
    session: Session,
    url: String,
    token: Zeroizing<String>,
}
enum Outcome {
    Pending,
    Granted(Grant),
    Denied,
}
struct Pending {
    id: u64,
    key: Zeroizing<String>,
    recipient: String,
    seen: bool,
    created: Instant,
    outcome: Outcome,
}
impl Pending {
    fn matches(&self, key: &str) -> bool {
        bool::from(self.key.as_bytes().ct_eq(key.as_bytes()))
    }
    fn expired(&self) -> bool {
        self.created.elapsed() >= LIFETIME
    }
}
struct StateData {
    pending: Option<Pending>,
    next_id: u64,
    last_request: Option<Instant>,
    status: ServerStatus,
}
#[derive(Clone)]
struct Shared {
    data: Arc<Mutex<StateData>>,
    notify: Arc<dyn Fn() + Send + Sync>,
    authority: String,
    requests: Arc<tokio::sync::Semaphore>,
    changed: Arc<tokio::sync::Notify>,
    instance: Option<crate::discovery::Instance>,
}

pub struct RequestBroker {
    shared: Shared,
    cancellation: CancellationToken,
}
impl RequestBroker {
    pub fn start(notify: Arc<dyn Fn() + Send + Sync>) -> Result<Self, &'static str> {
        Self::start_inner(0, notify, Some(crate::discovery::Registry::local()?))
    }
    /// Isolated/manual listener; does not publish a discovery record.
    pub fn start_on(port: u16, notify: Arc<dyn Fn() + Send + Sync>) -> Result<Self, &'static str> {
        Self::start_inner(port, notify, None)
    }
    pub(crate) fn start_inner(
        port: u16,
        notify: Arc<dyn Fn() + Send + Sync>,
        registry: Option<crate::discovery::Registry>,
    ) -> Result<Self, &'static str> {
        let data = Arc::new(Mutex::new(StateData {
            pending: None,
            next_id: 0,
            last_request: None,
            status: ServerStatus::Starting,
        }));
        let shared = Shared {
            data,
            notify,
            authority: String::new(),
            requests: Arc::new(tokio::sync::Semaphore::new(4)),
            changed: Arc::new(tokio::sync::Notify::new()),
            instance: None,
        };
        let cancellation = CancellationToken::new();
        let worker_cancel = cancellation.clone();
        let mut worker = shared.clone();
        std::thread::Builder::new().name("orv-approval-requests".into()).spawn(move || {
            let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_all().build();
            let result = runtime.as_ref().map_err(|_| "Could not start agent requests").and_then(|rt| rt.block_on(async {
                let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).await
                    .map_err(|_| "Agent requests unavailable: another app may be using the local port")?;
                let port = listener.local_addr().map_err(|_| "Agent request address unavailable")?.port();
                worker.authority = format!("127.0.0.1:{port}");
                let instance = crate::discovery::Instance::new(port)?;
                let _lease = registry.map(|r| r.publish(instance.clone())).transpose()?;
                worker.instance = Some(instance);
                worker.status(ServerStatus::Listening { port });
                let router = Router::new().route("/request", post(submit).get(collect)).route("/instance", get(identity))
                    .layer(DefaultBodyLimit::max(4096))
                    .layer(middleware::from_fn_with_state(worker.clone(), guard))
                    .with_state(worker.clone());
                let serving = axum::serve(listener, router).into_future();
                tokio::pin!(serving);
                loop {
                    let deadline = worker.data.lock().ok().and_then(|d| d.pending.as_ref().map(|p| p.created + LIFETIME));
                    let expiry = async {
                        if let Some(deadline) = deadline { tokio::time::sleep_until(deadline.into()).await; }
                        else { std::future::pending::<()>().await; }
                    };
                    tokio::select! {
                        _ = worker_cancel.cancelled() => break Ok(()),
                        result = &mut serving => break result.map_err(|_| "Agent requests stopped"),
                        _ = worker.changed.notified() => (),
                        _ = expiry => {
                            let expired = if let Ok(mut data) = worker.data.lock() {
                                if data.pending.as_ref().is_some_and(Pending::expired) { data.pending = None; true } else { false }
                            } else { false };
                            if expired { (worker.notify)(); }
                        }
                    }
                }
            }));
            if let Ok(mut data) = worker.data.lock() { data.pending = None; }
            worker.status(match result { Ok(()) => ServerStatus::Stopped, Err(e) => ServerStatus::Failed(e) });
            if let Ok(runtime) = runtime { runtime.shutdown_timeout(Duration::from_secs(1)); }
        }).map_err(|_| "Could not start agent request worker")?;
        Ok(Self {
            shared,
            cancellation,
        })
    }
    pub fn status(&self) -> ServerStatus {
        self.shared
            .data
            .lock()
            .map(|d| d.status.clone())
            .unwrap_or(ServerStatus::Failed("Agent requests unavailable"))
    }
    pub fn take_request(&self) -> Option<PendingRequest> {
        let mut data = self.shared.data.lock().ok()?;
        let pending = data.pending.as_mut()?;
        if pending.seen || pending.expired() || !matches!(pending.outcome, Outcome::Pending) {
            return None;
        }
        pending.seen = true;
        Some(PendingRequest {
            id: pending.id,
            recipient: pending.recipient.clone(),
        })
    }
    pub fn is_pending(&self, id: u64) -> bool {
        self.shared.data.lock().ok().is_some_and(|d| {
            d.pending.as_ref().is_some_and(|p| {
                p.id == id && !p.expired() && matches!(p.outcome, Outcome::Pending)
            })
        })
    }
    pub fn deny(&self, id: u64) {
        if let Ok(mut data) = self.shared.data.lock()
            && let Some(p) = &mut data.pending
            && p.id == id
        {
            p.outcome = Outcome::Denied;
        }
    }
    pub fn cancel_requests(&self) {
        if let Ok(mut data) = self.shared.data.lock() {
            data.pending = None;
        }
        self.shared.changed.notify_one();
    }
    pub fn approve(
        &self,
        id: u64,
        session: Session,
        url: String,
        token: &str,
    ) -> Result<(), &'static str> {
        crate::stdio::validate_endpoint(&url)?;
        if session.is_revoked() || !orv_core::scene::is_sha256(token) {
            return Err("Session unavailable");
        }
        let mut data = self.shared.data.lock().map_err(|_| "Request unavailable")?;
        let p = data
            .pending
            .as_mut()
            .filter(|p| p.id == id && !p.expired() && matches!(p.outcome, Outcome::Pending))
            .ok_or("The agent request expired; ask the agent to request access again")?;
        p.outcome = Outcome::Granted(Grant {
            session,
            url,
            token: Zeroizing::new(token.into()),
        });
        Ok(())
    }
}
impl Drop for RequestBroker {
    fn drop(&mut self) {
        self.cancel_requests();
        self.cancellation.cancel();
    }
}
impl Shared {
    fn status(&self, value: ServerStatus) {
        if let Ok(mut data) = self.data.lock() {
            data.status = value;
        }
        (self.notify)();
    }
}

async fn identity(
    State(shared): State<Shared>,
) -> Result<Json<crate::discovery::Instance>, StatusCode> {
    shared
        .instance
        .map(Json)
        .ok_or(StatusCode::SERVICE_UNAVAILABLE)
}

fn key(headers: &HeaderMap) -> Result<&str, StatusCode> {
    if headers.get_all(header::AUTHORIZATION).iter().count() != 1 {
        return Err(StatusCode::UNAUTHORIZED);
    }
    headers
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .filter(|s| orv_core::scene::is_sha256(s))
        .ok_or(StatusCode::UNAUTHORIZED)
}
async fn guard(
    State(shared): State<Shared>,
    request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let h = request.headers();
    if h.get_all(header::HOST).iter().count() != 1
        || h.get(header::HOST).and_then(|h| h.to_str().ok()) != Some(shared.authority.as_str())
        || h.contains_key(header::ORIGIN)
    {
        return Err(StatusCode::FORBIDDEN);
    }
    key(h)?;
    let _permit = shared
        .requests
        .try_acquire()
        .map_err(|_| StatusCode::TOO_MANY_REQUESTS)?;
    tokio::time::timeout(Duration::from_secs(5), next.run(request))
        .await
        .map_err(|_| StatusCode::REQUEST_TIMEOUT)
}
async fn submit(
    State(shared): State<Shared>,
    headers: HeaderMap,
    Json(request): Json<AccessRequest>,
) -> Result<Json<Reply>, StatusCode> {
    let key = key(&headers)?;
    let recipient = request.recipient.trim();
    if recipient.is_empty() || recipient.len() > 128 || recipient.chars().any(char::is_control) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let mut data = shared
        .data
        .lock()
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    if let Some(p) = &data.pending
        && !p.expired()
    {
        if p.matches(key) {
            return Ok(Json(Reply::Pending));
        }
        if matches!(p.outcome, Outcome::Pending | Outcome::Granted(_)) {
            return Err(StatusCode::CONFLICT);
        }
    }
    if data
        .last_request
        .is_some_and(|t| t.elapsed() < Duration::from_secs(3))
    {
        return Err(StatusCode::TOO_MANY_REQUESTS);
    }
    let id = data.next_id;
    data.next_id = data.next_id.wrapping_add(1);
    data.last_request = Some(Instant::now());
    data.pending = Some(Pending {
        id,
        key: Zeroizing::new(key.into()),
        recipient: recipient.into(),
        seen: false,
        created: Instant::now(),
        outcome: Outcome::Pending,
    });
    drop(data);
    shared.changed.notify_one();
    (shared.notify)();
    Ok(Json(Reply::Pending))
}
async fn collect(
    State(shared): State<Shared>,
    headers: HeaderMap,
) -> Result<Json<Reply>, StatusCode> {
    let key = key(&headers)?;
    let mut data = shared
        .data
        .lock()
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let p = data
        .pending
        .as_mut()
        .filter(|p| p.matches(key))
        .ok_or(StatusCode::NOT_FOUND)?;
    if p.expired() {
        p.outcome = Outcome::Denied;
    }
    let reply = match &p.outcome {
        Outcome::Pending => Reply::Pending,
        Outcome::Denied => Reply::Denied,
        Outcome::Granted(grant) if grant.session.is_revoked() => Reply::Denied,
        Outcome::Granted(grant) => Reply::Approved {
            url: grant.url.clone(),
            token: grant.token.to_string(),
        },
    };
    if !matches!(reply, Reply::Pending) {
        data.pending = None;
        shared.changed.notify_one();
    }
    Ok(Json(reply))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn expired_request_is_not_offered_and_can_be_denied_and_cleared() {
        let broker = RequestBroker {
            shared: Shared {
                data: Arc::new(Mutex::new(StateData {
                    pending: None,
                    next_id: 0,
                    last_request: None,
                    status: ServerStatus::Stopped,
                })),
                notify: Arc::new(|| {}),
                authority: String::new(),
                requests: Arc::new(tokio::sync::Semaphore::new(4)),
                changed: Arc::new(tokio::sync::Notify::new()),
                instance: None,
            },
            cancellation: CancellationToken::new(),
        };
        let mut data = broker.shared.data.lock().unwrap();
        data.pending = Some(Pending {
            id: 19,
            key: Zeroizing::new("a".repeat(64)),
            recipient: "Synthetic expired request".into(),
            seen: false,
            created: Instant::now() - LIFETIME,
            outcome: Outcome::Pending,
        });
        drop(data);
        assert!(!broker.is_pending(19));
        assert!(broker.take_request().is_none());
        broker.deny(19);
        assert!(matches!(
            broker
                .shared
                .data
                .lock()
                .unwrap()
                .pending
                .as_ref()
                .unwrap()
                .outcome,
            Outcome::Denied
        ));
        broker.cancel_requests();
        assert!(broker.shared.data.lock().unwrap().pending.is_none());
    }
}
