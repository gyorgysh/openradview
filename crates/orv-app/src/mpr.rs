// SPDX-License-Identifier: GPL-3.0-or-later
use crate::{
    annotations::Annotations,
    controls,
    mpr_worker::{MprSnapshot, MprWorker, PLANES},
    theme,
    viewport::Tool,
};
use eframe::egui::{self, Rect, RichText, Vec2};
use orv_core::{ViewState, WindowLevel};

#[derive(Clone, Copy, PartialEq, Default)]
enum Layout {
    #[default]
    Grid,
    Columns,
    Axial,
    Coronal,
    Sagittal,
}
impl Layout {
    fn label(self) -> &'static str {
        match self {
            Self::Grid => "Adaptive grid",
            Self::Columns => "Three columns",
            Self::Axial => "Focus axial",
            Self::Coronal => "Focus coronal",
            Self::Sagittal => "Focus sagittal",
        }
    }
}
pub struct MprSession {
    pub(super) picture_sizes: [Option<[u32; 2]>; 3],
    worker: MprWorker,
    pub(super) snapshot: Option<MprSnapshot>,
    error: Option<String>,
    pub(super) target: Option<[f64; 3]>,
    pub(super) views: [ViewState; 3],
    pub(super) annotations: [Annotations; 3],
    pub(super) scroll: [f32; 3],
    pub(super) active: usize,
    pub(super) tool: Tool,
    pub(super) window: WindowLevel,
    original_window: WindowLevel,
    invert: bool,
    layout: Layout,
    reformat_side: u32,
    last_input: std::time::Instant,
}
impl MprSession {
    pub(crate) fn picture(&self) -> Option<(crate::export::Pixels, [u32; 2])> {
        let (spec, true) = self.shared_view()? else {
            return None;
        };
        let index = match spec.view.plane {
            orv_core::Plane::Axial => 0,
            orv_core::Plane::Coronal => 1,
            orv_core::Plane::Sagittal => 2,
            _ => return None,
        };
        Some((
            crate::export::Pixels::Reformat(
                self.snapshot.as_ref()?.images[index].clone(),
                self.snapshot.as_ref()?.volume.clone(),
            ),
            self.picture_sizes[index]?,
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
            picture_sizes: [None; 3],
            worker: MprWorker::start(frames, ctx),
            snapshot: None,
            error: None,
            target: None,
            views: std::array::from_fn(|i| ViewState {
                plane: PLANES[i],
                window,
                ..Default::default()
            }),
            annotations: std::array::from_fn(|_| Annotations::for_reformats()),
            scroll: [0.; 3],
            active: 0,
            tool: Tool::Crosshair,
            window,
            original_window,
            invert: false,
            layout: Layout::Grid,
            reformat_side: 512,
            last_input: std::time::Instant::now(),
        }
    }
    pub(super) fn request(&mut self, volume: &orv_core::volume::Volume, point: [f64; 3]) {
        let Some(point) = volume.clamp_world(point) else {
            return;
        };
        if self
            .target
            .is_some_and(|old| old.iter().zip(point).all(|(a, b)| (a - b).abs() < 1e-6))
        {
            return;
        }
        self.target = Some(point);
        self.last_input = std::time::Instant::now();
        self.worker.request_interactive(point, self.reformat_side);
    }
    pub(super) fn is_settled(&self) -> bool {
        self.snapshot
            .as_ref()
            .is_some_and(|s| !s.preview && s.generation == self.worker.generation())
    }
    pub(super) fn poll(&mut self) {
        if let Some(result) = self.worker.take() {
            match result {
                Ok(snapshot) => {
                    if snapshot.volume.is_color() {
                        self.window = snapshot.volume.frames()[0].default_window;
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
                        self.target = Some(snapshot.point);
                    }
                    self.snapshot = Some(snapshot);
                    self.error = None;
                }
                Err(error) => self.error = Some(error),
            }
        }
    }
    pub fn ui(&mut self, ui: &mut egui::Ui, frame: &eframe::Frame) {
        let content_width = ui.available_width();
        self.poll();
        if let Some(error) = &self.error {
            ui.add_space(20.);
            ui.heading("This series cannot be reformatted");
            ui.label(error);
            ui.label("You can continue exploring the images in 2D.");
            return;
        }
        let Some(snapshot) = self.snapshot.clone() else {
            ui.add_space(30.);
            ui.label("Checking volume geometry and preparing linked views…");
            ui.label(RichText::new("Images remain on this device.").color(theme::SILVER));
            return;
        };
        let color = snapshot.volume.is_color();
        if color && self.tool == Tool::Window {
            self.tool = Tool::Pan;
        }
        if ui.is_enabled() && !ui.ctx().egui_wants_keyboard_input() {
            ui.input(|i| {
                if !color && i.key_pressed(egui::Key::W) {
                    self.tool = Tool::Window;
                }
                if i.key_pressed(egui::Key::P) {
                    self.tool = Tool::Pan;
                }
                if i.key_pressed(egui::Key::Z) {
                    self.tool = Tool::Zoom;
                }
                if i.key_pressed(egui::Key::S) {
                    self.tool = Tool::Browse;
                }
                if i.key_pressed(egui::Key::I) {
                    self.invert = !self.invert;
                }
                if i.key_pressed(egui::Key::R) {
                    for view in &mut self.views {
                        view.zoom = 1.;
                        view.pan = [0.; 2];
                    }
                }
            });
        }
        crate::view_context::show(ui, &snapshot.volume.frames()[0]);
        if controls::tools(ui, &mut self.tool, true, !color, |ui| {
            egui::ComboBox::from_id_salt("mpr-layout")
                .wrap_mode(egui::TextWrapMode::Extend)
                .selected_text(self.layout.label())
                .show_ui(ui, |ui| {
                    for layout in [
                        Layout::Grid,
                        Layout::Columns,
                        Layout::Axial,
                        Layout::Coronal,
                        Layout::Sagittal,
                    ] {
                        ui.selectable_value(&mut self.layout, layout, layout.label());
                    }
                });
            if ui.button("Clear measurements").clicked() {
                for a in &mut self.annotations {
                    a.clear();
                }
            }
        }) {
            for view in &mut self.views {
                view.zoom = 1.;
                view.pan = [0.; 2];
            }
        }
        if color {
            controls::color_display(ui, &mut self.invert, snapshot.volume.color_info());
        } else {
            controls::window(
                ui,
                &mut self.window,
                &mut self.invert,
                self.original_window,
                snapshot.volume.frames()[0]
                    .scalar
                    .as_ref()
                    .map(|data| &**data.choices()),
            );
        }
        let pending = self.worker.generation() != snapshot.generation || snapshot.preview;
        let elapsed = self.last_input.elapsed();
        update_status(
            ui,
            pending && elapsed >= std::time::Duration::from_millis(250),
        );
        if pending && elapsed < std::time::Duration::from_millis(250) {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(250) - elapsed);
        }
        let size = Vec2::new(content_width, ui.available_height().max(160.));
        let (area, _) = ui.allocate_exact_size(size, egui::Sense::hover());
        let gap = 12.0;
        let mut paints = Vec::with_capacity(3);
        let layout = if self.layout == Layout::Grid && area.height() < 360. {
            Layout::Columns
        } else {
            self.layout
        };
        match layout {
            Layout::Columns => {
                for i in 0..3 {
                    let width = (area.width() - gap * 2.) / 3.;
                    let rect = Rect::from_min_size(
                        area.min + Vec2::new(i as f32 * (width + gap), 0.),
                        Vec2::new(width, area.height()),
                    );
                    paints.push(self.prepare_plane(ui, i, rect, &snapshot));
                }
            }
            Layout::Grid => {
                let cell = (area.size() - Vec2::splat(gap)) / 2.;
                for i in 0..3 {
                    let offset = Vec2::new(
                        (i % 2) as f32 * (cell.x + gap),
                        (i / 2) as f32 * (cell.y + gap),
                    );
                    paints.push(self.prepare_plane(
                        ui,
                        i,
                        Rect::from_min_size(area.min + offset, cell),
                        &snapshot,
                    ));
                }
                let rect = Rect::from_min_size(area.min + cell + Vec2::splat(gap), cell);
                let mut info =
                    ui.new_child(egui::UiBuilder::new().id_salt("mpr-guide").max_rect(rect));
                info.set_clip_rect(rect);
                self.guide(&mut info, &snapshot);
            }
            focus => {
                let index = match focus {
                    Layout::Coronal => 1,
                    Layout::Sagittal => 2,
                    _ => 0,
                };
                paints.push(self.prepare_plane(ui, index, area, &snapshot));
            }
        }
        // Apply the final shared display state after handling all panes' input.
        // Each callback owns its own texture/uniform slot; no late write can replace a sibling's data.
        for view in &mut self.views {
            view.window = self.window;
            view.invert = self.invert;
        }
        for paint in paints {
            self.finish_plane(paint, frame, &snapshot);
        }
    }
    fn guide(&self, ui: &mut egui::Ui, snapshot: &MprSnapshot) {
        theme::eyebrow(ui, "LINKED CROSSHAIR · mm (LPS)");
        ui.label(format!(
            "L {:.2}   P {:.2}   S {:.2}",
            snapshot.point[0], snapshot.point[1], snapshot.point[2]
        )).on_hover_text("Coordinates in the source LPS reference, in millimetres: left, posterior, superior. Negative values are valid and depend on the source origin; these are not distances from a body landmark.");
        ui.add_space(8.);
        ui.label("Crosshair: choose the same point in all views. Images [S] or ↑↓: move a plane. Pan [P] or Space drag: move the image.");
        controls::navigation_hint(ui, self.tool);
        ui.label(RichText::new("Measurements on resampled images may differ from the source images. ROI values follow the selected scalar or RGB representation.").small().color(theme::SILVER));
        let [x, y, z] = snapshot.volume.dimensions();
        ui.label(
            RichText::new(format!(
                "{x} × {y} × {z} source samples\nPersonal research only"
            ))
            .monospace()
            .size(10.)
            .color(theme::SILVER),
        );
    }

