// SPDX-License-Identifier: GPL-3.0-or-later
//! Shared user-facing limits; these describe intended use, not legal certification.
pub const PURPOSE: &str = "OpenRadView is a free, open-source viewer for personal education with your own medical images.";
pub const LIMITS: &str = "It has not been clinically validated. Do not use it for diagnosis, screening, treatment or decisions about care.";
pub const CARE: &str = "Images and measurements can be wrong. Ask a qualified clinician to interpret your scans; do not delay care because of this app.";
pub const AGENT_PRIVACY: &str = "Your agent or its provider may keep shared data or use it for training under its own settings and terms. Check those before sharing. OpenRadView cannot change them or recall delivered copies.";
pub const AGENT_GUIDANCE: &str = include_str!("../../../assets/text/agent-guidance.txt");

pub fn introductory_task() -> String {
    format!(
        "Call request_access on the local OpenRadView connector and ask me to review the session dialog in the app. After I respond, call request_access again to collect approval. Do not retry a denial without asking me. Call open_study and list_series to check the approved scope, then render_snapshot. Explain a proposed view change before using set_view, and wait for any additional app review it requires.\n\n{AGENT_GUIDANCE}"
    )
}
