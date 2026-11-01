// SPDX-License-Identifier: GPL-3.0-or-later
//! Explicit local illustrations have their own composition, preview and manifest.
mod capture;
mod render;
mod save;
mod ui;
use super::{Input, Pixels};
use crate::{annotations::Outlines, labels::CapturedMasks};
use eframe::egui;
use orv_core::{resources::Reservation, study::Frame};
use serde::Serialize;
use std::sync::Arc;
pub(super) use ui::Controller;

pub(crate) struct FrozenInput {
    pub base: Input,
    pub outlines: Outlines,
    pub masks: Option<CapturedMasks>,
    pub mask_note: Option<&'static str>,
}
impl FrozenInput {
    fn image(&self) -> Result<&Arc<Frame>, &'static str> {
        let image = match &self.base.pixels {
            Pixels::Image(image) | Pixels::Reformat(image, _) => image,
            Pixels::Volume(_) => return Err("Illustrations need a source image or thin reformat"),
        };
        if image.slab.is_some() {
            return Err("Illustrations currently need a thin image plane");
        }
        Ok(image)
    }
}
#[derive(Clone, Copy, Default, PartialEq, Eq, Serialize)]
struct Choice {
    marks: bool,
    masks: bool,
}
struct Request {
    input: Arc<FrozenInput>,
    choice: Choice,
}
struct Artifact {
    input: Arc<FrozenInput>,
    choice: Choice,
    source_sha256: String,
    png: Vec<u8>,
    manifest: Vec<u8>,
    preview: Arc<egui::ColorImage>,
    size: [u32; 2],
    _memory: Reservation,
}
