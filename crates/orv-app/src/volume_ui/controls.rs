// SPDX-License-Identifier: GPL-3.0-or-later
use eframe::egui;
use orv_core::{
    ViewState,
    volume_view::{PhysicalClip, Projection, TransferPreset, VolumeView},
};

pub(super) struct Context<'a> {
    pub source_range: Option<&'a super::DisplayRange>,
    pub initial: &'a super::DisplayRange,
    pub color: bool,
    pub choices: Option<&'a orv_core::scalar::VoiChoices>,
    pub bounds: [[f64; 3]; 2],
}

pub(super) fn show(
    ui: &mut egui::Ui,
    state: &mut VolumeView,
    display: &mut ViewState,
    preset: &mut TransferPreset,
    context: Context<'_>,
) -> bool {
    let Context {
        source_range,
        initial,
        color,
        choices,
        bounds,
    } = context;
    let mut fit = false;
    let narrow = ui.available_width() < 760.;
    crate::toolbar::row(ui, |ui| {
        ui.selectable_value(&mut state.projection, Projection::Composite, "Volume");
        ui.selectable_value(
            &mut state.projection,
            Projection::Maximum,
            if color { "RGB maxima" } else { "MIP" },
        );
        if color {
            crate::toolbar::label(ui, "sRGB color");
        } else {
            let mut apply_preset = false;
            egui::ComboBox::from_id_salt("volume-transfer")
                .wrap_mode(egui::TextWrapMode::Extend)
                .selected_text(
                    if source_range.is_some_and(|r| r.0 == display.window && r.1 == state.transfer)
                    {
                        "Source range"
                    } else if preset.values() == (display.window, state.transfer.clone()) {
                        super::preset_name(*preset)
                    } else {
                        "Custom"
                    },
                )
                .show_ui(ui, |ui| {
                    if let Some(range) = source_range
                        && ui
                            .selectable_label(
                                range.0 == display.window && range.1 == state.transfer,
                                "Source range",
                            )
                            .clicked()
                    {
                        display.window = range.0;
                        state.transfer = range.1.clone();
                        ui.close();
                    }
                    for option in [
                        TransferPreset::Bone,
                        TransferPreset::SoftTissue,
                        TransferPreset::Lung,
                    ] {
                        if ui
                            .selectable_label(
                                option.values() == (display.window, state.transfer.clone()),
                                super::preset_name(option),
                            )
                            .clicked()
                        {
                            *preset = option;
                            apply_preset = true;
                            ui.close();
                        }
                    }
                });
            if apply_preset {
                let (window, transfer) = preset.values();
                display.window = window;
                state.transfer = transfer;
            }
        }
        if ui.button("Fit [R]").clicked() {
            display.zoom = 1.;
            display.pan = [0.; 2];
            fit = true;
        }
        ui.checkbox(&mut display.invert, "Invert [I]");
        if ui.button("Reset display").on_hover_text("Restore this volume’s initial display and turn inversion off. Fit resets the camera separately.").clicked() {
            *preset = TransferPreset::Bone;
            display.window = initial.0;
            state.transfer = initial.1.clone();
            display.invert = false;
        }
        if !narrow {
            quality(ui, state);
        }
        clipping(ui, state, bounds);
    });
    crate::toolbar::row(ui, |ui| {
        if narrow {
            quality(ui, state);
        }
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
                    .range(-360.0..=360.0)
                    .suffix("°"),
            );
        }
        if state.projection == Projection::Composite {
            crate::toolbar::number(
                ui,
                "Density / mm",
                egui::DragValue::new(&mut state.transfer.opacity_per_mm)
                    .clamp_existing_to_range(false)
                    .speed(0.001)
                    .range(0.0..=10.0),
            );
        }
    });
    crate::toolbar::row(ui, |ui| {
        if !color {
            if let Some(choices) = choices {
                egui::ComboBox::from_id_salt("volume source presentation")
                    .selected_text(match display.window.function {
                        orv_core::VoiFunction::Lookup { index } => {
                            format!("Source LUT {}", index + 1)
                        }
                        _ => choices
                            .windows()
                            .iter()
                            .position(|value| *value == display.window)
                            .map(|index| format!("Source window {}", index + 1))
                            .unwrap_or_else(|| "Source display".into()),
                    })
                    .show_ui(ui, |ui| {
                        for (index, window) in choices.windows().iter().copied().enumerate() {
                            if ui
                                .selectable_label(
                                    display.window == window,
                                    format!("Source window {}", index + 1),
                                )
                                .clicked()
                            {
                                display.window = window;
                                ui.close();
                            }
                        }
                        for (index, _) in choices.luts().iter().enumerate() {
                            let window = orv_core::WindowLevel::lookup(index as u8);
                            if ui
                                .selectable_label(
                                    display.window == window,
                                    format!("Source LUT {}", index + 1),
                                )
                                .clicked()
                            {
                                display.window = window;
                                ui.close();
                            }
                        }
                    });
            }
            crate::controls::window_function(ui, &mut display.window);
            let lookup = matches!(
                display.window.function,
                orv_core::VoiFunction::Lookup { .. }
            );
            let minimum = if display.window.function == orv_core::VoiFunction::Linear {
                1.
            } else {
                0.000001
            };
            if !lookup {
                for (label, value) in [
                    ("Width", &mut display.window.width),
                    ("Level", &mut display.window.center),
                ] {
                    crate::toolbar::number(
                        ui,
                        label,
                        egui::DragValue::new(value)
                            .clamp_existing_to_range(false)
                            .speed(1.0)
                            .range(if label == "Width" {
                                minimum..=1_000_000.0
                            } else {
                                -1_000_000_000.0..=1_000_000_000.0
                            }),
                    );
                }
            }
        }
        if state.projection == Projection::Composite {
            crate::toolbar::number(
                ui,
                if color {
                    "Brightness min"
                } else {
                    "Opacity min"
                },
                egui::DragValue::new(&mut state.transfer.low)
                    .clamp_existing_to_range(false)
                    .speed(1.0)
                    .range(
                        if color { 0. } else { -1_000_000_000.0 }
                            ..=state
                                .transfer
                                .high
                                .next_down()
                                .min(state.transfer.high - 1e-20),
                    ),
            );
            crate::toolbar::number(
                ui,
                if color {
                    "Brightness max"
                } else {
                    "Opacity max"
                },
                egui::DragValue::new(&mut state.transfer.high)
                    .clamp_existing_to_range(false)
                    .speed(1.0)
                    .range(
                        state.transfer.low.next_up().max(state.transfer.low + 1e-20)..=if color {
                            255.
                        } else {
                            1_000_000_000.0
                        },
                    ),
            );
        }
    });
    fit
}

