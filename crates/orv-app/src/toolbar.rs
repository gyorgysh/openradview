// SPDX-License-Identifier: GPL-3.0-or-later
//! One rhythm for view controls, with atomic label/value groups when wrapping.
use eframe::egui;
pub const HEIGHT: f32 = 32.;
const FIELD_WIDTH: f32 = 76.;

pub fn label(ui: &mut egui::Ui, text: impl Into<egui::WidgetText>) -> egui::Response {
    let galley = text.into().into_galley(
        ui,
        Some(egui::TextWrapMode::Extend),
        f32::INFINITY,
        egui::TextStyle::Body,
    );
    ui.add_sized([galley.size().x, HEIGHT], egui::Label::new(galley))
}

pub fn row<R>(
    ui: &mut egui::Ui,
    contents: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<R> {
    ui.scope(|ui| {
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
        let content_height = ui
            .text_style_height(&egui::TextStyle::Button)
            .max(ui.spacing().icon_width);
        ui.spacing_mut().interact_size.y = HEIGHT;
        ui.spacing_mut().button_padding =
            egui::vec2(10., ((HEIGHT - content_height) * 0.5).max(0.));
        ui.spacing_mut().item_spacing = egui::vec2(8., 8.);
        ui.allocate_ui_with_layout(
            egui::vec2(ui.available_width(), HEIGHT),
            // ComboBox 0.36 inherits this alignment in its inner UI. Center
            // alignment shifts its content/background below preceding buttons.
            egui::Layout::left_to_right(egui::Align::Min).with_main_wrap(true),
            contents,
        )
        .inner
    })
}

pub fn number(ui: &mut egui::Ui, label: &str, value: egui::DragValue<'_>) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(
        label.to_owned(),
        egui::TextStyle::Body.resolve(ui.style()),
        ui.visuals().text_color(),
    );
    ui.allocate_ui_with_layout(
        egui::vec2(galley.size().x + 6. + FIELD_WIDTH, HEIGHT),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.spacing_mut().item_spacing.x = 6.;
            let label = ui.label(label);
            ui.add_sized([FIELD_WIDTH, HEIGHT], value)
                .labelled_by(label.id)
        },
    )
    .inner
}
