// SPDX-License-Identifier: GPL-3.0-or-later
//! Local display fusion of the paired panes: the second pane's current image,
//! with its own display window, drawn over the first pane. Readouts, statistics
//! and exports keep each source separate; nothing here reaches sharing.
use crate::jobs::Worker;
use eframe::egui;
use orv_core::{ViewState, fusion, study::Frame};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Off,
    Opacity,
    Checkerboard,
}

struct Input {
    target: Arc<Frame>,
    source: Arc<Frame>,
    view: ViewState,
    offset_mm: f64,
    checkerboard: bool,
}
type Key = (usize, usize, ViewState, u64, bool);

fn compose(input: Input, cancelled: &dyn Fn() -> bool) -> Result<egui::ColorImage, String> {
    let plane = fusion::resample_onto(&input.target, &input.source, input.offset_mm, &cancelled)
        .map_err(|error| error.to_string())?;
    let (width, height) = (plane.width as usize, plane.height as usize);
    // Checker tiles cover roughly a twentieth of the target's longer side.
    let tile = (width.max(height) / 20).max(1);
    let mut rgba = Vec::new();
    rgba.try_reserve_exact(width * height * 4)
        .map_err(|_| fusion::FusionError::Limit.to_string())?;
    for (index, (value, valid)) in plane.values.iter().zip(&plane.valid).enumerate() {
        if index % 65536 == 0 && cancelled() {
            return Err(fusion::FusionError::Cancelled.to_string());
        }
        let (x, y) = (index % width, index / width);
        let shown = *valid == 1 && (!input.checkerboard || (x / tile + y / tile) % 2 == 0);
        let gray = shown
            .then(|| input.source.normalize(input.view.window, *value))
            .flatten()
            .map(|g| {
                if input.view.invert ^ input.source.monochrome1 {
                    1. - g
                } else {
                    g
                }
            });
        rgba.extend_from_slice(&match gray {
            Some(g) => {
                let g = (g.clamp(0., 1.) * 255.).round() as u8;
                [g, g, g, 255]
            }
            None => [0, 0, 0, 0],
        });
    }
    Ok(egui::ColorImage::from_rgba_unmultiplied(
        [width, height],
        &rgba,
    ))
}

