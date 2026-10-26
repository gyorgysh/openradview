// SPDX-License-Identifier: GPL-3.0-or-later
//! Native rendering of the supplied open-viewport symbol and its finite intro.
use crate::theme::{GRAPHITE, PAPER, SILVER};
use eframe::egui::{self, Color32, Rect, Stroke, Vec2};

pub const WORDMARK: &[u8] = include_bytes!("../../../assets/brand/wordmark.png");

pub fn texture(ctx: &egui::Context, bytes: &[u8]) -> Option<egui::TextureHandle> {
    let rgba = image::load_from_memory(bytes).ok()?.into_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    Some(ctx.load_texture(
        "outlined-wordmark",
        egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()),
        egui::TextureOptions::LINEAR,
    ))
}

/// Animation follows the supplied 0.9 s frame / 0.8 s staggered slice timing.
/// No perpetual lift loop: idle rendering stops. reduce_motion draws final frame.
pub fn symbol(ui: &mut egui::Ui, side: f32, seconds: f32, reduce_motion: bool) {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(side), egui::Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::Button,
            ui.is_enabled(),
            "Replay OpenRadView logo",
        )
    });
    if response.clicked() {
        replay(ui.ctx());
    }
    response.on_hover_text(if reduce_motion {
        "OpenRadView · reduced motion"
    } else {
        "Replay OpenRadView logo"
    });
    let seconds = ui
        .ctx()
        .data(|d| d.get_temp::<std::time::Instant>(egui::Id::new("brand-replay")))
        .map_or(seconds, |started| started.elapsed().as_secs_f32());
    let painter = ui.painter_at(rect);
    let scale = side / 64.0;
    let point = |x: f32, y: f32| rect.min + egui::vec2(x, y) * scale;
    let progress = if reduce_motion {
        1.0
    } else {
        ((seconds - 0.05) / 0.9).clamp(0.0, 1.0)
    };
    let points = [
        point(42.0, 7.0),
        point(7.0, 7.0),
        point(7.0, 57.0),
        point(57.0, 57.0),
        point(57.0, 22.0),
    ];
    let mut remaining = 170.0 * scale * smooth(progress);
    let mut revealed = vec![points[0]];
    for segment in points.windows(2) {
        let length = segment[0].distance(segment[1]);
        if remaining > 0.0 {
            let end = segment[0].lerp(segment[1], (remaining / length).min(1.0));
            revealed.push(end);
        }
        remaining -= length;
    }
    if revealed.len() > 1 {
        painter.add(egui::Shape::line(revealed, Stroke::new(6.0 * scale, PAPER)));
    }
    // Background masks retain the two-unit separation of the original artwork.
    for (x, y, delay, color) in [
        (30.0, 14.0, 0.45, GRAPHITE),
        (22.0, 22.0, 0.62, SILVER),
        (14.0, 30.0, 0.79, PAPER),
    ] {
        let t = if reduce_motion {
            1.0
        } else {
            ((seconds - delay) / 0.8).clamp(0.0, 1.0)
        };
        let shift = 12.0 * (1.0 - (1.0 - t).powi(3));
        let offset = if reduce_motion { 0.0 } else { 12.0 - shift };
        let origin = point(x + offset, y - offset);
        let fill =
            Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), (t * 255.0) as u8);
        painter.rect_filled(
            Rect::from_min_size(origin - Vec2::splat(2.0 * scale), Vec2::splat(24.0 * scale)),
            0.0,
            crate::theme::INK,
        );
        painter.rect_filled(
            Rect::from_min_size(origin, Vec2::splat(20.0 * scale)),
            0.0,
            fill,
        );
    }
    if !reduce_motion && seconds < 2.1 {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(8));
    }
}

fn smooth(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

fn replay(ctx: &egui::Context) {
    ctx.data_mut(|d| d.insert_temp(egui::Id::new("brand-replay"), std::time::Instant::now()));
    ctx.request_repaint();
}

pub fn lockup(
    ui: &mut egui::Ui,
    texture: Option<&egui::TextureHandle>,
    seconds: f32,
    reduce_motion: bool,
) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        symbol(ui, 34.0, seconds, reduce_motion);
        if let Some(texture) = texture {
            let ratio = texture.size_vec2().x / texture.size_vec2().y;
            if ui
                .add(
                    egui::Image::new((texture.id(), egui::vec2(174.0, 174.0 / ratio)))
                        .sense(egui::Sense::click()),
                )
                .on_hover_text("Replay OpenRadView logo")
                .clicked()
            {
                replay(ui.ctx());
            }
        }
    });
}
