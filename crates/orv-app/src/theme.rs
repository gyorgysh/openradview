// SPDX-License-Identifier: GPL-3.0-or-later
use eframe::egui::{self, Color32, FontData, FontDefinitions, FontFamily, FontId, TextStyle};

pub const INK: Color32 = Color32::from_gray(10);
pub const GRAPHITE: Color32 = Color32::from_gray(107);
pub const SILVER: Color32 = Color32::from_gray(179);
pub const SMOKE: Color32 = Color32::from_gray(242);
pub const PAPER: Color32 = Color32::WHITE;

pub fn install(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::empty();
    fonts.font_data.insert(
        "space".into(),
        FontData::from_static(include_bytes!("../../../assets/fonts/SpaceGrotesk.ttf")).into(),
    );
    fonts.font_data.insert(
        "plex".into(),
        FontData::from_static(include_bytes!(
            "../../../assets/fonts/IBMPlexMono-Regular.ttf"
        ))
        .into(),
    );
    fonts
        .families
        .insert(FontFamily::Proportional, vec!["space".into()]);
    fonts
        .families
        .insert(FontFamily::Monospace, vec!["plex".into()]);
    ctx.set_fonts(fonts);
    let mut style = egui::Style {
        visuals: egui::Visuals::dark(),
        ..Default::default()
    };
    style.visuals.panel_fill = INK;
    style.visuals.window_fill = INK;
    style.visuals.extreme_bg_color = INK;
    style.visuals.faint_bg_color = Color32::from_gray(19);
    style.visuals.override_text_color = Some(SMOKE);
    style.visuals.selection.bg_fill = GRAPHITE;
    style.visuals.selection.stroke = egui::Stroke::new(1.0, PAPER);
    style.visuals.hyperlink_color = SILVER;
    style.visuals.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, Color32::from_gray(38));
    style.visuals.widgets.inactive.weak_bg_fill = Color32::from_gray(25);
    style.visuals.widgets.hovered.weak_bg_fill = Color32::from_gray(45);
    style.visuals.widgets.active.weak_bg_fill = GRAPHITE;
    style.spacing.item_spacing = egui::vec2(12.0, 12.0);
    style.spacing.button_padding = egui::vec2(18.0, 10.0);
    style
        .text_styles
        .insert(TextStyle::Heading, FontId::proportional(30.0));
    style
        .text_styles
        .insert(TextStyle::Body, FontId::proportional(15.0));
    style
        .text_styles
        .insert(TextStyle::Button, FontId::proportional(15.0));
    style
        .text_styles
        .insert(TextStyle::Small, FontId::proportional(12.0));
    style
        .text_styles
        .insert(TextStyle::Monospace, FontId::monospace(11.0));
    ctx.set_theme(egui::Theme::Dark);
    ctx.set_style_of(egui::Theme::Dark, style);
}

pub fn eyebrow(ui: &mut egui::Ui, text: &str) {
    ui.label(
        egui::RichText::new(text)
            .monospace()
            .size(10.0)
            .color(SILVER),
    );
}
