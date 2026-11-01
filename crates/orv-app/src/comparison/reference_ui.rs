// SPDX-License-Identifier: GPL-3.0-or-later
//! Fill reserved canvas slots only after both completed pane bindings still match.
use crate::{annotations, theme};
use eframe::egui;
use orv_core::{
    ViewState,
    reference_line::{self, Intersection},
    spatial::GeometryError,
    study::Frame,
};
use std::sync::Arc;

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub(super) enum Status {
    #[default]
    Off,
    Pending,
    Shown,
    OutsideView,
    Parallel,
    Coplanar,
    Outside,
    Reference,
    Geometry,
    Projection,
    Display,
}
impl Status {
    pub fn text(self) -> &'static str {
        match self {
            Self::Off => "Reference lines are off.",
            Self::Pending => {
                "Reference lines wait for both completed images and their current display settings."
            }
            Self::Shown => {
                "Dashed lines show the other pane's declared plane within both image footprints. Check alignment yourself; no registration is performed."
            }
            Self::OutsideView => "The declared intersection is outside the current displayed area.",
            Self::Parallel => "Parallel or nearly parallel planes have no unique reference line.",
            Self::Coplanar => {
                "The planes coincide within the supported precision; there is no unique reference line."
            }
            Self::Outside => "The two finite image footprints have no common line segment.",
            Self::Reference => {
                "Reference lines need matching known spatial references. No alignment is guessed."
            }
            Self::Geometry => "Reference lines need valid image position, directions and spacing.",
            Self::Projection => {
                "Reference lines need two thin image planes, without slab projection."
            }
            Self::Display => {
                "A reference line could not be represented within this display's limits."
            }
        }
    }
}
pub(super) struct Target {
    image: Arc<Frame>,
    view: ViewState,
    painter: egui::Painter,
    rect: egui::Rect,
    slot: egui::layers::ShapeIdx,
}
pub(super) type Current<'a> = Option<(usize, &'a Arc<Frame>, &'a ViewState)>;
impl Target {
    pub fn reserve(ui: &egui::Ui, rect: egui::Rect, image: Arc<Frame>, view: ViewState) -> Self {
        let painter = ui.painter_at(rect.shrink(2.));
        let slot = painter.add(egui::Shape::Noop);
        Self {
            image,
            view,
            painter,
            rect,
            slot,
        }
    }
    fn matches(&self, current: Current<'_>) -> bool {
        current.is_some_and(|(index, image, view)| {
            index == view.slice as usize && Arc::ptr_eq(image, &self.image) && view == &self.view
        })
    }
}
pub(super) fn paint_pair(targets: [Option<&Target>; 2], current: [Current<'_>; 2]) -> Status {
    let [Some(left), Some(right)] = targets else {
        return Status::Pending;
    };
    if !left.matches(current[0]) || !right.matches(current[1]) {
        return Status::Pending;
    }
    if left.image.slab.is_some() || right.image.slab.is_some() {
        return Status::Projection;
    }
    let results = [
        reference_line::intersection(&left.image, &right.image),
        reference_line::intersection(&right.image, &left.image),
    ];
    for result in results {
        if let Err(error) = result {
            return if error == GeometryError::Reference {
                Status::Reference
            } else {
                Status::Geometry
            };
        }
    }
    match results {
        [Ok(Intersection::Segment(a)), Ok(Intersection::Segment(b))] => {
            let (Ok(a), Ok(b)) = (shapes(left, a), shapes(right, b)) else {
                return Status::Display;
            };
            let visible = a.is_some() || b.is_some();
            if let Some(shape) = a {
                left.painter.set(left.slot, shape);
            }
            if let Some(shape) = b {
                right.painter.set(right.slot, shape);
            }
            if visible {
                Status::Shown
            } else {
                Status::OutsideView
            }
        }
        [Ok(Intersection::Parallel), Ok(Intersection::Parallel)] => Status::Parallel,
        [Ok(Intersection::Coplanar), Ok(Intersection::Coplanar)] => Status::Coplanar,
        _ => Status::Outside,
    }
}
fn shapes(target: &Target, points: [[f64; 2]; 2]) -> Result<Option<egui::Shape>, ()> {
    let rect = target.painter.clip_rect();
    let original = target.rect;
    let a = annotations::screen_at(original, &target.image, &target.view, points[0]).ok_or(())?;
    let b = annotations::screen_at(original, &target.image, &target.view, points[1]).ok_or(())?;
    if !a.is_finite() || !b.is_finite() {
        return Err(());
    }
    let Some([a, b]) = clip([a, b], rect) else {
        return Ok(None);
    };
    let delta = b - a;
    let length = delta.length();
    if !length.is_finite() || length <= f32::EPSILON {
        return Err(());
    }
    // Clip before tessellation: extreme zoom/pan cannot create an unbounded dash list.
    let count = (length / 12.).ceil().clamp(1., 128.) as usize;
    let mut shapes = Vec::new();
    shapes.try_reserve_exact(count * 2).map_err(|_| ())?;
    let intervals = (count * 2 - 1) as f32;
    for index in 0..count {
        let points = [
            a + delta * (index * 2) as f32 / intervals,
            a + delta * (index * 2 + 1) as f32 / intervals,
        ];
        for stroke in [
            egui::Stroke::new(3., theme::INK),
            egui::Stroke::new(1., theme::PAPER),
        ] {
            shapes.push(egui::Shape::line_segment(points, stroke));
        }
    }
    Ok(Some(egui::Shape::Vec(shapes)))
}
fn clip(points: [egui::Pos2; 2], rect: egui::Rect) -> Option<[egui::Pos2; 2]> {
    let origin = [f64::from(points[0].x), f64::from(points[0].y)];
    let direction = [
        f64::from(points[1].x) - origin[0],
        f64::from(points[1].y) - origin[1],
    ];
    let mut range = [0.0f64, 1.];
    for axis in 0..2 {
        let bounds = if axis == 0 {
            [rect.min.x, rect.max.x]
        } else {
            [rect.min.y, rect.max.y]
        };
        if direction[axis] == 0. {
            if origin[axis] < f64::from(bounds[0]) || origin[axis] > f64::from(bounds[1]) {
                return None;
            }
        } else {
            let mut a = (f64::from(bounds[0]) - origin[axis]) / direction[axis];
            let mut b = (f64::from(bounds[1]) - origin[axis]) / direction[axis];
            if a > b {
                std::mem::swap(&mut a, &mut b);
            }
            range[0] = range[0].max(a);
            range[1] = range[1].min(b);
            if range[0] >= range[1] {
                return None;
            }
        }
    }
    Some(range.map(|t| {
        egui::pos2(
            (origin[0] + direction[0] * t) as f32,
            (origin[1] + direction[1] * t) as f32,
        )
    }))
}
pub(super) fn status(ui: &mut egui::Ui, status: Status) {
    let height = ui.text_style_height(&egui::TextStyle::Small) * 2. + 4.;
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        egui::Sense::hover(),
    );
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .id_salt("reference-status")
            .max_rect(rect),
    );
    child.set_clip_rect(ui.clip_rect().intersect(rect));
    let response =
        child.add(egui::Label::new(egui::RichText::new(status.text()).small().weak()).wrap());
    ui.ctx().accesskit_node_builder(response.id, |node| {
        node.set_live(egui::accesskit::Live::Polite)
    });
    response.on_hover_text(status.text());
}

#[cfg(test)]
mod tests;