#[derive(Default)]
pub struct Fusion {
    pub mode: Mode,
    pub opacity: f32,
    worker: Option<Worker<Input, egui::ColorImage>>,
    pending: Option<(u64, Key)>,
    /// A refused combination is not resubmitted until an input changes.
    failed: Option<Key>,
    texture: Option<(Key, egui::TextureHandle)>,
    pub status: Option<String>,
}
impl Fusion {
    pub fn controls(&mut self, ui: &mut egui::Ui, enabled: bool, pair: [usize; 2]) {
        ui.add_enabled_ui(enabled, |ui| {
            ui.horizontal(|ui| {
                ui.label(format!("Fuse pane {} over pane {}", pair[1] + 1, pair[0] + 1));
                // Inline choices: a nested popup would close the Links menu.
                for (mode, label) in [
                    (Mode::Off, "Off"),
                    (Mode::Opacity, "Opacity"),
                    (Mode::Checkerboard, "Checkerboard"),
                ] {
                    ui.selectable_value(&mut self.mode, mode, label);
                }
            });
            if self.mode == Mode::Opacity {
                if self.opacity == 0. {
                    self.opacity = 0.5;
                }
                ui.add(egui::Slider::new(&mut self.opacity, 0.05..=1.).text("Opacity"));
            }
        })
        .response
        .on_disabled_hover_text("Fusion needs both paired panes on regular parallel planes in the same declared spatial reference; no registration.");
        if self.mode != Mode::Off {
            ui.weak("Display only: probes, statistics and exports keep each source separate.");
            if let Some(status) = &self.status {
                ui.weak(status.as_str());
            }
        }
    }
    pub fn clear(&mut self) {
        self.mode = Mode::Off;
        self.pending = None;
        self.failed = None;
        self.texture = None;
        self.status = None;
        if let Some(worker) = &self.worker {
            worker.clear();
        }
    }
    /// Returns the composed texture for the target pane, if current.
    pub fn update(
        &mut self,
        ctx: &egui::Context,
        target: Option<&Arc<Frame>>,
        source: Option<(&Arc<Frame>, &ViewState)>,
        offset_mm: Option<f64>,
    ) -> Option<(egui::TextureId, f32)> {
        if self.mode == Mode::Off {
            self.texture = None;
            self.pending = None;
            return None;
        }
        let (Some(target), Some((source, view)), Some(offset_mm)) = (target, source, offset_mm)
        else {
            self.texture = None;
            self.status = Some("Fusion is waiting for both paired images.".into());
            return None;
        };
        let checkerboard = self.mode == Mode::Checkerboard;
        let mut window_view = view.clone();
        // Only display mapping matters to the composed picture.
        window_view.pan = [0.; 2];
        window_view.zoom = 1.;
        window_view.slice = 0;
        window_view.transform = Default::default();
        let key: Key = (
            Arc::as_ptr(target) as usize,
            Arc::as_ptr(source) as usize,
            window_view.clone(),
            offset_mm.to_bits(),
            checkerboard,
        );
        if let Some(worker) = &self.worker
            && let Some((generation, result)) = worker.take()
            && let Some((expected, pending)) = self.pending.take()
        {
            if generation == expected {
                match result {
                    Ok(picture) => {
                        let handle = ctx.load_texture(
                            "comparison fusion",
                            picture,
                            egui::TextureOptions::NEAREST,
                        );
                        self.texture = Some((pending, handle));
                        self.status = None;
                    }
                    Err(message) => {
                        self.texture = None;
                        self.failed = Some(pending);
                        self.status = Some(message);
                    }
                }
            } else {
                self.pending = Some((expected, pending));
            }
        }
        let current = self.texture.as_ref().is_some_and(|(old, _)| *old == key);
        let requested = self.pending.as_ref().is_some_and(|(_, old)| *old == key);
        let failed = self.failed.as_ref() == Some(&key);
        if !current && !requested && !failed {
            self.status = Some("Preparing the fused picture locally…".into());
            if self.worker.is_none() {
                match Worker::start(ctx.clone(), "orv-fusion", compose) {
                    Ok(worker) => self.worker = Some(worker),
                    Err(message) => {
                        self.status = Some(message);
                        return None;
                    }
                }
            }
            if let Some(worker) = &self.worker {
                let generation = worker.submit(Input {
                    target: target.clone(),
                    source: source.clone(),
                    view: window_view,
                    offset_mm,
                    checkerboard,
                });
                self.pending = Some((generation, key.clone()));
            }
        }
        let opacity = if checkerboard { 1. } else { self.opacity };
        self.texture
            .as_ref()
            .filter(|(old, _)| *old == key)
            .map(|(_, texture)| (texture.id(), opacity))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orv_core::{WindowLevel, study::Geometry};
    fn frame(values: Vec<f32>, width: u32, x: f64) -> Arc<Frame> {
        let mut frame = (*orv_core::study::demo_study().series[0].frames[0]).clone();
        frame.width = width;
        frame.height = values.len() as u32 / width;
        frame.pixels = values.into();
        frame.rgba = None;
        frame.color = None;
        frame.validity = None;
        frame.presentation = None;
        frame.monochrome1 = false;
        frame.geometry = Some(Geometry {
            nifti_transform: None,
            frame_of_reference: Some("2.25.9".into()),
            position: [x, 0., 0.],
            orientation: [1., 0., 0., 0., 1., 0.],
            spacing: [1., 1.],
        });
        Arc::new(frame)
    }
    #[test]
    fn composition_uses_the_source_window_and_leaves_gaps_transparent() {
        let target = frame(vec![0.; 40 * 2], 40, 0.);
        // Source starts one pixel right; its values equal the column index.
        let source = frame((0..80).map(|i| (i % 40) as f32).collect(), 40, 1.);
        let view = ViewState {
            window: WindowLevel {
                function: orv_core::VoiFunction::LinearExact,
                width: 40.,
                center: 20.,
            },
            ..Default::default()
        };
        for (checkerboard, invert) in [(false, false), (false, true), (true, false)] {
            let picture = compose(
                Input {
                    target: target.clone(),
                    source: source.clone(),
                    view: ViewState {
                        invert,
                        ..view.clone()
                    },
                    offset_mm: 0.,
                    checkerboard,
                },
                &|| false,
            )
            .unwrap();
            assert_eq!(picture.size, [40, 2]);
            // Target column 0 lies outside the source.
            assert_eq!(picture.pixels[0].a(), 0);
            // Target column 11 is source column 10: ((10 - 20) / 40 + 0.5) * 255.
            let pixel = picture.pixels[11];
            let tile_shown = !checkerboard || 11 / 2 % 2 == 0;
            if tile_shown {
                let expected = if invert { 255 - 64 } else { 64 };
                assert!(pixel.r().abs_diff(expected) <= 1, "{pixel:?}");
            } else {
                assert_eq!(pixel.a(), 0);
            }
        }
        assert!(
            compose(
                Input {
                    target,
                    source,
                    view,
                    offset_mm: 0.,
                    checkerboard: false,
                },
                &|| true,
            )
            .is_err()
        );
    }
}