    pub(super) fn shared_view(&self) -> Option<(crate::rendered::ViewSpec, bool)> {
        let snapshot = self.snapshot.as_ref()?;
        let index = match self.layout {
            Layout::Axial => 0,
            Layout::Coronal => 1,
            Layout::Sagittal => 2,
            _ => self.active,
        };
        let mut view = self.views[index].clone();
        view.slice = 0;
        Some((
            crate::rendered::ViewSpec {
                view,
                point: Some(snapshot.point),
                reformat_side: snapshot.reformat_side,
                reconstruction: None,
                preferred_size: None,
            },
            snapshot.generation == self.worker.generation() && !snapshot.preview,
        ))
    }

    #[cfg(feature = "mcp")]
    pub(super) fn apply_shared_view(&mut self, scene: &orv_core::scene::SceneState) {
        use orv_core::Plane;
        self.active = match scene.view.plane {
            Plane::Coronal => 1,
            Plane::Sagittal => 2,
            _ => 0,
        };
        self.layout = match self.active {
            1 => Layout::Coronal,
            2 => Layout::Sagittal,
            _ => Layout::Axial,
        };
        self.window = scene.view.window;
        self.invert = scene.view.invert;
        self.views[self.active] = scene.view.clone();
        self.reformat_side = scene.reformat_side;
        if let Some(point) = scene.crosshair_mm {
            self.target = Some(point);
            self.last_input = std::time::Instant::now();
            self.worker.request(point, scene.reformat_side);
        }
    }
}

