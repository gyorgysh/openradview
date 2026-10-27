// SPDX-License-Identifier: GPL-3.0-or-later
mod sharing;
mod worker;
use crate::{
    annotations::Annotations,
    controls, theme,
    viewport::{self, Tool},
};
use eframe::egui::{self, RichText, Vec2};
use orv_core::{
    ViewState, WindowLevel, oblique::ObliquePlane, spatial::GeometryError, volume::Volume,
};
use worker::{Snapshot, Worker};

pub struct ObliqueSession {
    picture_size: Option<[u32; 2]>,
    worker: Worker,
    snapshot: Option<Snapshot>,
    error: Option<String>,
    angles: [f64; 3],
    offset_mm: f64,
    target: Option<ObliquePlane>,
    base: Option<ObliquePlane>,
    reformat_side: u32,
    view: ViewState,
    original_window: WindowLevel,
    tool: Tool,
    scroll: f32,
    annotations: Annotations,
}
impl ObliqueSession {
    pub(crate) fn picture(&self) -> Option<(crate::export::Pixels, [u32; 2])> {
        let (_, true) = self.shared_view()? else {
            return None;
        };
        Some((
            crate::export::Pixels::Reformat(
                self.snapshot.as_ref()?.image.clone(),
                self.snapshot.as_ref()?.volume.clone(),
            ),
            self.picture_size?,
        ))
    }
    pub fn start(
        frames: impl Into<crate::frame_source::FrameSource>,
        window: WindowLevel,
        ctx: egui::Context,
    ) -> Self {
        let frames = frames.into();
        let original_window = frames.original_window().unwrap_or(window);
        Self {
            picture_size: None,
            worker: Worker::start(frames, ctx),
            snapshot: None,
            error: None,
            angles: [0.; 3],
            offset_mm: 0.,
            target: None,
            base: None,
            reformat_side: 512,
            view: ViewState {
                window,
                ..Default::default()
            },
            original_window,
            tool: Tool::Pan,
            scroll: 0.,
            annotations: Annotations::for_reformats(),
        }
    }
    pub(crate) fn poll(&mut self) {
        if let Some(result) = self.worker.take() {
            match result {
                Ok(snapshot) => {
                    if snapshot.volume.is_color() {
                        self.view.window = snapshot.volume.frames()[0].default_window;
                    }
                    if self.snapshot.is_none()
                        && let Some(frame) = snapshot
                            .volume
                            .frames()
                            .get(snapshot.volume.frames().len() / 2)
                    {
                        self.original_window = frame.default_window;
                    }
                    if self.target.is_none() {
                        self.target = Some(snapshot.plane.clone());
                    }
                    self.annotations.clear();
                    self.snapshot = Some(snapshot);
                    self.error = None;
                }
                Err(error) => self.error = Some(error),
            }
        }
    }
    pub fn ui(&mut self, ui: &mut egui::Ui, frame: &eframe::Frame) {
        let width = ui.available_width();
        self.poll();
        if let Some(error) = &self.error {
            ui.heading("This oblique plane could not be prepared");
            ui.label(error);
            ui.label("Return to 2D/MPR to continue exploring the source images.");
            return;
        }
        let Some(snapshot) = self.snapshot.clone() else {
            ui.label("Checking geometry and preparing an oblique plane on a background worker…");
            return;
        };
        let color = snapshot.volume.is_color();
        if color && self.tool == Tool::Window {
            self.tool = Tool::Pan;
        }
        if ui.is_enabled() && !ui.ctx().egui_wants_keyboard_input() {
            ui.input(|i| {
                for (key, tool) in [
                    (egui::Key::W, Tool::Window),
                    (egui::Key::P, Tool::Pan),
                    (egui::Key::Z, Tool::Zoom),
                    (egui::Key::S, Tool::Browse),
                ] {
                    if i.key_pressed(key) && (!color || tool != Tool::Window) {
                        self.tool = tool;
                    }
                }
                if i.key_pressed(egui::Key::I) {
                    self.view.invert = !self.view.invert;
                }
                if i.key_pressed(egui::Key::R) {
                    self.fit();
                }
            });
        }
        crate::view_context::show(ui, &snapshot.image);
        if controls::tools(ui, &mut self.tool, false, !color, |ui| {
            controls::orientation(ui, &mut self.view.transform);
            if ui.button("Clear measurements").clicked() {
                self.annotations.clear();
            }
        }) {
            self.fit();
        }
        if color {
            controls::color_display(ui, &mut self.view.invert, snapshot.volume.color_info());
        } else {
            controls::window(
                ui,
                &mut self.view.window,
                &mut self.view.invert,
                self.original_window,
                snapshot.volume.frames()[0]
                    .scalar
                    .as_ref()
                    .map(|data| &**data.choices()),
            );
        }
        let controls = egui::CollapsingHeader::new("Plane controls")
            .default_open(width >= 800.)
            .show(ui, |ui| self.plane_controls(ui, &snapshot.volume));
        let geometry = controls
            .body_returned
            .unwrap_or_else(|| self.plane_geometry(&snapshot.volume));
        let (mut plane, limits) = match geometry {
            Ok(geometry) => geometry,
            Err(error) => {
                self.error = Some(error.to_string());
                return;
            }
        };
        let normal = plane.normal_direction();
        let count = (((limits[1] - limits[0]) / snapshot.volume.step_mm()).ceil() as u32)
            .clamp(1, 8191)
            + 1;
        let step = (limits[1] - limits[0]) / f64::from(count - 1);
        let initial = ((self.offset_mm - limits[0]) / step)
            .round()
            .clamp(0., f64::from(count - 1)) as u32;
        self.view.slice = initial;
        if ui.is_enabled() && !ui.ctx().egui_wants_keyboard_input() {
            ui.input(|i| {
                if i.key_pressed(egui::Key::ArrowDown) {
                    self.view.slice = (initial + 1).min(count - 1);
                }
                if i.key_pressed(egui::Key::ArrowUp) {
                    self.view.slice = initial.saturating_sub(1);
                }
            });
        }
        let (_, response) = ui.allocate_exact_size(
            Vec2::new(width, (ui.available_height() - 62.).max(60.)),
            egui::Sense::click_and_drag(),
        );
        viewport::interact(
            ui,
            &response,
            &mut self.view,
            self.tool,
            count as usize,
            &mut self.scroll,
        );
        if self.view.slice != initial {
            self.offset_mm = limits[0] + step * f64::from(self.view.slice);
        }
        self.view.slice = 0;
        if self.offset_mm != 0. {
            plane.center_mm =
                std::array::from_fn(|i| plane.center_mm[i] + normal[i] * self.offset_mm);
        }
        if self.target.as_ref() != Some(&plane) {
            self.worker.request(plane.clone(), self.reformat_side);
            self.target = Some(plane);
        }
        self.picture_size = Some(crate::rendered::picture_size(
            response.rect.size() * ui.ctx().pixels_per_point(),
        ));
        viewport::draw(
            ui,
            frame,
            response.rect,
            snapshot.image.clone(),
            &self.view,
            4,
            "OBLIQUE · RESEARCH ONLY",
        );
        crate::image_overlay::draw(ui, &response, &snapshot.image, &self.view);
        self.annotations
            .show(ui, &response, &snapshot.image, &self.view, self.tool);
        self.annotations
            .status_overlay(ui, response.rect, self.tool);
        let [l, p, s] = snapshot.plane.center_mm;
        let status = if snapshot.generation == self.worker.generation() {
            "Rendered"
        } else {
            "Updating · previous plane shown"
        };
        ui.label(
            RichText::new(format!("{status} · centre L {l:.2}  P {p:.2}  S {s:.2} mm"))
                .small()
                .color(theme::SILVER),
        );
        let hint = if color {
            "Images [S] / arrows: move plane · Pan [P] / Space drag: move image · Zoom [Z] / pinch: zoom · Resampled RGB codes may differ from source images."
        } else {
            "Images [S] / arrows: move plane · Pan [P] / Space drag: move image · Zoom [Z] / pinch: zoom · Contrast [W]: window/level · Resampled values may differ from source images."
        };
        let hint_height = ui
            .painter()
            .layout(
                hint.into(),
                egui::TextStyle::Small.resolve(ui.style()),
                theme::SILVER,
                width,
            )
            .size()
            .y;
        if ui.available_height() >= hint_height {
            ui.label(RichText::new(hint).small().color(theme::SILVER));
        }
        response.on_hover_text(hint);
    }
    pub(crate) fn plane_geometry(
        &mut self,
        volume: &Volume,
    ) -> Result<(ObliquePlane, [f64; 2]), GeometryError> {
        let base = match &self.base {
            Some(base) => base.clone(),
            None => ObliquePlane::from_angles(volume.center(), [0.; 3])?,
        };
        let plane = base.rotated(self.angles)?;
        let normal = plane.normal_direction();
        let mut limits = [f64::INFINITY, f64::NEG_INFINITY];
        for corner in volume.corners() {
            let offset: f64 = (0..3)
                .map(|i| (corner[i] - plane.center_mm[i]) * normal[i])
                .sum();
            limits[0] = limits[0].min(offset);
            limits[1] = limits[1].max(offset);
        }
        // An explicitly requested plane may be outside the source. Keep its exact
        // pivot instead of silently moving it into the volume while idle.
        limits[0] = limits[0].min(0.);
        limits[1] = limits[1].max(0.);
        self.offset_mm = self.offset_mm.clamp(limits[0], limits[1]);
        Ok((plane, limits))
    }
    fn plane_controls(
        &mut self,
        ui: &mut egui::Ui,
        volume: &Volume,
    ) -> Result<(ObliquePlane, [f64; 2]), GeometryError> {
        crate::toolbar::row(ui, |ui| {
            crate::toolbar::label(ui,if self.base.is_some(){"LPS rotation Δ"}else{"LPS rotation"});
            for (label, angle) in ["X", "Y", "Z"].into_iter().zip(&mut self.angles) {
                crate::toolbar::number(ui, label,
                    egui::DragValue::new(angle)
                        .range(-180.0..=180.0)
                        .speed(0.5)
                        .suffix("°"),
                );
            }
            if ui.button("Reset plane").clicked() {
                self.angles = [0.; 3];
                self.base=None;
                self.offset_mm = 0.;
                self.fit();
            }
        })
        .response
        .on_hover_text("Fixed LPS X, then Y, then Z rotations about the plane pivot. With an applied plane, Δ is relative to its exact axes. Reset restores +X columns / +Y rows at the volume centre.");
        let (plane, limits) = self.plane_geometry(volume)?;
        crate::toolbar::row(ui, |ui| {
            ui.add(egui::Slider::new(&mut self.offset_mm, limits[0]..=limits[1]).show_value(false));
            crate::toolbar::number(
                ui,
                "Normal offset",
                egui::DragValue::new(&mut self.offset_mm)
                    .range(limits[0]..=limits[1])
                    .speed(volume.step_mm())
                    .fixed_decimals(1)
                    .suffix(" mm"),
            );
        });
        Ok((plane, limits))
    }
    fn fit(&mut self) {
        self.view.zoom = 1.;
        self.view.pan = [0.; 2];
    }
}
