// SPDX-License-Identifier: GPL-3.0-or-later
//! Embedded presentation over the MPR source; no second series materialization.
use super::*;
use orv_render::volume_source::GpuVolumeSource;

impl VolumeSession {
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
    ) -> Option<[f64; 3]> {
        self.regions.set_link_epoch(link_epoch);
        self.poll();
        let narrow = ui.available_width() < 250.;
        if narrow {
            crate::theme::eyebrow(ui, "3D · LOCAL");
        }
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
                ui.weak("3D camera and appearance are independent of the MPR display.");
                self.show_controls(ui, labels, selected, false);
                ui.checkbox(&mut self.cursor_visible, "Projected MPR cursor");
                ui.weak("The cursor and its three source axes are projected through the volume. These guides have no depth test and cannot select or edit a region.");
                ui.weak("This combined view stays local. Choose a single MPR pane or 3D mode for the supported picture and sharing views.");
                crate::dialogs::reveal_focus(ui);
            });
        self.settings_open = open;
        if let Some(editor) = &mut self.material_editor {
            editor.window(ui.ctx());
        }
        let selection = self
            .volume
            .as_ref()
            .and_then(|volume| self.regions.selection(labels, selected, volume));
        let (rect, response) = ui.allocate_exact_size(
            Vec2::new(ui.available_width(), (ui.available_height() - 28.).max(32.)),
            egui::Sense::click_and_drag(),
        );
        let shown = self.paint_viewport(ui, rect, &response, selection, true);
        if self.cursor_visible
            && let (Some(volume), Some((display, state, size))) = (&self.volume, shown)
            && let Ok(camera) = state
                .camera
                .frame(&display, f64::from(size[0]) / f64::from(size[1]))
        {
            paint_cursor(ui, rect, volume, &camera, point);
        }
        self.regions.take_point()
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
