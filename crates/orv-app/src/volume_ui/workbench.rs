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
        self.physical_region_pending = true;
        self.settings_open = false;
    }
    fn sync_physical_region(&mut self, labels: &mut crate::labels::LabelUi, selected: usize) {
        if !self.physical_region_pending {
            return;
        }
        if !labels.physical_dialog_open()
            && !labels.physical_drawing()
            && !labels.physical_preview_visible()
        {
            self.physical_region_pending = false;
            return;
        }
        if self
            .workbench_source
            .as_ref()
            .or(self.volume.as_ref())
            .is_some_and(|volume| {
                matches!(labels.region_maps(selected, volume).manual, Ok(Some(_)))
            })
        {
            labels.select_manual_for_physical();
            self.regions.select_manual();
            self.material_enabled = true;
            self.physical_region_pending = false;
        }
    }
    pub(crate) fn from_mpr(
        volume: Arc<Volume>,
        source: Option<Arc<GpuVolumeSource>>,
        prefer_source: bool,
        render: &eframe::egui_wgpu::RenderState,
        ctx: egui::Context,
    ) -> Self {
        let mut session = Self::new(prefer_source);
        // MPR can already edit this source while its 3D upload is preparing.
        session.workbench_source = Some(volume.clone());
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
        self.sync_physical_region(labels, selected);
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
        self.sync_physical_region(labels, selected);
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
        let painted = self.paint_viewport(ui, rect, &response, selection, Some(point), planes);
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
        let preview = labels.physical_preview_visible();
        let preview_in_3d = painted
            .region
            .as_ref()
            .is_some_and(|region| labels.is_physical_preview_region(region));
        if let Some(status) = workbench_status(
            preview,
            preview_in_3d,
            self.cuts.active(),
            painted.shown.is_some(),
        ) {
            let painter = ui.painter_at(rect);
            let text = painter.layout_no_wrap(
                status.into(),
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
            && let (Some(volume), Some((display, state, size))) = (&self.volume, painted.shown)
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

fn workbench_status(
    preview: bool,
    preview_in_3d: bool,
    cut: bool,
    shown: bool,
) -> Option<&'static str> {
    match (preview, preview_in_3d, cut, shown) {
        (true, true, true, _) => Some("PREVIEW ONLY · CUT"),
        (true, true, false, _) => Some("PHYSICAL PREVIEW ONLY"),
        (true, false, true, _) => Some("PREVIEW HIDDEN · CUT"),
        (true, false, false, _) => Some("PREVIEW NOT IN 3D"),
        (false, _, true, true) => Some("LINKED CUT"),
        _ => None,
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
    use orv_core::{labels::Labels, study::demo_study, volume_view::Camera};

    #[test]
    fn preview_status_distinguishes_the_painted_region_from_a_hidden_proposal() {
        assert_eq!(
            workbench_status(true, true, false, true),
            Some("PHYSICAL PREVIEW ONLY")
        );
        assert_eq!(
            workbench_status(true, false, false, true),
            Some("PREVIEW NOT IN 3D")
        );
        assert_eq!(
            workbench_status(true, false, false, false),
            Some("PREVIEW NOT IN 3D")
        );
        assert_eq!(
            workbench_status(true, true, true, true),
            Some("PREVIEW ONLY · CUT")
        );
        assert_eq!(
            workbench_status(true, false, true, true),
            Some("PREVIEW HIDDEN · CUT")
        );
        assert_eq!(
            workbench_status(false, false, true, true),
            Some("LINKED CUT")
        );
        assert_eq!(workbench_status(false, false, false, true), None);
    }

    #[test]
    fn physical_editor_waits_for_a_matching_manual_map_before_selecting_it() {
        let mut frames = demo_study().series.remove(0).frames;
        frames.truncate(2);
        let source = Arc::new(Volume::new(frames).unwrap());
        let other = Arc::new(Volume::new(source.frames().to_vec()).unwrap());
        let mut session = VolumeSession::new(false);
        let mut labels = crate::labels::LabelUi::default();
        session.volume = Some(source.clone());
        session.open_physical_editor(&mut labels);
        session.sync_physical_region(&mut labels, 0);
        assert!(!session.regions.enabled());
        assert!(!session.material_enabled);

        labels.current = Some(Arc::new(Labels::empty(other).unwrap()));
        session.sync_physical_region(&mut labels, 0);
        assert!(!session.regions.enabled());
        labels.current = Some(Arc::new(Labels::empty(source).unwrap()));
        session.sync_physical_region(&mut labels, 1);
        assert!(!session.regions.enabled());
        session.sync_physical_region(&mut labels, 0);
        assert!(session.regions.enabled());
        assert!(session.material_enabled);
        assert!(!session.physical_region_pending);
    }

    #[test]
    fn closing_physical_editor_without_a_map_cancels_the_deferred_selection() {
        let mut frames = demo_study().series.remove(0).frames;
        frames.truncate(2);
        let source = Arc::new(Volume::new(frames).unwrap());
        let mut session = VolumeSession::new(false);
        let mut labels = crate::labels::LabelUi::default();
        session.volume = Some(source.clone());
        session.open_physical_editor(&mut labels);
        labels.end_physical_tools();
        session.sync_physical_region(&mut labels, 0);
        labels.current = Some(Arc::new(Labels::empty(source).unwrap()));
        session.sync_physical_region(&mut labels, 0);
        assert!(!session.regions.enabled());
        assert!(!session.physical_region_pending);
    }

    #[test]
    fn physical_edit_can_select_matching_labels_while_3d_is_preparing() {
        let mut frames = demo_study().series.remove(0).frames;
        frames.truncate(2);
        let source = Arc::new(Volume::new(frames).unwrap());
        let mut session = VolumeSession::new(false);
        session.workbench_source = Some(source.clone());
        let mut labels = crate::labels::LabelUi::default();
        labels.current = Some(Arc::new(Labels::empty(source).unwrap()));
        session.open_physical_editor(&mut labels);
        session.sync_physical_region(&mut labels, 0);
        assert!(session.volume.is_none());
        assert!(session.regions.enabled() && session.material_enabled);
        assert!(!session.physical_region_pending);
    }

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
