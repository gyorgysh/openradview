// SPDX-License-Identifier: GPL-3.0-or-later
//! Embedded presentation over the MPR source; no second series materialization.
use super::*;
use orv_render::volume_source::GpuVolumeSource;

impl VolumeSession {
    fn surface_editor_available(&self, labels: &crate::labels::LabelUi, selected: usize) -> bool {
        self.material_editor.is_some()
            && self
                .volume
                .as_ref()
                .is_some_and(|volume| labels.can_start_surface_erase(selected, volume))
    }
    fn open_physical_editor(&mut self, labels: &mut crate::labels::LabelUi) {
        self.plane_settings.erase = false;
        labels.open_physical_tools();
        self.regions.select_manual();
        self.material_enabled = true;
        self.settings_open = false;
    }
    pub(crate) fn from_mpr(
        volume: Arc<Volume>,
        source: Option<Arc<GpuVolumeSource>>,
        prefer_source: bool,
        render: &eframe::egui_wgpu::RenderState,
        ctx: egui::Context,
    ) -> Self {
        let mut session = Self::new(prefer_source);
        session.upload = Some(match source {
            Some(source) if Arc::ptr_eq(source.volume(), &volume) => {
                Upload::from_source(source, render.target_format, ctx)
            }
            Some(_) => {
                session.error = Some("The 3D source does not match these MPR planes".into());
                return session;
            }
            None => Upload::from_volume(
                volume,
                render.device.clone(),
                render.queue.clone(),
                render.target_format,
                ctx,
            ),
        });
        session
    }

