// SPDX-License-Identifier: GPL-3.0-or-later
//! Geometry-derived context only; no pixel scans, I/O or inferred medical units.
use crate::{annotations, theme};
use eframe::egui::{self, Align2, FontId, Rect};
use orv_core::{ViewState, inspection, study::Frame};

pub fn draw(ui: &egui::Ui, response: &egui::Response, image: &Frame, view: &ViewState) {
    let rect = response.rect;
    if rect.width() < 120. || rect.height() < 80. {
        return;
    }
    let painter = ui.painter_at(rect);
    let label = |position: egui::Pos2, align: Align2, text: &str| {
        let galley = painter.layout_no_wrap(text.into(), FontId::monospace(10.), theme::SMOKE);
        let bounds = align.anchor_size(position, galley.size());
        painter.rect_filled(bounds.expand(2.), 2., egui::Color32::from_black_alpha(180));
        painter.galley(bounds.min, galley, theme::SMOKE);
    };
    if let Some(directions) = image
        .geometry
        .as_ref()
        .and_then(inspection::edge_directions)
    {
        let edges = rect.shrink(8.);
        for ((position, align), text) in [
            (edges.left_center(), Align2::LEFT_CENTER),
            (edges.right_center(), Align2::RIGHT_CENTER),
            (edges.center_top(), Align2::CENTER_TOP),
            (edges.center_bottom(), Align2::CENTER_BOTTOM),
        ]
        .into_iter()
        .zip(view.transform.edge_order().map(|index| &directions[index]))
        {
            label(position, align, text);
        }
        if let Some((mm, width)) = scale_bar(rect, image, view) {
            let start = rect.left_bottom() + egui::vec2(12., -14.);
            let end = start + egui::vec2(width, 0.);
            let stroke = egui::Stroke::new(1., theme::SMOKE);
            painter.rect_filled(
                Rect::from_min_max(start - egui::vec2(4., 20.), end + egui::vec2(4., 4.)),
                2.,
                egui::Color32::from_black_alpha(180),
            );
            for (a, b) in [
                (start, end),
                (start - egui::vec2(0., 3.), start + egui::vec2(0., 3.)),
                (end - egui::vec2(0., 3.), end + egui::vec2(0., 3.)),
            ] {
                painter.line_segment([a, b], stroke);
            }
            label(
                start - egui::vec2(0., 5.),
                Align2::LEFT_BOTTOM,
                &format!("{mm} mm · image"),
            );
        }
    }
    if ui.is_enabled()
        && response.hovered()
        && let Some(pos) = response.hover_pos()
        && let Some(pixel) = annotations::pixel_at(rect, image, view, pos)
        && let Some(sample) = inspection::sample(image, pixel)
    {
        let value = match sample {
            inspection::Sample::Scalar(value) => {
                format!("Value {}", crate::image_info::number(f64::from(value)))
            }
            inspection::Sample::RgbCodes { values, bits } => {
                format!("RGB{bits} · {}, {}, {}", values[0], values[1], values[2])
            }
            inspection::Sample::SrgbDisplay(values) => {
                format!("sRGB8 · {}, {}, {}", values[0], values[1], values[2])
            }
        };
        let position = rect.right_bottom() + egui::vec2(-12., -12.);
        label(position, Align2::RIGHT_BOTTOM, &value);
        label(
            position - egui::vec2(0., 16.),
            Align2::RIGHT_BOTTOM,
            &format!(
                "Pixel {}, {}",
                pixel[0].round() as u32 + 1,
                pixel[1].round() as u32 + 1
            ),
        );
    }
}

fn scale_bar(rect: Rect, image: &Frame, view: &ViewState) -> Option<(f64, f32)> {
    let geometry = image.geometry.as_ref()?;
    geometry.validate().ok()?;
    let mapping = annotations::mapping(rect, image, view)?;
    let a = mapping.pixel_to_view([0., 0.])?;
    let (pixel_step, spacing) = if view.transform.swaps_axes() {
        ([0., 1.], geometry.spacing[0])
    } else {
        ([1., 0.], geometry.spacing[1])
    };
    let b = mapping.pixel_to_view(pixel_step)?;
    let points_per_mm = (b[0] - a[0]).abs() * f64::from(rect.width()) / spacing;
    let available = f64::from((rect.width() * 0.22).min(120.));
    let target = available / points_per_mm;
    if !target.is_finite() || target <= 0. {
        return None;
    }
    let decade = 10f64.powf(target.log10().floor());
    let mm = [1., 2., 5., 10.]
        .into_iter()
        .map(|factor| factor * decade)
        .rfind(|length| *length <= target)?;
    let points = (mm * points_per_mm) as f32;
    (points.is_finite() && points >= 8.).then_some((mm, points))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn image_scale_follows_anisotropic_spacing_fit_and_zoom_without_monitor_assumptions() {
        let mut image = (*orv_core::study::demo_study().series[0].frames[0]).clone();
        image.width = 200;
        image.height = 100;
        image.geometry.as_mut().unwrap().spacing = [3., 0.7];
        let rect = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400., 400.));
        let view = ViewState::default();
        let (mm, points) = scale_bar(rect, &image, &view).unwrap();
        assert!((f64::from(points) / mm - 400. / 300.).abs() < 1e-5);
        let zoom = ViewState {
            zoom: 2.,
            pan: [0.7, -0.3],
            ..view
        };
        let (mm, points) = scale_bar(rect, &image, &zoom).unwrap();
        assert!((f64::from(points) / mm - 800. / 300.).abs() < 1e-5);
        for quarter_turns in [1, 3] {
            for flip_horizontal in [false, true] {
                let rotated = ViewState {
                    transform: orv_core::DisplayTransform {
                        quarter_turns,
                        flip_horizontal,
                        flip_vertical: true,
                    },
                    ..Default::default()
                };
                let (mm, points) = scale_bar(rect, &image, &rotated).unwrap();
                assert!((f64::from(points) / mm - 400. / 300.).abs() < 1e-5);
            }
        }
        image.geometry = None;
        assert!(scale_bar(rect, &image, &zoom).is_none());
    }
}
