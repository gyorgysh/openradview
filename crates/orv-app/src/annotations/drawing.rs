// SPDX-License-Identifier: GPL-3.0-or-later
//! Shared native/illustration geometry; control handles are an explicit paint choice.
use super::*;
fn ellipse_steps(radius: f32) -> usize {
    ((std::f32::consts::PI * (radius.max(0.) / 0.5).sqrt()).ceil() as usize)
        .clamp(32, 2048)
        .div_ceil(4)
        * 4
}
pub(super) fn segments(
    rect: Rect,
    image: &Frame,
    view: &ViewState,
    points: &[[f64; 2]],
    tool: Tool,
) -> Option<usize> {
    if tool == Tool::EllipseRoi && points.len() == 2 {
        let bounds = Rect::from_two_pos(
            screen_at(rect, image, view, points[0])?,
            screen_at(rect, image, view, points[1])?,
        );
        Some(ellipse_steps(bounds.size().max_elem() / 2.))
    } else {
        Some(if tool == Tool::Roi { 4 } else { points.len() })
    }
}
pub(super) fn geometry(
    painter: &egui::Painter,
    rect: Rect,
    image: &Frame,
    view: &ViewState,
    points: &[[f64; 2]],
    tool: Tool,
    handles: bool,
) {
    let screen = points
        .iter()
        .filter_map(|p| screen_at(rect, image, view, *p))
        .collect::<Vec<_>>();
    // The pinned ellipse tessellator scales its contour length linearly with
    // radius. Bound native and export work while adapting to projected curvature.
    let ellipse = if tool == Tool::EllipseRoi && screen.len() > 1 {
        let bounds = Rect::from_two_pos(screen[0], screen[1]);
        let radius = bounds.size() / 2.;
        let count = ellipse_steps(radius.max_elem());
        (radius.min_elem() > 0.).then(|| {
            (0..count)
                .map(|i| {
                    let angle = std::f32::consts::TAU * i as f32 / count as f32;
                    bounds.center() + egui::vec2(radius.x * angle.cos(), radius.y * angle.sin())
                })
                .collect::<Vec<_>>()
        })
    } else {
        None
    };
    // The dark surround keeps geometry visible over bright source pixels.
    for stroke in [
        egui::Stroke::new(3.5, theme::INK),
        egui::Stroke::new(1.5, theme::PAPER),
    ] {
        if matches!(tool, Tool::Roi | Tool::EllipseRoi) && screen.len() > 1 {
            let bounds = Rect::from_two_pos(screen[0], screen[1]);
            if tool == Tool::EllipseRoi {
                if let Some(points) = &ellipse {
                    painter.add(egui::epaint::PathShape::closed_line(
                        points.clone(),
                        egui::epaint::PathStroke::from(stroke).outside(),
                    ));
                }
            } else {
                painter.rect_stroke(bounds, 0., stroke, egui::StrokeKind::Middle);
            }
        } else {
            for pair in screen.windows(2) {
                painter.line_segment([pair[0], pair[1]], stroke);
            }
            if tool == Tool::PolygonRoi && screen.len() > 2 {
                painter.line_segment([screen[screen.len() - 1], screen[0]], stroke);
            }
        }
    }
    if handles {
        for pos in screen {
            painter.circle_filled(pos, 3.0, theme::PAPER);
            painter.circle_stroke(pos, 4.5, egui::Stroke::new(1.0, theme::INK));
        }
    }
}

pub(super) fn number(
    painter: &egui::Painter,
    rect: Rect,
    image: &Frame,
    view: &ViewState,
    point: [f64; 2],
    number: u64,
    selected: bool,
) {
    if let Some(position) = screen_at(rect, image, view, point) {
        let galley = painter.layout_no_wrap(
            format!("#{number}{}", if selected { " •" } else { "" }),
            egui::FontId::monospace(11.),
            theme::PAPER,
        );
        let bounds = Rect::from_min_size(position + egui::vec2(6., 6.), galley.size());
        painter.rect_filled(bounds.expand(1.), 0., theme::INK);
        painter.galley(bounds.min, galley, theme::PAPER);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ellipse_contours_are_bounded_and_keep_small_projected_chord_error() {
        for radius in [1., 10., 100., 500., 25_600., 409_600.] {
            let count = ellipse_steps(radius);
            assert!((32..=2048).contains(&count) && count.is_multiple_of(4));
            let error = f64::from(radius) * (1. - (std::f64::consts::PI / count as f64).cos());
            assert!(
                error <= 0.5,
                "Projected chord error {error} at radius {radius}"
            );
        }
        assert_eq!(ellipse_steps(f32::MAX), 2048);
    }
}
