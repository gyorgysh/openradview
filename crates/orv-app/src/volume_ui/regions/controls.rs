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
    pub geometry: Geometry,
    pub surface_lighting: bool,
    pub pick: bool,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            enabled: false,
            group: Group::Manual,
            supplied: 0,
            luminance: 0.8,
            geometry: Geometry::Cells,
            surface_lighting: true,
            pick: false,
        }
    }
}
impl Settings {
    pub fn select_manual(&mut self) {
        self.enabled = true;
        self.group = Group::Manual;
        self.pick = false;
    }
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
                .on_hover_text("One opaque region. Fractional masks use half the maximum. Physical label tools are in MPR + 3D.")
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
                egui::containers::menu::MenuButton::new(format!("{}{}…", self.geometry.label(), if self.pick { " · pick" } else { "" }))
                    .config(egui::containers::menu::MenuConfig::new().close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside))
                    .ui(ui, |ui| {
                        ui.set_max_width(400.);
                        crate::toolbar::row(ui, |ui| {
                            for geometry in [Geometry::Cells, Geometry::Faces] {
                                ui.selectable_value(&mut self.geometry, geometry, geometry.label());
                            }
                            crate::toolbar::number(ui, "Brightness", egui::DragValue::new(&mut self.luminance).range(0. ..=1.).speed(0.01).fixed_decimals(2));
                        });
                        if self.geometry == Geometry::Faces {
                            ui.checkbox(&mut self.surface_lighting, "Surface lighting");
                            ui.checkbox(&mut self.pick, "Pick surface").on_hover_text("Click a visible, completed cell face to inspect its source coordinates. In the workbench the MPR crosshair follows the selected source-cell centre. No label is changed.");
                            ui.small("Cell faces follow the source grid. Unknown/missing interfaces stay open; clipping adds no cap. This is derived geometry, not inferred anatomy.");
                        } else { self.pick = false; }
                    });
            }
            if ui.button("Labels…").clicked() {
                labels.open = true;
            }
        });
        if self.enabled && ui.ctx().content_rect().height() >= 700. {
            ui.small(
                "One opaque region. Fractional masks use half the maximum. Physical label tools are in MPR + 3D.",
            );
        }
        let waiting = self.enabled
            && match self.group {
                Group::Manual => maps.manual.is_err(),
                Group::Supplied => maps.supplied.is_err(),
            };
        (result, waiting)
    }
    pub fn current(
        &mut self,
        labels: &LabelUi,
        maps: &RegionMaps,
    ) -> (Option<RegionSelection>, bool) {
        if !self.enabled {
            return (None, false);
        }
        let result = match self.group {
            Group::Manual => maps.manual.as_ref().map(|map| {
                map.as_ref().and_then(|map| {
                    RegionSelection::new(
                        RegionMap::Manual(map.clone()),
                        u16::from(labels.manual_region_number()),
                    )
                    .ok()
                })
            }),
            Group::Supplied => maps.supplied.as_ref().map(|map| {
                map.as_ref().and_then(|map| {
                    if !map.infos().any(|info| info.number == self.supplied) {
                        self.supplied = map.infos().next()?.number;
                    }
                    RegionSelection::new(RegionMap::Supplied(map.clone()), self.supplied).ok()
                })
            }),
        };
        let waiting = result.is_err();
        (result.ok().flatten(), waiting)
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
    fn geometry_menu_keeps_direct_choices_and_picking_available_after_selection() {
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
        let mut time = 0.;
        let mut draw = |events| {
            time += 0.1;
            let mut output = ctx.run_ui(
                egui::RawInput {
                    events,
                    time: Some(time),
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(900., 600.),
                    )),
                    ..Default::default()
                },
                |ui| {
                    settings.ui(ui, &mut labels, 0, &source, &mut material);
                },
            );
            output.textures_delta.clear();
            output
        };
        let mut output = draw(vec![]);
        for target in ["Mask cells…", "Cell faces", "Pick surface"] {
            let position = crate::controls::tests::text_rect(&output, target).center();
            let _ = draw(vec![egui::Event::PointerMoved(position)]);
            for pressed in [true, false] {
                let _ = draw(vec![egui::Event::PointerButton {
                    pos: position,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                }]);
            }
            output = draw(vec![]);
        }
        assert!(crate::controls::tests::text_rect(&output, "Brightness").is_positive());
        assert!(settings.geometry == Geometry::Faces && settings.pick);
    }

    #[test]
    fn selection_without_settings_ui_tracks_the_current_manual_id_revision_and_source() {
        let mut frames = demo_study().series.remove(0).frames;
        frames.truncate(2);
        let source = Arc::new(Volume::new(frames).unwrap());
        let mut labels = LabelUi::default();
        let original = Arc::new(Labels::empty(source.clone()).unwrap());
        labels.current = Some(original.clone());
        let mut settings = Settings {
            enabled: true,
            ..Default::default()
        };
        let first = settings
            .current(&labels, &labels.region_maps(0, &source))
            .0
            .unwrap();
        labels.set_manual_region_number(7);
        let selected = settings
            .current(&labels, &labels.region_maps(0, &source))
            .0
            .unwrap();
        assert_eq!(selected.number, 7);
        assert!(!selected.same(&first));
        let mut edited = original.try_clone(|| false).unwrap();
        edited
            .threshold(None, [-100_000., 100_000.], 7, || false)
            .unwrap();
        labels.current = Some(Arc::new(edited));
        let changed = settings
            .current(&labels, &labels.region_maps(0, &source))
            .0
            .unwrap();
        assert!(!changed.same(&selected));
        assert!(Arc::ptr_eq(&changed.map.source(), &source));
        let copy = Arc::new(Volume::new(source.frames().to_vec()).unwrap());
        assert!(
            settings
                .current(&labels, &labels.region_maps(0, &copy))
                .0
                .is_none()
        );
        assert!(
            settings
                .current(&labels, &labels.region_maps(1, &source))
                .0
                .is_none()
        );
    }

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
            for text in ["Region in 3D", "Manual", "ID", "Mask cells…", "Labels…"] {
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