fn update_status(ui: &mut egui::Ui, pending: bool) {
    // Reserve exactly the same band while pending and settled. Never insert a
    // label that changes pane height and flips the adaptive layout on/off.
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 18.0), egui::Sense::hover());
    if pending {
        ui.painter().text(
            rect.right_center(),
            egui::Align2::RIGHT_CENTER,
            "Updating linked views…",
            egui::FontId::proportional(11.0),
            theme::SILVER,
        );
    }
}

#[cfg(test)]
mod layout_tests {
    use super::*;
    #[test]
    fn pending_linked_views_do_not_shift_the_panes_or_adaptive_layout() {
        for size in [egui::vec2(860., 640.), egui::vec2(1280., 840.)] {
            let ctx = egui::Context::default();
            let mut panes = Vec::new();
            for pending in [false, true, false, true] {
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, size)),
                        ..Default::default()
                    },
                    |ui| {
                        update_status(ui, pending);
                        let (rect, _) =
                            ui.allocate_exact_size(ui.available_size(), egui::Sense::hover());
                        panes.push(rect);
                    },
                );
                output.textures_delta.clear();
            }
            assert!(panes.windows(2).all(|p| p[0] == p[1]));
        }
    }
}

#[cfg(all(test, feature = "mcp"))]
mod preview_sharing_tests {
    use super::*;
    #[test]
    fn interactive_preview_is_withheld_from_exact_scene_publication_until_refined() {
        let mut session = MprSession::start(
            crate::mpr_worker::tests::frames(),
            WindowLevel::SOFT_TISSUE,
            egui::Context::default(),
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while session.snapshot.is_none() {
            session.poll();
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        let volume = session.snapshot.as_ref().unwrap().volume.clone();
        session.request(&volume, [1., 2., 9.]);
        while !session.snapshot.as_ref().unwrap().preview {
            session.poll();
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        let (_, ready) = session.shared_view().unwrap();
        assert!(!ready);
        while !session.is_settled() {
            session.poll();
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        let (spec, ready) = session.shared_view().unwrap();
        assert!(ready);
        assert_eq!(spec.reformat_side, 512);
        assert_eq!(spec.point, Some([1., 2., 9.]));
    }
}
