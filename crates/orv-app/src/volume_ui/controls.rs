// SPDX-License-Identifier: GPL-3.0-or-later
use eframe::egui;
use orv_core::{
    ViewState,
    volume_view::{Projection, TransferPreset, VolumeView},
};

pub(super) fn show(
    ui: &mut egui::Ui,
    state: &mut VolumeView,
    display: &mut ViewState,
    preset: &mut TransferPreset,
    source_range: Option<&super::DisplayRange>,
    initial: &super::DisplayRange,
    color: bool,
) -> bool {
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
                            1.0..=1_000_000.0
                        } else {
                            -1_000_000_000.0..=1_000_000_000.0
                        }),
                );
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
                    .range(if color { 0. } else { -1_000_000_000.0 }..=state.transfer.high - 1.),
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
                    .range(state.transfer.low + 1.0..=if color { 255. } else { 1_000_000_000.0 }),
            );
        }
    });
    fit
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
                        None,
                        &initial,
                        false,
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
        let mut display = ViewState {
            window: WindowLevel {
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
                    None,
                    &TransferPreset::Bone.values(),
                    false
                ));
                // The same shared window controls also receive source/MPR requests.
                crate::controls::window(
                    ui,
                    &mut display.window,
                    &mut display.invert,
                    WindowLevel::SOFT_TISSUE,
                );
            });
            output.textures_delta.clear();
            assert_eq!((state.clone(), display.clone()), expected);
        }
    }
}
