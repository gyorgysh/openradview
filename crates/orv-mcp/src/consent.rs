// SPDX-License-Identifier: GPL-3.0-or-later
//! Explicit per-session authorization; no implicit grants or persisted secrets.
use crate::catalog::{PreparedSeries, SeriesDescription};
use orv_core::scene::{SceneState, hex};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use subtle::ConstantTimeEq;
use tokio_util::sync::CancellationToken;
use zeroize::Zeroize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DataScope {
    /// Each new scene needs another exact image preview in the desktop app.
    ReviewedScenes,
    /// Explicitly permits exploration of all images/derived views in one series.
    SelectedSeries,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolScopes {
    pub view_control: bool,
    pub snapshots: bool,
    pub metadata: bool,
    pub measurements: bool,
}

#[derive(Clone, Debug)]
pub struct Consent {
    /// User-supplied label, never a claim that a remote account is authenticated.
    pub recipient: String,
    pub data: DataScope,
    pub tools: ToolScopes,
    pub preview_reviewed: bool,
    pub burned_in_risk_accepted: bool,
}

impl Consent {
    fn validate(&self) -> Result<(), &'static str> {
        if self.recipient.trim().is_empty()
            || self.recipient.len() > 128
            || self.recipient.chars().any(char::is_control)
        {
            return Err("Enter a recipient label of at most 128 bytes");
        }
        if !self.preview_reviewed || !self.burned_in_risk_accepted {
            return Err("Review the image and acknowledge the burned-in identifier risk first");
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
pub enum Operation {
    SetView,
    Snapshot,
    Metadata,
    Measurement,
}

struct State {
    source: Arc<PreparedSeries>,
    consent: Consent,
    reviewed: HashSet<String>,
    current: SceneState,
    pending_view: Option<SceneState>,
}

struct Inner {
    revoked: AtomicBool,
    cancellation: CancellationToken,
    state: Mutex<Option<State>>,
}

/// Cloning this handle never extends a grant beyond revocation.
#[derive(Clone)]
pub struct Session {
    inner: Arc<Inner>,
}

impl Session {
    /// Only desktop consent UI may call this. No transport can create a grant.
    pub fn grant(
        source: Arc<PreparedSeries>,
        consent: Consent,
        preview: SceneState,
    ) -> Result<Self, &'static str> {
        consent.validate()?;
        source.check_scene(&preview)?;
        let reviewed = HashSet::from([preview.hash()?]);
        Ok(Self {
            inner: Arc::new(Inner {
                revoked: AtomicBool::new(false),
                cancellation: CancellationToken::new(),
                state: Mutex::new(Some(State {
                    source,
                    consent,
                    reviewed,
                    current: preview,
                    pending_view: None,
                })),
            }),
        })
    }

    pub fn is_revoked(&self) -> bool {
        self.inner.revoked.load(Ordering::Acquire)
    }
    pub fn cancellation(&self) -> CancellationToken {
        self.inner.cancellation.clone()
    }

    pub fn revoke(&self) {
        self.inner.revoked.store(true, Ordering::Release);
        self.inner.cancellation.cancel();
        if let Ok(mut state) = self.inner.state.lock() {
            *state = None;
        }
    }

    fn with_state<T>(
        &self,
        f: impl FnOnce(&mut State) -> Result<T, &'static str>,
    ) -> Result<T, &'static str> {
        if self.is_revoked() {
            return Err("Session access has been revoked");
        }
        let mut guard = self
            .inner
            .state
            .lock()
            .map_err(|_| "Session access is unavailable")?;
        if self.is_revoked() {
            return Err("Session access has been revoked");
        }
        f(guard.as_mut().ok_or("Session access has been revoked")?)
    }

    pub fn description(&self) -> Result<SeriesDescription, &'static str> {
        self.with_state(|s| Ok(s.source.description().clone()))
    }
    pub fn current(&self) -> Result<SceneState, &'static str> {
        self.with_state(|s| Ok(s.current.clone()))
    }
    /// Transport-visible context must not reveal an unreviewed desktop position.
    pub fn approved_current(&self) -> Result<Option<SceneState>, &'static str> {
        self.with_state(|s| {
            let approved = s.consent.data == DataScope::SelectedSeries
                || s.reviewed.contains(&s.current.hash()?);
            Ok(approved.then(|| s.current.clone()))
        })
    }
    pub fn recipient(&self) -> Result<String, &'static str> {
        self.with_state(|s| Ok(s.consent.recipient.clone()))
    }

    pub fn authorize(
        &self,
        scene: &SceneState,
        operation: Operation,
    ) -> Result<Arc<PreparedSeries>, &'static str> {
        self.with_state(|s| {
            s.source.check_scene(scene)?;
            let allowed = match operation {
                Operation::SetView => s.consent.tools.view_control,
                Operation::Snapshot => s.consent.tools.snapshots,
                Operation::Metadata => s.consent.tools.metadata,
                Operation::Measurement => s.consent.tools.measurements,
            };
            if !allowed {
                return Err("This tool is outside the consented scope");
            }
            if !matches!(operation, Operation::SetView)
                && s.consent.data == DataScope::ReviewedScenes
                && !s.reviewed.contains(&scene.hash()?)
            {
                return Err(
                    "Review and approve this exact view in the desktop app before sharing its data",
                );
            }
            Ok(s.source.clone())
        })
    }

    /// Called by desktop UI after displaying the exact new preview. Not a tool.
    pub fn approve_preview(&self, scene: &SceneState) -> Result<(), &'static str> {
        self.with_state(|s| {
            s.source.check_scene(scene)?;
            if s.reviewed.len() >= 256 {
                return Err("Start a new session to review more views");
            }
            s.reviewed.insert(scene.hash()?);
            Ok(())
        })
    }

    pub fn request_view(&self, scene: SceneState) -> Result<(), &'static str> {
        self.authorize(&scene, Operation::SetView)?;
        self.with_state(|s| {
            s.pending_view = Some(scene.clone());
            s.current = scene;
            Ok(())
        })
    }
    /// Coalesced requests keep an agent from creating an unbounded UI queue.
    pub fn take_requested_view(&self) -> Result<Option<SceneState>, &'static str> {
        self.with_state(|s| Ok(s.pending_view.take()))
    }
    pub fn publish_view(&self, scene: SceneState) -> Result<(), &'static str> {
        self.with_state(|s| {
            s.source.check_scene(&scene)?;
            // A request arriving between UI polling and publication must retain
            // precedence until the desktop has consumed it.
            if s.pending_view.is_none() {
                s.current = scene;
            }
            Ok(())
        })
    }
}

/// The token is deliberately neither Debug nor Serialize. Never put it in a URL.
pub struct BearerToken {
    value: String,
}

impl BearerToken {
    pub fn generate() -> Result<Self, &'static str> {
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes).map_err(|_| "Could not obtain secure session randomness")?;
        let value = hex(&bytes);
        bytes.zeroize();
        Ok(Self { value })
    }
    /// Only for explicit user copy/configuration, never logging.
    pub fn expose_for_configuration(&self) -> &str {
        &self.value
    }
    pub fn matches(&self, candidate: &str) -> bool {
        self.value.as_bytes().ct_eq(candidate.as_bytes()).into()
    }
}
impl Drop for BearerToken {
    fn drop(&mut self) {
        self.value.zeroize();
    }
}
