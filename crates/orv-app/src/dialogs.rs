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
}
