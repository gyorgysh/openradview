// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use crate::labels::{RegionMap, RegionMaps};
#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Group {
    #[default]
    Manual,
    Supplied,
}

pub(super) struct Settings {
    pub enabled: bool,
    group: Group,
    supplied: u16,
    pub luminance: f32,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            enabled: false,
            group: Group::Manual,
            supplied: 0,
            luminance: 0.8,
        }
    }
}
impl Settings {
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        labels: &mut LabelUi,
        selected: usize,
        source: &Arc<Volume>,
        material: &mut bool,
    ) -> (Option<RegionSelection>, bool) {
        if source.is_color() {
            self.enabled = false;
            return (None, false);
        }
        let maps = labels.region_maps(selected, source);
        let mut result = None;
        crate::toolbar::row(ui, |ui| {
            if ui.checkbox(&mut self.enabled, "Region in 3D")
                .on_hover_text("One opaque region. Fractional masks use half the maximum. Painting stays in 2D.")
                .changed() && self.enabled {
                *material = true;
                if matches!(&maps.manual, Ok(None)) && matches!(&maps.supplied, Ok(Some(_))) {
                    self.group = Group::Supplied;
                }
            }
            if self.enabled {
                egui::ComboBox::from_id_salt("volume-region-group")
                    .selected_text(if self.group == Group::Manual {
                        "Manual"
                    } else {
                        "Supplied"
                    })
                    .width(80.)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.group, Group::Manual, "Manual");
                        ui.selectable_value(&mut self.group, Group::Supplied, "Supplied");
                    });
                match self.selection(ui, labels, &maps) {
                    Ok(selection) => result = selection,
                    Err(error) => {
                        ui.small(error);
                    }
                }
                crate::toolbar::number(
                    ui,
                    "Brightness",
                    egui::DragValue::new(&mut self.luminance)
                        .range(0. ..=1.)
                        .speed(0.01)
                        .fixed_decimals(2),
                );
            }
            if ui.button("Labels…").clicked() {
                labels.open = true;
            }
        });
        if self.enabled && ui.ctx().content_rect().height() >= 700. {
            ui.small(
                "One opaque region. Fractional masks use half the maximum. Painting stays in 2D.",
            );
        }
        let waiting = self.enabled
            && match self.group {
                Group::Manual => maps.manual.is_err(),
                Group::Supplied => maps.supplied.is_err(),
            };
        (result, waiting)
    }
    fn selection(
        &mut self,
        ui: &mut egui::Ui,
        labels: &mut LabelUi,
        maps: &RegionMaps,
    ) -> Result<Option<RegionSelection>, &'static str> {
        let (map, number) = match self.group {
            Group::Manual => {
                let Some(map) = maps.manual.as_ref().map_err(|v| *v)? else {
                    return Err("Create or load manual labels");
                };
                let mut number = labels.manual_region_number();
                if crate::toolbar::number(
                    ui,
                    "ID",
                    egui::DragValue::new(&mut number).range(1..=255),
                )
                .changed()
                {
                    labels.set_manual_region_number(number);
                }
                (RegionMap::Manual(map.clone()), u16::from(number))
            }
            Group::Supplied => {
                let Some(map) = maps.supplied.as_ref().map_err(|v| *v)? else {
                    return Err("Load matching supplied masks");
                };
                if !map.infos().any(|v| v.number == self.supplied) {
                    self.supplied = map.infos().next().ok_or("No supplied IDs")?.number;
                }
                egui::ComboBox::from_id_salt("volume-region-id")
                    .selected_text(format!("ID {}", self.supplied))
                    .width(90.)
                    .show_ui(ui, |ui| {
                        for info in map.infos() {
                            ui.selectable_value(
                                &mut self.supplied,
                                info.number,
                                format!("ID {}", info.number),
                            );
                        }
                    });
                (RegionMap::Supplied(map.clone()), self.supplied)
            }
        };
        RegionSelection::new(map, number).map(Some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orv_core::{labels::Labels, study::demo_study, volume_view::TransferPreset};

    #[test]
    fn minimum_window_region_controls_stay_visible_and_leave_room_for_the_image() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut frames = demo_study().series.remove(0).frames;
        frames.truncate(2);
        let source = Arc::new(Volume::new(frames).unwrap());
        let mut labels = LabelUi::default();
        labels.current = Some(Arc::new(Labels::empty(source.clone()).unwrap()));
        let mut settings = Settings {
            enabled: true,
            ..Default::default()
        };
        let mut material = true;
        let mut editor =
            super::super::super::material::MaterialEditor::new(&TransferPreset::Bone.values())
                .unwrap();
        let mut state = VolumeView::fit(&source);
        let mut display = ViewState::default();
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
                        &mut display,
                        source.bounds(),
                        &mut material,
                    );
                    let (selection, waiting) =
                        settings.ui(&mut child, &mut labels, 0, &source, &mut material);
                    assert!(selection.is_some());
                    assert!(!waiting);
                    remaining = child.available_height();
                },
            );
            output.textures_delta.clear();
            if pass < 2 {
                continue;
            }
            for text in ["Region in 3D", "Manual", "ID", "Brightness", "Labels…"] {
                let rect = crate::controls::tests::text_rect(&output, text);
                assert!(
                    rect.left() >= 220. && rect.right() <= 828.,
                    "Clipped region control {text}: {rect:?}"
                );
            }
            assert!(
                remaining > 200.,
                "Only {remaining} points remain for the image and status"
            );
        }
    }
}
