// SPDX-License-Identifier: GPL-3.0-or-later
//! Ephemeral comparisons. No provider credentials, filesystem access or sockets.
//! A recipient can read only one reviewed snapshot and submit one bounded reply.
mod session;

pub use session::{AgentReply, Comparison, Consent, Recipient, RecipientStatus};

use orv_core::scene::hex;
use orv_render::snapshot::Snapshot;
use sha2::{Digest, Sha256};
use std::sync::Arc;

/// One immutable renderer result, shared by every recipient of a comparison.
/// It retains neither the source series nor a capability to render other views.
pub struct ReviewedSnapshot {
    snapshot: Snapshot,
}
impl ReviewedSnapshot {
    pub fn new(snapshot: Snapshot) -> Result<Arc<Self>, &'static str> {
        let manifest = &snapshot.manifest;
        let png = &snapshot.png;
        if manifest.scene.hash()? != manifest.scene_sha256
            || hex(&Sha256::digest(png)) != manifest.png_sha256
            || manifest.includes_ui_overlays
        {
            return Err("Snapshot provenance does not match the reviewed data");
        }
        // Renderer-produced PNGs only. Check the fixed IHDR before accepting the
        // bounded bytes; never decode remotely supplied images or metadata here.
        if png.len() < 33
            || png.len() > 8 * 1024 * 1024
            || &png[..8] != b"\x89PNG\r\n\x1a\n"
            || &png[8..16] != b"\0\0\0\rIHDR"
            || png[16..20] != manifest.scene.output_size[0].to_be_bytes()
            || png[20..24] != manifest.scene.output_size[1].to_be_bytes()
        {
            return Err("Snapshot PNG does not match the reviewed dimensions or limits");
        }
        Ok(Arc::new(Self { snapshot }))
    }
    pub fn snapshot(&self) -> &Snapshot {
        &self.snapshot
    }
}

#[cfg(test)]
mod tests;