fn clipping(ui: &mut egui::Ui, state: &mut VolumeView, bounds: [[f64; 3]; 2]) {
    egui::containers::menu::MenuButton::new(if state.clip.is_some() {
        "Clipping · on"
    } else {
        "Clipping…"
    })
    .config(
        egui::containers::menu::MenuConfig::new()
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside),
    )
    .ui(ui, |ui| {
        ui.set_max_width(400.);
        ui.weak("Clip the projected source in LPS millimetres. Source pixels stay unchanged.");
        let mut enabled = state.clip.is_some();
        if ui
            .checkbox(&mut enabled, "Enable physical clipping")
            .changed()
        {
            state.clip = enabled.then_some(PhysicalClip {
                minimum_mm: bounds[0],
                maximum_mm: bounds[1],
            });
        }
        if let Some(clip) = &mut state.clip {
            for (axis, label) in ["L", "P", "S"].into_iter().enumerate() {
                ui.push_id(axis, |ui| {
                    crate::toolbar::row(ui, |ui| {
                        crate::toolbar::number(
                            ui,
                            &format!("{label} min"),
                            egui::DragValue::new(&mut clip.minimum_mm[axis])
                                .clamp_existing_to_range(false)
                                .speed(0.25)
                                .range(-1e7..=clip.maximum_mm[axis].next_down())
                                .suffix(" mm"),
                        );
                        crate::toolbar::number(
                            ui,
                            &format!("{label} max"),
                            egui::DragValue::new(&mut clip.maximum_mm[axis])
                                .clamp_existing_to_range(false)
                                .speed(0.25)
                                .range(clip.minimum_mm[axis].next_up()..=1e7)
                                .suffix(" mm"),
                        );
                    });
                });
            }
            if ui.button("Restore full source bounds").clicked() {
                *clip = PhysicalClip {
                    minimum_mm: bounds[0],
                    maximum_mm: bounds[1],
                };
            }
        }
    });
}