    pub(crate) fn embedded_ui(
        &mut self,
        ui: &mut egui::Ui,
        labels: &mut crate::labels::LabelUi,
        selected: usize,
        point: [f64; 3],
        link_epoch: u64,
        planes: Option<LinkedPlanes<'_>>,
    ) -> Option<[f64; 3]> {
        self.regions.set_link_epoch(link_epoch);
        if let Some(view) = &mut self.plane_view {
            view.set_link_epoch(link_epoch);
        }
        self.poll();
        let narrow = ui.available_width() < 250.;
        if narrow {
            crate::theme::eyebrow(ui, "3D · LOCAL");
        }
        let mut edit_requested = false;
        let mut surface_requested = false;
        let mut read_only_pick_requested = false;
        ui.horizontal_wrapped(|ui| {
            if !narrow {
                crate::theme::eyebrow(ui, "3D · LOCAL");
            }
            if ui
                .button(if narrow {
                    "Settings…"
                } else {
                    "3D settings…"
                })
                .clicked()
            {
                self.settings_open = true;
            }
            if ui.button(if narrow { "Fit" } else { "Fit 3D" }).clicked() {
                self.display.zoom = 1.;
                self.display.pan = [0.; 2];
                self.needs_fit = true;
            }
            if !narrow && ui.button("Physical edit…").clicked() {
                self.open_physical_editor(labels);
            }
            if !narrow
                && ui
                    .add_enabled(
                        self.surface_editor_available(labels, selected),
                        egui::Button::new("Surface stamp…"),
                    )
                    .on_hover_text(
                        "Requires Material view support and an applied manual label map. Show the selected ID in Labels before editing.",
                    )
                    .clicked()
            {
                surface_requested = true;
            }
        });
        if let Some(error) = &self.error {
            ui.label(error);
            return None;
        }
        if self.volume.is_none() {
            ui.label("Preparing 3D from the MPR source…");
            return None;
        }
        let mut open = self.settings_open;
        crate::dialogs::escape(ui.ctx(), &mut open);
        crate::dialogs::window("3D workbench settings", ui.ctx())
            .open(&mut open)
            .default_width(440.)
            .show(ui.ctx(), |ui| {
                if ui.button("Physical label edit…").clicked() {
                    edit_requested = true;
                }
                if ui.add_enabled(self.surface_editor_available(labels,selected),egui::Button::new("Surface erase stamp…")).on_hover_text("Requires Material view support and an applied manual label map. Show the selected ID in Labels before editing.").clicked() {surface_requested=true;}
                ui.weak("3D camera and appearance are independent of the MPR display.");
                if ui.add_enabled_ui(self.material_editor.is_some(),|ui|self.plane_settings.ui(ui)).inner {self.material_enabled=true;}
                if self.material_editor.is_none() {ui.small("Source planes are unavailable for this volume.");}
                self.regions.set_section_mode(self.plane_settings.active());
                self.show_controls(ui, labels, selected, false);
                if self.plane_settings.erase && self.regions.section_pick() {
                    self.plane_settings.erase=false;
                    read_only_pick_requested=true;
                }
                if !self.material_enabled {self.plane_settings.enabled=false;self.plane_settings.erase=false;}
                self.cuts.ui(ui, point);
                ui.checkbox(&mut self.cursor_visible, "Projected MPR cursor");
                ui.weak("The cursor and its three source axes are projected through the volume. These guides have no depth test and cannot select or edit a region.");
                ui.weak("This combined view stays local. Choose a single MPR pane or 3D mode for the supported picture and sharing views.");
                crate::dialogs::reveal_focus(ui);
            });
        self.settings_open = open && !edit_requested;
        self.regions.set_section_mode(self.plane_settings.active());
        if read_only_pick_requested {
            self.regions.select_face_pick();
        }
        if !self.plane_settings.active() {
            self.plane_view = None;
        }
        if edit_requested {
            self.plane_settings.erase = false;
            self.open_physical_editor(labels);
        }
        if surface_requested {
            if self.surface_editor_available(labels, selected) {
                labels.begin_surface_erase();
                self.regions.select_manual();
                self.material_enabled = true;
                self.plane_settings.erase = true;
                self.settings_open = false;
                if let Some(view) = &mut self.plane_view {
                    view.open_correction_panel();
                }
            } else {
                labels.open = true;
            }
        }
        if let Some(editor) = &mut self.material_editor {
            editor.window(ui.ctx());
        }
        let selection = self
            .volume
            .as_ref()
            .and_then(|volume| self.regions.selection(labels, selected, volume));
        if let (Some(view), Some(volume)) = (&mut self.plane_view, &self.volume) {
            view.sync_correction(
                labels.surface_erase_base(selected, volume),
                selected,
                link_epoch,
            );
        }
        let (rect, response) = ui.allocate_exact_size(
            Vec2::new(ui.available_width(), (ui.available_height() - 28.).max(32.)),
            egui::Sense::click_and_drag(),
        );
        let shown = self.paint_viewport(ui, rect, &response, selection, Some(point), planes);
        if let (Some(view), Some(volume)) = (&mut self.plane_view, &self.volume) {
            view.sync_correction(
                labels.surface_erase_base(selected, volume),
                selected,
                link_epoch,
            );
            if let Some(apply) = view.correction_window(ui.ctx(), &mut self.plane_settings.erase) {
                match labels.apply_surface_erase(
                    apply.proposal,
                    &apply.current,
                    &apply.brush,
                    selected,
                    volume,
                ) {
                    Ok(()) => (),
                    Err(error) => view.correction_notice(error),
                }
            }
        }
        if shown.is_some() && (labels.physical_preview_visible() || self.cuts.active()) {
            let painter = ui.painter_at(rect);
            let text = painter.layout_no_wrap(
                match (labels.physical_preview_visible(), self.cuts.active()) {
                    (true, true) => "PREVIEW ONLY · CUT",
                    (true, false) => "PHYSICAL PREVIEW ONLY",
                    _ => "LINKED CUT",
                }
                .into(),
                egui::FontId::monospace(10.),
                crate::theme::PAPER,
            );
            let position = rect.left_top() + egui::vec2(4., 4.);
            painter.rect_filled(
                egui::Rect::from_min_size(
                    position - egui::vec2(2., 2.),
                    text.size() + egui::vec2(4., 4.),
                ),
                2.,
                egui::Color32::BLACK,
            );
            painter.galley(position, text, crate::theme::PAPER);
        }
        if self.cursor_visible
            && let (Some(volume), Some((display, state, size))) = (&self.volume, shown)
            && let Ok(camera) = state
                .camera
                .frame(&display, f64::from(size[0]) / f64::from(size[1]))
        {
            paint_cursor(ui, rect, volume, &camera, point);
        }
        self.plane_view
            .as_mut()
            .and_then(source_planes::View::take_point)
            .or_else(|| self.regions.take_point())
    }
}

