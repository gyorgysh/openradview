// SPDX-License-Identifier: GPL-3.0-or-later
use crate::ReviewedSnapshot;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

/// Desktop-only review. Protocol callers cannot create or widen a grant.
pub struct Consent {
    pub recipients: Vec<String>,
    pub preview_reviewed: bool,
    pub onward_sharing_accepted: bool,
}

impl Consent {
    pub fn validate(&self) -> Result<(), &'static str> {
        if !self.preview_reviewed || !self.onward_sharing_accepted {
            return Err(
                "Review the exact image, metadata, recipients and onward-sharing risk first",
            );
        }
        if !(2..=4).contains(&self.recipients.len()) {
            return Err("Choose two to four comparison recipients");
        }
        for (index, label) in self.recipients.iter().enumerate() {
            let label = label.trim();
            if !valid_text(label, 128, false)
                || self.recipients[..index]
                    .iter()
                    .any(|s| s.trim().eq_ignore_ascii_case(label))
            {
                return Err("Recipient labels must be distinct and contain at most 128 bytes");
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentReply {
    pub scene_sha256: String,
    pub png_sha256: String,
    /// Self-reported by the client, never authenticated as a provider/model.
    pub model: String,
    pub commentary: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum RecipientStatus {
    Waiting,
    SnapshotRead,
    Submitted(AgentReply),
    Revoked,
}

struct State {
    snapshot: Arc<ReviewedSnapshot>,
    status: RecipientStatus,
}
struct Inner {
    label: String,
    cancellation: CancellationToken,
    state: Mutex<Option<State>>,
}

#[derive(Clone)]
pub struct Recipient(Arc<Inner>);
impl Recipient {
    pub fn label(&self) -> &str {
        &self.0.label
    }
    pub fn cancellation(&self) -> CancellationToken {
        self.0.cancellation.clone()
    }
    pub fn is_revoked(&self) -> bool {
        self.0.cancellation.is_cancelled()
    }
    pub fn revoke(&self) {
        self.0.cancellation.cancel();
        if let Ok(mut state) = self.0.state.lock() {
            *state = None;
        }
    }
    fn with_state<T>(
        &self,
        f: impl FnOnce(&mut State) -> Result<T, &'static str>,
    ) -> Result<T, &'static str> {
        let mut guard = self.0.state.lock().map_err(|_| "Comparison unavailable")?;
        if self.is_revoked() {
            return Err("Comparison access has been revoked");
        }
        f(guard.as_mut().ok_or("Comparison access has been revoked")?)
    }
    pub fn status(&self) -> RecipientStatus {
        self.with_state(|s| Ok(s.status.clone()))
            .unwrap_or(RecipientStatus::Revoked)
    }
    /// Callers must recheck revocation before returning bytes over a transport.
    pub fn read_snapshot(&self) -> Result<Arc<ReviewedSnapshot>, &'static str> {
        self.with_state(|s| {
            if s.status == RecipientStatus::Waiting {
                s.status = RecipientStatus::SnapshotRead;
            }
            Ok(s.snapshot.clone())
        })
    }
    pub fn submit(&self, reply: AgentReply) -> Result<(), &'static str> {
        if !valid_text(&reply.model, 128, false) || !valid_text(&reply.commentary, 32_768, true) {
            return Err(
                "Reply must contain a model label and plain commentary within the size limits",
            );
        }
        self.with_state(|s| {
            let m = &s.snapshot.snapshot().manifest;
            if reply.scene_sha256 != m.scene_sha256 || reply.png_sha256 != m.png_sha256 {
                return Err("Reply provenance does not match this comparison");
            }
            match &s.status {
                RecipientStatus::Waiting => {
                    Err("Read the comparison snapshot before submitting commentary")
                }
                RecipientStatus::Submitted(previous) if *previous == reply => Ok(()),
                RecipientStatus::Submitted(_) => Err("A final reply has already been submitted"),
                RecipientStatus::Revoked => Err("Comparison access has been revoked"),
                RecipientStatus::SnapshotRead => {
                    s.status = RecipientStatus::Submitted(reply);
                    Ok(())
                }
            }
        })
    }
}

fn valid_text(text: &str, limit: usize, multiline: bool) -> bool {
    !text.trim().is_empty()
        && text.len() <= limit
        && !text.chars().any(|c| {
            (c.is_control() && !(multiline && matches!(c, '\n' | '\t')))
                || matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        })
}

/// Owning desktop handle. Dropping it revokes all cloned recipient handles.
pub struct Comparison {
    recipients: Vec<Recipient>,
}
impl Comparison {
    pub fn grant(snapshot: Arc<ReviewedSnapshot>, consent: Consent) -> Result<Self, &'static str> {
        consent.validate()?;
        let labels = consent
            .recipients
            .into_iter()
            .map(|label| label.trim().to_owned());
        Ok(Self {
            recipients: labels
                .map(|label| {
                    Recipient(Arc::new(Inner {
                        label,
                        cancellation: CancellationToken::new(),
                        state: Mutex::new(Some(State {
                            snapshot: snapshot.clone(),
                            status: RecipientStatus::Waiting,
                        })),
                    }))
                })
                .collect(),
        })
    }
    pub fn recipients(&self) -> &[Recipient] {
        &self.recipients
    }
    pub fn revoke(&self) {
        for recipient in &self.recipients {
            recipient.revoke();
        }
    }
}
impl Drop for Comparison {
    fn drop(&mut self) {
        self.revoke();
    }
}
