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
            number_field(ui, label.id, value)
        },
    )
    .inner
}

/// A table cell uses the same commit/focus behavior as an inline numeric group.
pub fn number_field(
    ui: &mut egui::Ui,
    label: egui::Id,
    value: egui::DragValue<'_>,
) -> egui::Response {
    let focused = ui.memory(|memory| memory.focused());
    let commit = ui.input(|input| {
        input.key_pressed(egui::Key::Enter) && input.modifiers == egui::Modifiers::NONE
    });
    let response = ui
        .add_sized([FIELD_WIDTH, HEIGHT], value)
        .labelled_by(label);
    // DragValue's single-line editor surrenders focus on Enter. Keep the
    // committed field as the Tab origin rather than restarting at root.
    if commit
        && focused == Some(response.id)
        && response.lost_focus()
        && ui.memory(|memory| memory.focused()).is_none()
    {
        response.request_focus();
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn numeric_enter_keeps_tab_origin_and_next_field_then_button_receive_focus() {
        let ctx = egui::Context::default();
        let mut values = [1u32, 2u32];
        let mut ids = [egui::Id::NULL; 3];
        let mut render = |events, request_first: bool| {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    events,
                    ..Default::default()
                },
                |ui| {
                    ids[0] = number(ui, "First", egui::DragValue::new(&mut values[0])).id;
                    ids[1] = number(ui, "Second", egui::DragValue::new(&mut values[1])).id;
                    ids[2] = ui.button("Continue").id;
                    if request_first {
                        ctx.memory_mut(|memory| memory.request_focus(ids[0]));
                    }
                },
            );
            output.textures_delta.clear();
            ids
        };
        let ids = render(vec![], true);
        render(vec![], false);
        let press = |key| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        render(vec![press(egui::Key::Enter)], false);
        assert_eq!(ctx.memory(|memory| memory.focused()), Some(ids[0]));
        render(vec![press(egui::Key::Tab)], false);
        assert_eq!(ctx.memory(|memory| memory.focused()), Some(ids[1]));
        render(vec![press(egui::Key::Tab)], false);
        assert_eq!(ctx.memory(|memory| memory.focused()), Some(ids[2]));
        assert_eq!(values[0], 1);
    }
}
