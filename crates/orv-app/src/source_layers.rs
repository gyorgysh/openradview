// SPDX-License-Identifier: GPL-3.0-or-later
//! Supplied overlay planes and display shutters in source 2D. Overlays can hold
//! burned-in text, so both stay off until chosen for the current series, and
//! they are never part of picture exports, snapshots or shared views.
use eframe::egui;
use orv_core::{ViewState, study::Frame};
use std::sync::Arc;

/// Composition runs on the UI thread only when the layer changes; keep it small.
const MAX_PIXELS: u64 = 4 * 1024 * 1024;

type Key = (usize, u32, bool, bool);

#[derive(Default)]
pub struct SourceLayers {
    pub overlays: bool,
    pub shutter: bool,
    texture: Option<(Key, egui::TextureHandle)>,
}
impl SourceLayers {
    pub fn controls(&mut self, ui: &mut egui::Ui, image: &Frame) {
        let Some(layers) = &image.presentation else {
            return;
        };
        let too_large = u64::from(image.width) * u64::from(image.height) > MAX_PIXELS;
        ui.add_enabled_ui(!too_large, |ui| {
            if layers.overlay_count() > 0 {
                ui.checkbox(&mut self.overlays, "Overlays")
                    .on_hover_text("Show overlay planes supplied with this image. They can contain burned-in text. Display only: not exported or shared.");
            }
            if layers.has_shutter() {
                ui.checkbox(&mut self.shutter, "Shutter")
                    .on_hover_text("Mask the area outside the supplied display shutter. Statistics and probes still use every source pixel. Not a privacy redaction.");
            }
        });
        if too_large {
            crate::toolbar::label(ui, egui::RichText::new("Layers: image too large").small());
        } else if layers.unsupported > 0 {
            crate::toolbar::label(
                ui,
                egui::RichText::new(format!(
                    "{} unsupported layers not shown",
                    layers.unsupported
                ))
                .small(),
            );
        }
    }
    pub fn draw(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        image: &Frame,
        view: &ViewState,
    ) {
        let Some(layers) = &image.presentation else {
            return;
        };
        if !(self.overlays || self.shutter)
            || u64::from(image.width) * u64::from(image.height) > MAX_PIXELS
        {
            return;
        }
        let key = (
            Arc::as_ptr(layers) as usize,
            image.frame_index,
            self.overlays,
            self.shutter,
        );
        if self.texture.as_ref().is_none_or(|(old, _)| *old != key) {
            self.texture = layers
                .rgba(
                    image.frame_index,
                    image.width,
                    image.height,
                    self.overlays,
                    self.shutter,
                )
                .map(|rgba| {
                    let picture = egui::ColorImage::from_rgba_unmultiplied(
                        [image.width as usize, image.height as usize],
                        &rgba,
                    );
                    let handle = ui.ctx().load_texture(
                        "source presentation layers",
                        picture,
                        egui::TextureOptions::NEAREST,
                    );
                    (key, handle)
                });
        }
        if let Some((_, texture)) = &self.texture
            && let Some(mesh) =
                crate::labels::overlay_mesh(response.rect, image, view, texture.id(), 1.)
        {
            ui.painter_at(response.rect).add(egui::Shape::mesh(mesh));
        }
    }
}