fn quality(ui: &mut egui::Ui, state: &mut VolumeView) {
    use orv_core::volume_view::Quality;
    egui::ComboBox::from_id_salt("volume-detail")
        .wrap_mode(egui::TextWrapMode::Extend)
        .width(90.)
        .selected_text(if state.quality == Quality::Refined {
            "Refined"
        } else {
            "Preview"
        })
        .show_ui(ui, |ui| {
            for (value, label) in [
                (Quality::Refined, "Refined"),
                (Quality::Interactive, "Preview"),
            ] {
                if ui
                    .selectable_value(&mut state.quality, value, label)
                    .clicked()
                {
                    ui.close();
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use orv_core::{WindowLevel, study::demo_study, volume::Volume};
    #[test]
    fn clipping_popup_accepts_numeric_input_without_closing_before_the_edit() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let volume = Volume::new(demo_study().series[0].frames.clone()).unwrap();
        let bounds = [[0.; 3], [88.9, 66.5, 4.5]];
        let mut state = VolumeView::fit(&volume);
        state.clip = Some(PhysicalClip {
            minimum_mm: bounds[0],
            maximum_mm: bounds[1],
        });
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
                    clipping(ui, &mut state, bounds);
                },
            );
            output.textures_delta.clear();
            output
        };
        let mut output = draw(vec![]);
        for target in ["Clipping · on", "88.9"] {
            let pos = crate::controls::tests::text_rect(&output, target).center();
            let _ = draw(vec![egui::Event::PointerMoved(pos)]);
            for pressed in [true, false] {
                let _ = draw(vec![egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                }]);
            }
            output = draw(vec![]);
        }
        assert!(crate::controls::tests::text_rect(&output, "L max").is_positive());
        let _ = draw(vec![egui::Event::Text("44.45".into())]);
        let output = draw(vec![egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }]);
        assert!(crate::controls::tests::text_rect(&output, "L max").is_positive());
        assert_eq!(state.clip.as_ref().unwrap().maximum_mm[0], 44.45);
    }
    #[test]
    fn volume_controls_align_dropdowns_and_keep_labels_with_values() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let volume = Volume::new(demo_study().series[0].frames.clone()).unwrap();
        for width in [1400., 560.] {
            let mut state = VolumeView::fit(&volume);
            state.projection = Projection::Composite;
            let initial = TransferPreset::Bone.values();
            state.transfer = initial.1.clone();
            let mut display = ViewState {
                window: initial.0,
                ..Default::default()
            };
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(width, 600.),
                    )),
                    ..Default::default()
                },
                |ui| {
                    show(
                        ui,
                        &mut state,
                        &mut display,
                        &mut TransferPreset::Bone,
                        Context {
                            source_range: None,
                            initial: &initial,
                            color: false,
                            choices: None,
                            bounds: volume.bounds(),
                        },
                    );
                },
            );
            output.textures_delta.clear();
            let text = |label| crate::controls::tests::text_rect(&output, label);
            if width > 1000. {
                let y = text("Volume").center().y;
                for label in ["MIP", "Bone", "Fit [R]", "Refined"] {
                    assert!(
                        (text(label).center().y - y).abs() < 0.75,
                        "Misaligned {label}"
                    );
                }
            }
            for (label, value) in [
                ("Width", "2000"),
                ("Level", "300"),
                ("Opacity min", "150"),
                ("Opacity max", "1200"),
            ] {
                assert!(
                    (text(label).center().y - text(value).center().y).abs() < 0.75,
                    "Separated {label}"
                );
                assert!(text(value).right() <= width);
            }
        }
    }
    #[test]
    fn idle_native_controls_preserve_exact_valid_applied_parameters() {
        let volume = Volume::new(demo_study().series[0].frames.clone()).unwrap();
        let mut state = VolumeView::fit(&volume);
        state.projection = Projection::Composite;
        state.camera.yaw_degrees = 300.123456789;
        state.camera.pitch_degrees = 120.123456789;
        state.camera.roll_degrees = -300.123456789;
        state.transfer.opacity_per_mm = 2.125;
        state.transfer.low = -2_000_000.;
        state.transfer.high = 3_000_000.;
        state.clip = Some(PhysicalClip {
            minimum_mm: [10.12345, 20.12345, 30.12345],
            maximum_mm: [130.12345, 160.12345, 75.12345],
        });
        let mut display = ViewState {
            window: WindowLevel {
                function: Default::default(),
                width: 450_000.,
                center: 200_000_000.,
            },
            ..Default::default()
        };
        state.validate().unwrap();
        let expected = (state.clone(), display.clone());
        let ctx = egui::Context::default();
        let mut preset = TransferPreset::Bone;
        for _ in 0..3 {
            let mut output = ctx.run_ui(Default::default(), |ui| {
                assert!(!show(
                    ui,
                    &mut state,
                    &mut display,
                    &mut preset,
                    Context {
                        source_range: None,
                        initial: &TransferPreset::Bone.values(),
                        color: false,
                        choices: None,
                        bounds: volume.bounds(),
                    }
                ));
                // The same shared window controls also receive source/MPR requests.
                crate::controls::window(
                    ui,
                    &mut display.window,
                    &mut display.invert,
                    WindowLevel::SOFT_TISSUE,
                    None,
                );
            });
            output.textures_delta.clear();
            assert_eq!((state.clone(), display.clone()), expected);
        }
    }
}
