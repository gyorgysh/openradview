// SPDX-License-Identifier: GPL-3.0-or-later
use crate::viewport::Tool;
use eframe::egui;
use orv_core::WindowLevel;
#[cfg(test)]
pub(crate) mod tests;

pub fn tools(
    ui: &mut egui::Ui,
    tool: &mut Tool,
    crosshair: bool,
    scalar: bool,
    extra: impl FnOnce(&mut egui::Ui),
) -> bool {
    let previous = *tool;
    let mut fit = false;
    let narrow = ui.available_width() < if crosshair { 900. } else { 650. };
    let mut extra = Some(extra);
    crate::toolbar::row(ui, |ui| {
        if crosshair {
            ui.selectable_value(tool, Tool::Crosshair, "Crosshair");
        }
        ui.selectable_value(tool, Tool::Pan, "Pan [P]");
        ui.selectable_value(tool, Tool::Zoom, "Zoom [Z]");
        ui.selectable_value(tool, Tool::Browse, "Images [S]");
        ui.add_enabled_ui(scalar, |ui| {
            ui.selectable_value(tool, Tool::Window, "Contrast [W]");
        });
        if !narrow {
            measurement_tool(ui, tool);
        }
        fit = ui.button("Fit [R]").clicked();
        if !narrow && let Some(extra) = extra.take() {
            extra(ui);
        }
    });
    if narrow {
        crate::toolbar::row(ui, |ui| {
            measurement_tool(ui, tool);
            if let Some(extra) = extra.take() {
                extra(ui);
            }
        });
    }
    if *tool != previous || fit {
        ui.ctx().request_repaint();
    }
    fit
}
fn measurement_tool(ui: &mut egui::Ui, tool: &mut Tool) {
    egui::ComboBox::from_id_salt("measurement-tool")
        .wrap_mode(egui::TextWrapMode::Extend)
        .width(120.)
        .selected_text(match tool {
            Tool::Distance => "Distance",
            Tool::Angle => "Angle",
            Tool::Roi => "ROI stats",
            _ => "Measure",
        })
        .show_ui(ui, |ui| {
            for (value, label) in [
                (Tool::Distance, "Distance · 2 points"),
                (Tool::Angle, "Angle · 3 points"),
                (Tool::Roi, "ROI stats · 2 corners"),
            ] {
                if ui.selectable_value(tool, value, label).clicked() {
                    ui.close();
                }
            }
        });
}
pub fn window(
    ui: &mut egui::Ui,
    window: &mut WindowLevel,
    invert: &mut bool,
    original: WindowLevel,
    choices: Option<&orv_core::scalar::VoiChoices>,
) {
    crate::toolbar::row(ui, |ui| {
        egui::ComboBox::from_id_salt("preset")
            .wrap_mode(egui::TextWrapMode::Extend)
            .selected_text(match window.function {
                orv_core::VoiFunction::Lookup { index } => format!("Source LUT {}", index + 1),
                _ => preset_name(*window, original).to_owned(),
            })
            .show_ui(ui, |ui| {
                if let Some(choices) = choices {
                    for (index, value) in choices.windows().iter().copied().enumerate() {
                        if ui
                            .selectable_label(
                                *window == value,
                                format!("Source window {}", index + 1),
                            )
                            .clicked()
                        {
                            *window = value;
                            ui.close();
                        }
                    }
                    for (index, _) in choices.luts().iter().enumerate() {
                        let value = WindowLevel::lookup(index as u8);
                        if ui
                            .selectable_label(*window == value, format!("Source LUT {}", index + 1))
                            .clicked()
                        {
                            *window = value;
                            ui.close();
                        }
                    }
                }
                for (label, value) in [
                    ("Original image", original),
                    ("Soft tissue", WindowLevel::SOFT_TISSUE),
                    ("Lung", WindowLevel::LUNG),
                    ("Bone", WindowLevel::BONE),
                    ("Brain", WindowLevel::BRAIN),
                ] {
                    if ui.selectable_label(*window == value, label).clicked() {
                        *window = value;
                        ui.close();
                    }
                }
            });
        window_function(ui, window);
        let lookup = matches!(window.function, orv_core::VoiFunction::Lookup { .. });
        let minimum = if window.function == orv_core::VoiFunction::Linear {
            1.
        } else {
            0.000001
        };
        if !lookup {
            crate::toolbar::number(
                ui,
                "Width",
                egui::DragValue::new(&mut window.width)
                    .clamp_existing_to_range(false)
                    .range(minimum..=100_000.0)
                    .speed(2.0),
            )
            .on_hover_text("Contrast range (window width)");
            crate::toolbar::number(
                ui,
                "Level",
                egui::DragValue::new(&mut window.center)
                    .clamp_existing_to_range(false)
                    .range(-1_000_000.0..=1_000_000.0)
                    .speed(1.0),
            )
            .on_hover_text("Brightness centre (window level)");
        }

        ui.checkbox(invert, "Invert [I]");
        if ui.button("Reset display").on_hover_text("Restore the original image's window/level and turn inversion off. Fit resets pan/zoom separately.").clicked() {
            *window = original;
            *invert = false;
        }
    });
}
pub fn window_function(ui: &mut egui::Ui, window: &mut WindowLevel) {
    use orv_core::VoiFunction;
    let lookup = matches!(window.function, VoiFunction::Lookup { .. });
    if lookup {
        return;
    }
    egui::ComboBox::from_id_salt("window function")
        .selected_text(match window.function {
            VoiFunction::Linear => "DICOM linear",
            VoiFunction::LinearExact => "Exact linear",
            VoiFunction::Sigmoid => "Sigmoid",
            VoiFunction::Lookup { .. } => "Source table",
        })
        .show_ui(ui, |ui| {
            for (label, function) in [
                ("DICOM linear", VoiFunction::Linear),
                ("Exact linear", VoiFunction::LinearExact),
                ("Sigmoid", VoiFunction::Sigmoid),
            ] {
                if ui
                    .selectable_label(window.function == function, label)
                    .clicked()
                {
                    window.function = function;
                    if function == VoiFunction::Linear {
                        window.width = window.width.max(1.);
                    }
                    ui.close();
                }
            }
        });
}

