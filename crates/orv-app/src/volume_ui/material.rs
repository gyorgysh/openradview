// SPDX-License-Identifier: GPL-3.0-or-later
//! Native-only optical appearance, deliberately outside source scene sharing.
use eframe::egui;
use orv_core::{
    ViewState,
    volume_material::{MaterialPoint, VolumeMaterial},
    volume_view::{Projection, VolumeView},
};
use orv_render::material_volume::Lighting;

#[derive(Clone, PartialEq)]
pub(super) struct OpticalStyle {
    pub material: VolumeMaterial,
    pub lighting: Lighting,
}
pub(super) struct MaterialEditor {
    pub open: bool,
    initial: Vec<MaterialPoint>,
    draft: Vec<MaterialPoint>,
    applied: OpticalStyle,
    error: Option<&'static str>,
}
impl MaterialEditor {
    pub fn new(initial: &super::DisplayRange) -> Result<Self, &'static str> {
        let transfer = &initial.1;
        let middle = ((f64::from(transfer.low) + f64::from(transfer.high)) * 0.5) as f32;
        let mut points = vec![MaterialPoint {
            value: transfer.low,
            luminance: 0.,
            extinction_per_mm: 0.,
        }];
        if middle > transfer.low && middle < transfer.high {
            points.push(MaterialPoint {
                value: middle,
                luminance: 0.7,
                extinction_per_mm: transfer.opacity_per_mm * 0.5,
            });
        }
        points.push(MaterialPoint {
            value: transfer.high,
            luminance: 1.,
            extinction_per_mm: transfer.opacity_per_mm,
        });
        let material = VolumeMaterial::new(&points)?;
        validate_knots(&material)?;
        Ok(Self {
            open: false,
            initial: points.clone(),
            draft: points,
            applied: OpticalStyle {
                material,
                lighting: Lighting::HEADLIGHT,
            },
            error: None,
        })
    }
    pub fn style(&self) -> OpticalStyle {
        self.applied.clone()
    }
    fn apply(&mut self) {
        if self.draft == self.applied.material.points() {
            self.error = None;
            return;
        }
        match VolumeMaterial::new(&self.draft).and_then(|m| validate_knots(&m).map(|()| m)) {
            Ok(material) => {
                self.applied.material = material;
                self.error = None;
            }
            Err(error) => self.error = Some(error),
        }
    }
    fn add_point(&mut self) {
        if self.draft.len() >= orv_core::volume_material::MAX_MATERIAL_POINTS {
            return;
        }
        let Some((index, pair)) = self.draft.windows(2).enumerate().max_by(|(_, a), (_, b)| {
            (f64::from(a[1].value) - f64::from(a[0].value))
                .total_cmp(&(f64::from(b[1].value) - f64::from(b[0].value)))
        }) else {
            return;
        };
        let value = ((f64::from(pair[0].value) + f64::from(pair[1].value)) * 0.5) as f32;
        if value <= pair[0].value || value >= pair[1].value {
            self.error = Some("No representable value fits between these points");
            return;
        }
        let point = MaterialPoint {
            value,
            luminance: (pair[0].luminance + pair[1].luminance) * 0.5,
            extinction_per_mm: (pair[0].extinction_per_mm + pair[1].extinction_per_mm) * 0.5,
        };
        self.draft.insert(index + 1, point);
    }
    fn point_table(&mut self, ui: &mut egui::Ui) {
        let can_remove = self.draft.len() > 2;
        let mut remove = None;
        egui::Grid::new("material-points")
            .spacing(egui::vec2(12., 8.))
            .show(ui, |ui| {
                ui.label("Point");
                let value_label = ui.label("Scalar value").id;
                let light_label = ui.label("Brightness").id;
                let opacity_label = ui.label("Opacity / mm").id;
                ui.end_row();
                for (index, point) in self.draft.iter_mut().enumerate() {
                    ui.label((index + 1).to_string());
                    crate::toolbar::number_field(
                        ui,
                        value_label,
                        egui::DragValue::new(&mut point.value)
                            .clamp_existing_to_range(false)
                            .speed(1.)
                            .range(-1e20..=1e20),
                    );
                    crate::toolbar::number_field(
                        ui,
                        light_label,
                        egui::DragValue::new(&mut point.luminance)
                            .speed(0.01)
                            .range(0. ..=1.),
                    );
                    crate::toolbar::number_field(
                        ui,
                        opacity_label,
                        egui::DragValue::new(&mut point.extinction_per_mm)
                            .speed(0.001)
                            .range(0. ..=10.),
                    );
                    if ui
                        .add_enabled(can_remove, egui::Button::new("Remove"))
                        .clicked()
                    {
                        remove = Some(index);
                    }
                    ui.end_row();
                }
            });
        if let Some(index) = remove {
            self.draft.remove(index);
        }
    }
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        state: &mut VolumeView,
        display: &mut ViewState,
        bounds: [[f64; 3]; 2],
        enabled: &mut bool,
    ) -> bool {
        let mut fit = false;
        crate::toolbar::row(ui, |ui| {
            if ui.selectable_label(false, "Volume").clicked() {
                *enabled = false;
                state.projection = Projection::Composite;
            }
            if ui.selectable_label(false, "MIP").clicked() {
                *enabled = false;
                state.projection = Projection::Maximum;
            }
            if ui.selectable_label(true,"Material…").on_hover_text("Edit local optical appearance. Choose Volume or MIP for picture export or agent sharing.").clicked() {self.open=true;}
            if ui
                .button(crate::actions::label(ui.ctx(), crate::actions::Action::Fit))
                .clicked()
            {
                display.zoom = 1.;
                display.pan = [0.; 2];
                fit = true;
            }
            let mut lighting = self.applied.lighting != Lighting::UNLIT;
            if ui.checkbox(&mut lighting, "Lighting").changed() {
                self.applied.lighting = if lighting {
                    Lighting::HEADLIGHT
                } else {
                    Lighting::UNLIT
                };
            }
            super::controls::quality(ui, state);
            super::controls::clipping(ui, state, bounds);
        });
        crate::controls::display_panel(ui, "material-camera-controls", |ui| {
            crate::toolbar::row(ui, |ui| {
                for (label, value) in [
                    ("Yaw", &mut state.camera.yaw_degrees),
                    ("Pitch", &mut state.camera.pitch_degrees),
                    ("Roll", &mut state.camera.roll_degrees),
                ] {
                    crate::toolbar::number(
                        ui,
                        label,
                        egui::DragValue::new(value)
                            .clamp_existing_to_range(false)
                            .speed(0.5)
                            .range(-360. ..=360.)
                            .suffix("°"),
                    );
                }
            });
        });
        let mut open = self.open;
        crate::dialogs::escape(ui.ctx(), &mut open);
        crate::dialogs::window("Volume material", ui.ctx())
            .open(&mut open)
            .default_width(600.)
            .show(ui.ctx(), |ui| {
                ui.label("Map scalar values to linear brightness and opacity per millimetre. Changes adjust appearance; they do not change source values.");
                egui::ScrollArea::vertical()
                    .max_height((ui.ctx().content_rect().height() - 280.).clamp(120., 300.))
                    .show(ui, |ui| {
                        self.point_table(ui);
                        crate::dialogs::reveal_focus(ui);
                    });
                ui.horizontal(|ui| {
                    if ui.add_enabled(self.draft.len()<orv_core::volume_material::MAX_MATERIAL_POINTS,egui::Button::new("Add point")).clicked() {self.add_point();}
                    if ui.button("Reset material").clicked() {self.draft=self.initial.clone();}
                });
                self.apply();
                ui.weak("Material view stays local. Choose Volume or MIP for picture export or agent sharing.");
                if let Some(error)=self.error {ui.label(error);ui.weak("The last valid material remains visible.");}
                crate::dialogs::reveal_focus(ui);
            });
        self.open = open;
        fit
    }
}
fn validate_knots(material: &VolumeMaterial) -> Result<(), &'static str> {
    if material
        .points()
        .windows(2)
        .any(|p| f64::from(p[1].value) - f64::from(p[0].value) < 1e-20)
    {
        return Err("Keep scalar values in increasing order and at least 1e-20 apart");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use orv_core::volume_view::TransferPreset;
    #[test]
    fn invalid_draft_preserves_the_visible_material_and_recovers_after_correction() {
        let mut editor = MaterialEditor::new(&TransferPreset::Bone.values()).unwrap();
        let original = editor.style();
        editor.draft[1].value = editor.draft[0].value;
        editor.apply();
        assert!(editor.error.is_some());
        assert!(editor.applied == original);
        editor.draft = editor.initial.clone();
        editor.draft[1].luminance = 0.4;
        editor.apply();
        assert!(editor.error.is_none());
        assert!(editor.applied != original);
        assert_eq!(editor.style().material.points()[1].luminance, 0.4);
    }
    #[test]
    fn inserting_material_points_preserves_order_and_respects_the_point_limit() {
        let mut editor = MaterialEditor::new(&TransferPreset::Bone.values()).unwrap();
        for _ in 0..100 {
            editor.add_point();
            editor.apply();
        }
        assert_eq!(
            editor.draft.len(),
            orv_core::volume_material::MAX_MATERIAL_POINTS
        );
        assert!(editor.error.is_none());
        assert_eq!(editor.draft, editor.applied.material.points());
        assert!(editor.draft.windows(2).all(|p| p[0].value < p[1].value));
    }
}

