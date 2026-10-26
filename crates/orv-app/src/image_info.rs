// SPDX-License-Identifier: GPL-3.0-or-later
//! Explicit local inspection of one immutable displayed source image.
mod worker;
use crate::theme;
use eframe::egui;
use orv_core::{
    ViewState,
    inspection::{self, Histogram},
    study::Frame,
};
use std::{sync::Arc, sync::mpsc::TryRecvError};

#[derive(Default)]
pub struct ImageInfo {
    image: Option<Arc<Frame>>,
    index: usize,
    view: ViewState,
    task: Option<worker::Task>,
    histogram: Option<Histogram>,
    error: Option<&'static str>,
    channel: usize,
    logarithmic: bool,
}
impl ImageInfo {
    pub fn is_open(&self) -> bool {
        self.image.is_some()
    }
    pub fn close(&mut self) {
        *self = Self::default();
    }
    pub fn open(&mut self, image: Arc<Frame>, index: usize, view: ViewState, ctx: egui::Context) {
        self.close();
        self.task = Some(worker::Task::start(image.clone(), ctx));
        self.image = Some(image);
        self.index = index;
        self.view = ViewState {
            slice: index as u32,
            ..view
        };
    }
    pub fn show(&mut self, ctx: &egui::Context) {
        let Some(image) = self.image.clone() else {
            return;
        };
        if let Some(task) = &self.task {
            match task.result.try_recv() {
                Ok(Ok(histogram)) => {
                    self.histogram = Some(histogram);
                    self.task = None;
                }
                Ok(Err(message)) => {
                    self.error = Some(message);
                    self.task = None;
                }
                Err(TryRecvError::Disconnected) => {
                    self.error = Some("The image information worker stopped");
                    self.task = None;
                }
                Err(TryRecvError::Empty) => {}
            }
        }
        let mut open = true;
        window(ctx)
            .open(&mut open)
            .show(ctx, |ui| self.content(ui, &image));
        if !open {
            self.close();
        }
    }
    fn content(&mut self, ui: &mut egui::Ui, image: &Frame) {
        ui.spacing_mut().item_spacing = egui::vec2(8., 6.);
        ui.spacing_mut().button_padding = egui::vec2(10., 5.);
        ui.label(
            egui::RichText::new(format!(
                "Image {} · {} × {}",
                self.index + 1,
                image.width,
                image.height
            ))
            .strong()
            .size(16.),
        );
        ui.weak("Local pixel values. Nothing in this window is sent or saved.");
        if let Some(g) = image.geometry.as_ref().filter(|g| g.validate().is_ok()) {
            ui.small(format!(
                "{} · spacing {} × {} mm (columns × rows)",
                inspection::coordinate_reference(g),
                number(g.spacing[1]),
                number(g.spacing[0])
            ));
        } else {
            ui.small("Physical orientation/spacing unavailable · source pixel grid only");
        }
        ui.separator();
        ui.label(egui::RichText::new("Pixel distribution").strong().size(14.));
        ui.weak("256 equal-width bins · all valid pixels · display settings excluded");
        if let Some(histogram) = &self.histogram {
            ui.label(histogram.domain);
            ui.label(format!(
                "{} valid pixels · {} missing",
                histogram.count, histogram.missing
            ));
            ui.horizontal(|ui| {
                if histogram.channels.len() == 3 {
                    for (index, label) in ["Red", "Green", "Blue"].into_iter().enumerate() {
                        ui.selectable_value(&mut self.channel, index, label);
                    }
                }
                ui.checkbox(&mut self.logarithmic, "Log count scale");
            });
            chart(ui, histogram, self.channel, self.logarithmic);
        } else if let Some(message) = self.error {
            ui.label(message);
        } else {
            ui.label("Counting pixels on a background worker…");
        }
        egui::CollapsingHeader::new("Image details").show(ui, |ui| self.details(ui, image));
        ui.weak("Close this window to continue navigating.");
    }
    fn details(&self, ui: &mut egui::Ui, image: &Frame) {
        if let Some(nifti) = &image.nifti {
            ui.label(nifti.label());
        }
        if let Some(color) = &image.color {
            ui.label(format!(
                "{} source · {}-bit RGB codes",
                match color.info.source_encoding {
                    orv_core::color::ColorEncoding::Rgb => "RGB",
                    orv_core::color::ColorEncoding::YbrFull => "YBR_FULL",
                    orv_core::color::ColorEncoding::YbrFull422 => "YBR_FULL_422",
                    orv_core::color::ColorEncoding::YbrRct => "YBR_RCT",
                    orv_core::color::ColorEncoding::YbrIct => "YBR_ICT",
                    orv_core::color::ColorEncoding::Palette => "Palette",
                },
                color.info.bits
            ));
            ui.label(color.info.presentation());
        } else if image.rgba.is_some() {
            ui.label("8-bit sRGB presentation");
        } else {
            ui.label(
                "Scalar source values after stored scaling; no physical intensity unit is assumed.",
            );
            ui.label(format!(
                "Display width {:.2} · level {:.2} · inverted {}",
                self.view.window.width,
                self.view.window.center,
                self.view.invert ^ image.monochrome1
            ));
        }
        ui.weak("Image spacing is not monitor calibration or a guarantee of true object size. Pixel values do not establish a medical finding.");
    }
}
fn window(ctx: &egui::Context) -> egui::Window<'static> {
    crate::dialogs::window("Image information", ctx)
        .default_width(600.)
        .default_height(560.)
}
pub(super) fn number(value: f64) -> String {
    if value != 0. && (value.abs() < 0.001 || value.abs() >= 1_000_000.) {
        format!("{value:.6e}")
    } else {
        let text = format!("{value:.6}");
        text.trim_end_matches('0').trim_end_matches('.').to_owned()
    }
}