pub fn color_display(
    ui: &mut egui::Ui,
    invert: &mut bool,
    info: Option<&orv_core::color::ColorInfo>,
) {
    crate::toolbar::row(ui, |ui| {
        crate::toolbar::label(
            ui,
            info.map(|info| format!("RGB · {}-bit codes · {}", info.bits, info.presentation()))
                .unwrap_or_else(|| "sRGB color presentation".into()),
        );
        ui.checkbox(invert, "Invert [I]");
        if ui.button("Reset display").clicked() {
            *invert = false;
        }
    });
}

fn preset_name(window: WindowLevel, original: WindowLevel) -> &'static str {
    if window == original {
        return "Original image";
    }
    for (name, value) in [
        ("Soft tissue", WindowLevel::SOFT_TISSUE),
        ("Lung", WindowLevel::LUNG),
        ("Bone", WindowLevel::BONE),
        ("Brain", WindowLevel::BRAIN),
    ] {
        if window == value {
            return name;
        }
    }
    "Custom contrast"
}

pub fn navigation_hint(ui: &mut egui::Ui, tool: Tool) {
    let scroll = match tool {
        Tool::Browse | Tool::Crosshair => "Scroll: images / plane",
        Tool::Zoom => "Scroll: zoom",
        _ => "Scroll: move image",
    };
    ui.label(egui::RichText::new(format!("{scroll} · Pinch / Cmd/Ctrl scroll: zoom · Space drag: move · ↑↓: images · [S] browse slices" )).size(11.0).color(crate::theme::SILVER));
}