#[cfg(test)]
mod layout_tests {
    use super::*;
    use orv_core::{study::demo_study, volume::Volume, volume_view::TransferPreset};
    #[test]
    fn minimum_window_material_controls_fit_one_row_and_leave_room_for_the_image() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let volume = Volume::new(demo_study().series[0].frames.clone()).unwrap();
        let mut state = VolumeView::fit(&volume);
        state.projection = Projection::Composite;
        let mut view = ViewState::default();
        let mut enabled = true;
        let mut editor = MaterialEditor::new(&TransferPreset::Bone.values()).unwrap();
        for pass in 0..3 {
            let mut remaining = 0.;
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(860., 640.),
                    )),
                    ..Default::default()
                },
                |ui| {
                    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(
                        egui::Rect::from_min_max(egui::pos2(220., 240.), egui::pos2(828., 600.)),
                    ));
                    editor.ui(
                        &mut child,
                        &mut state,
                        &mut view,
                        volume.bounds(),
                        &mut enabled,
                    );
                    remaining = child.available_height();
                },
            );
            output.textures_delta.clear();
            if pass < 2 {
                continue;
            }
            let label = |text| crate::controls::tests::text_rect(&output, text);
            let baseline = label("Material…").center().y;
            for text in [
                "Volume",
                "MIP",
                "Fit [R]",
                "Lighting",
                "Refined",
                "Clipping…",
            ] {
                assert!(
                    (label(text).center().y - baseline).abs() < 1.,
                    "Wrapped material control: {text}"
                );
            }
            assert!(
                remaining > 240.,
                "Material controls left only {remaining} points for image and status"
            );
        }
    }
}