fn project(
    rect: egui::Rect,
    camera: &orv_core::volume_view::RayFrame,
    point: [f64; 3],
) -> Option<egui::Pos2> {
    let delta = std::array::from_fn::<_, 3, _>(|axis| point[axis] - camera.origin_mm[axis]);
    let coordinate = |basis: [f64; 3]| {
        let length_squared = basis.iter().map(|v| v * v).sum::<f64>();
        0.5 + delta.iter().zip(basis).map(|(a, b)| a * b).sum::<f64>() / length_squared
    };
    let uv = [coordinate(camera.right_mm), coordinate(camera.down_mm)];
    uv.iter()
        .all(|v| v.is_finite())
        .then(|| rect.min + rect.size() * egui::vec2(uv[0] as f32, uv[1] as f32))
        .filter(|pos| pos.is_finite())
}

fn paint_cursor(
    ui: &egui::Ui,
    rect: egui::Rect,
    volume: &Volume,
    camera: &orv_core::volume_view::RayFrame,
    point: [f64; 3],
) {
    let painter = ui.painter_at(rect);
    let stroke = egui::Stroke::new(1., crate::theme::SILVER.linear_multiply(0.7));
    for axis in 0..3 {
        let direction = std::array::from_fn(|index| if index == axis { 1. } else { 0. });
        if let Some(interval) = volume.ray_interval(point, direction) {
            let ends =
                interval.map(|t| std::array::from_fn(|index| point[index] + direction[index] * t));
            if let [Some(a), Some(b)] = ends.map(|p| project(rect, camera, p)) {
                painter.line_segment([a, b], stroke);
            }
        }
    }
    if let Some(position) = project(rect, camera, point) {
        painter.circle_stroke(position, 5., egui::Stroke::new(3., egui::Color32::BLACK));
        painter.circle_stroke(position, 5., egui::Stroke::new(1., crate::theme::PAPER));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orv_core::volume_view::Camera;

    #[test]
    fn physical_editor_click_is_retained_while_the_3d_source_is_still_preparing() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut session = VolumeSession::new(false);
        let mut labels = crate::labels::LabelUi::default();
        let mut draw = |events| {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(900., 600.),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| {
                    session.embedded_ui(ui, &mut labels, 0, [0.; 3], 1, None);
                },
            );
            output.textures_delta.clear();
            output
        };
        let output = draw(vec![]);
        let position = crate::controls::tests::text_rect(&output, "Physical edit…").center();
        let _ = draw(vec![egui::Event::PointerMoved(position)]);
        for pressed in [true, false] {
            let _ = draw(vec![egui::Event::PointerButton {
                pos: position,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            }]);
        }
        assert!(session.volume.is_none());
        assert!(labels.physical_dialog_open());
    }

    #[test]
    fn projected_cursor_matches_camera_rays_with_pan_zoom_roll_and_depth() {
        let rect = egui::Rect::from_min_size(egui::pos2(173., 281.), egui::vec2(353., 177.));
        for (yaw, pitch, roll) in [(0., 0., 0.), (90., -90., 180.), (29.5, -43.2, 71.)] {
            let camera = Camera {
                target_mm: [-137., 41., 950.],
                yaw_degrees: yaw,
                pitch_degrees: pitch,
                roll_degrees: roll,
                vertical_span_mm: 470.,
            };
            let view = ViewState {
                pan: [-0.21, 0.13],
                zoom: 2.7,
                ..Default::default()
            };
            let camera = camera.frame(&view, 1.73).unwrap();
            for uv in [[0., 0.], [0.5, 0.5], [0.18, 0.93], [1., 1.], [-0.2, 1.3]] {
                let (origin, direction) = camera.ray(uv);
                for depth in [-731., 0., 493.] {
                    let point = std::array::from_fn(|axis| origin[axis] + direction[axis] * depth);
                    let actual = project(rect, &camera, point).unwrap();
                    let expected = rect.min + rect.size() * egui::vec2(uv[0] as f32, uv[1] as f32);
                    assert!(actual.distance(expected) < 0.0001);
                }
            }
            assert!(project(rect, &camera, [f64::NAN, 0., 0.]).is_none());
        }
    }
}