fn chart(ui: &mut egui::Ui, histogram: &Histogram, channel: usize, logarithmic: bool) {
    let Some(hist) = histogram.channels.get(channel) else {
        return;
    };
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 150.), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2., theme::INK);
    let transform = |count: u64| {
        if logarithmic {
            (count as f64).ln_1p()
        } else {
            count as f64
        }
    };
    let maximum = hist
        .bins
        .iter()
        .copied()
        .max()
        .map(transform)
        .unwrap_or(1.)
        .max(1.);
    let width = rect.width() / hist.bins.len() as f32;
    for (index, count) in hist.bins.iter().enumerate().filter(|(_, n)| **n > 0) {
        let x = rect.left() + index as f32 * width;
        let height = (transform(*count) / maximum) as f32 * rect.height();
        painter.rect_filled(
            egui::Rect::from_min_max(
                egui::pos2(x, rect.bottom() - height),
                egui::pos2(x + width, rect.bottom()),
            ),
            0.,
            theme::SILVER,
        );
    }
    ui.horizontal(|ui| {
        ui.monospace(format!("{} … {}", number(hist.min), number(hist.max)));
        ui.weak("source value range");
    });
    if let Some(pos) = response.hover_pos() {
        let index = (((pos.x - rect.left()) / width) as usize).min(hist.bins.len() - 1);
        let step = (hist.max - hist.min) / hist.bins.len() as f64;
        let lower = hist.min + index as f64 * step;
        let upper = if index + 1 == hist.bins.len() {
            hist.max
        } else {
            lower + step
        };
        if hist.min == hist.max {
            response.on_hover_text(format!(
                "Constant value {} · {} pixels in this bin",
                number(hist.min),
                hist.bins[index]
            ));
            return;
        }
        response.on_hover_text(format!(
            "Bin {} · {:.6} to {:.6}{}\n{} pixels",
            index + 1,
            lower,
            upper,
            if index + 1 == hist.bins.len() {
                " (maximum included)"
            } else {
                " (upper endpoint excluded)"
            },
            hist.bins[index]
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn closing_information_releases_the_captured_frame_and_discards_pending_work() {
        let image = orv_core::study::demo_study().series[0].frames[0].clone();
        let weak = Arc::downgrade(&image);
        let mut info = ImageInfo::default();
        info.open(image, 0, ViewState::default(), egui::Context::default());
        assert!(info.is_open());
        info.close();
        assert!(!info.is_open());
        assert!(info.task.is_none());
        assert!(info.histogram.is_none());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while weak.upgrade().is_some() {
            assert!(
                std::time::Instant::now() < deadline,
                "closed inspection retained the source image"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
    #[test]
    fn information_window_with_real_fonts_stays_centered_inside_minimum_viewport() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let image = orv_core::study::demo_study().series[0].frames[0].clone();
        let mut info = ImageInfo {
            histogram: Some(inspection::histogram(&image, 256, || false).unwrap()),
            ..Default::default()
        };
        for size in [
            egui::vec2(1280., 840.),
            egui::vec2(860., 640.),
            egui::vec2(1280., 840.),
        ] {
            let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
            for pass in 0..3 {
                let mut bounds = None;
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(screen),
                        ..Default::default()
                    },
                    |ui| {
                        bounds = window(ui.ctx())
                            .open(&mut true)
                            .show(ui.ctx(), |ui| info.content(ui, &image))
                            .map(|r| r.response.rect);
                    },
                );
                output.textures_delta.clear();
                let bounds = bounds.unwrap();
                assert!(
                    screen.contains_rect(bounds),
                    "information window outside viewport: {bounds:?}"
                );
                assert!(pass == 0 || bounds.center().distance(screen.center()) < 1.5);
            }
        }
    }
}
