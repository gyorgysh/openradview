// SPDX-License-Identifier: GPL-3.0-or-later
//! Shared placement for auxiliary windows; `open` supplies egui's close button.
use eframe::egui;

pub fn window(title: &'static str, ctx: &egui::Context) -> egui::Window<'static> {
    egui::Window::new(title)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .max_width((ctx.content_rect().width() - 48.).max(280.))
        .max_height((ctx.content_rect().height() - 64.).max(200.))
        .collapsible(false)
        .vscroll(true)
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
}

#[cfg(feature = "mcp")]
pub fn heading(ui: &mut egui::Ui, title: &str) -> bool {
    ui.horizontal(|ui| {
        ui.heading(title);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.button("×").on_hover_text("Close dialog").clicked()
        })
        .inner
    })
    .inner
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn auxiliary_window_remains_centered_and_inside_resized_viewports() {
        let ctx = egui::Context::default();
        for size in [
            egui::vec2(1280., 840.),
            egui::vec2(860., 640.),
            egui::vec2(1280., 840.),
        ] {
            let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
            for pass in 0..3 {
                let mut bounds = None;
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(screen),
                        ..Default::default()
                    },
                    |ui| {
                        bounds = window("Centered help test", ui.ctx())
                            .open(&mut true)
                            .default_width(430.)
                            .show(ui.ctx(), |ui| {
                                ui.label("Help content");
                                ui.add_space(180.);
                            })
                            .map(|r| r.response.rect);
                    },
                );
                output.textures_delta.clear();
                let rect = bounds.unwrap();
                assert!(screen.contains_rect(rect), "{rect:?} outside {screen:?}");
                assert!(
                    pass == 0 || rect.center().distance(screen.center()) < 1.5,
                    "Window drifted: {rect:?}"
                );
            }
        }
    }
    #[test]
    fn overflowing_dialog_can_scroll_its_entire_last_line_into_view() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        for size in [egui::vec2(1280., 840.), egui::vec2(860., 640.)] {
            for pass in 0..4 {
                let mut output = ctx.run_ui(egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                    ..Default::default()
                }, |ui| {
                    window("Scrolling dialog test", ui.ctx()).open(&mut true)
                        .default_width(470.).default_height(600.)
                        .show(ui.ctx(), |ui| {
                            for _ in 0..30 { ui.label("Wrapped dialog content, including further help and privacy information."); }
                            ui.label("Last line must be readable in full.");
                            ui.add_space(12.);
                            ui.scroll_to_cursor_animation(Some(egui::Align::BOTTOM), egui::style::ScrollAnimation::none());
                        });
                });
                output.textures_delta.clear();
                if pass == 3 {
                    let shape = output
                        .shapes
                        .iter()
                        .find_map(|s| match &s.shape {
                            egui::epaint::Shape::Text(t)
                                if t.galley.job.text == "Last line must be readable in full." =>
                            {
                                Some((s.clip_rect, t.galley.rect.translate(t.pos.to_vec2())))
                            }
                            _ => None,
                        })
                        .unwrap();
                    assert!(
                        shape.0.contains_rect(shape.1),
                        "Last line clipped: {shape:?}"
                    );
                }
            }
        }
    }
}
