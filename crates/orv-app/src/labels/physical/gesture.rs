// SPDX-License-Identifier: GPL-3.0-or-later
//! One bounded pointer path, bound to its completed plane and display transform.
use super::*;
use orv_core::{
    ViewState,
    labels::MAX_STROKE_POINTS,
    resources::{MemoryBudget, Reservation},
    study::Frame,
};

pub(in super::super) struct Path {
    points: Vec<[f64; 3]>,
    _memory: Reservation,
}
impl Path {
    fn new() -> Result<Self, &'static str> {
        let memory = MemoryBudget::volumes()
            .reserve((MAX_STROKE_POINTS * std::mem::size_of::<[f64; 3]>()) as u64)
            .ok_or("Not enough memory for the physical pointer path")?;
        let mut points = Vec::new();
        points
            .try_reserve_exact(MAX_STROKE_POINTS)
            .map_err(|_| "Not enough memory for the physical pointer path")?;
        Ok(Self {
            points,
            _memory: memory,
        })
    }
    pub fn points(&self) -> &[[f64; 3]] {
        &self.points
    }
    fn push(&mut self, point: [f64; 3]) -> Result<(), &'static str> {
        if self.points.last() == Some(&point) {
            return Ok(());
        }
        if self.points.len() == MAX_STROKE_POINTS {
            return Err("The physical pointer path is full; draw a shorter path");
        }
        self.points.push(point);
        Ok(())
    }
}
pub(in super::super) enum InputPath {
    Point([f64; 3]),
    Trace(Arc<Path>),
}
impl InputPath {
    pub fn points(&self) -> &[[f64; 3]] {
        match self {
            Self::Point(point) => std::slice::from_ref(point),
            Self::Trace(path) => path.points(),
        }
    }
}
impl From<[f64; 3]> for InputPath {
    fn from(point: [f64; 3]) -> Self {
        Self::Point(point)
    }
}
pub(super) struct Capture {
    image: Arc<Frame>,
    view: ViewState,
    rect: egui::Rect,
    plane: usize,
    epoch: u64,
    path: Path,
}
impl Capture {
    fn record(&mut self, position: egui::Pos2) -> Result<(), &'static str> {
        let point = world_at(self.rect, &self.image, &self.view, position)
            .ok_or("The pointer left the displayed plane; the path was discarded.")?;
        self.path.push(point)
    }
    fn events(&mut self, input: &egui::InputState, started: bool) -> Result<bool, &'static str> {
        let mut sampling = !started;
        for event in &input.events {
            match event {
                egui::Event::PointerButton {
                    button, modifiers, ..
                } if *button != egui::PointerButton::Primary
                    || *modifiers != egui::Modifiers::NONE =>
                {
                    return Err("Navigation input changed; the pointer path was discarded.");
                }
                egui::Event::Key {
                    key: egui::Key::Space,
                    ..
                } => {
                    return Err("Navigation input changed; the pointer path was discarded.");
                }
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    ..
                } => {
                    if *pressed && started {
                        sampling = true;
                    }
                    if sampling {
                        self.record(*pos)?;
                        if !pressed {
                            return Ok(true);
                        }
                    }
                }
                egui::Event::PointerMoved(position) if sampling => self.record(*position)?,
                egui::Event::PointerGone if sampling => {
                    return Err("Pointer input ended; the path was discarded.");
                }
                _ => (),
            }
        }
        if !input.pointer.primary_down() {
            return Err("Pointer input ended; the path was discarded.");
        }
        Ok(false)
    }
    fn matches(
        &self,
        image: &Arc<Frame>,
        view: &ViewState,
        rect: egui::Rect,
        plane: usize,
        epoch: u64,
    ) -> bool {
        Arc::ptr_eq(&self.image, image)
            && self.view == *view
            && self.rect == rect
            && self.plane == plane
            && self.epoch == epoch
    }
}

fn point_at(response: &egui::Response, image: &Frame, view: &ViewState) -> Option<[f64; 3]> {
    let position = response.interact_pointer_pos()?;
    world_at(response.rect, image, view, position)
}
fn world_at(
    rect: egui::Rect,
    image: &Frame,
    view: &ViewState,
    position: egui::Pos2,
) -> Option<[f64; 3]> {
    if !position.is_finite() || !rect.contains(position) {
        return None;
    }
    let pixel = crate::annotations::pixel_at(rect, image, view, position)?;
    if pixel[0] < -0.5
        || pixel[1] < -0.5
        || pixel[0] > f64::from(image.width) - 0.5
        || pixel[1] > f64::from(image.height) - 0.5
    {
        return None;
    }
    image.geometry.as_ref()?.pixel_to_world(pixel).ok()
}
impl LabelUi {
    pub fn physical_drawing(&self) -> bool {
        self.physical.drawing
    }
    pub fn physical_drawing_plane(&self) -> Option<usize> {
        self.physical.gesture.as_ref().map(|gesture| gesture.plane)
    }
    pub fn take_physical_draw_request(&mut self) -> bool {
        std::mem::take(&mut self.physical.draw_requested)
    }
    pub fn stop_physical_drawing(&mut self) {
        if self.physical.drawing {
            self.physical.drawing = false;
            self.invalidate_physical();
        }
    }
    pub fn physical_pointer(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        image: &Arc<Frame>,
        view: &ViewState,
        plane: usize,
    ) {
        if !self.physical.drawing || self.physical.open {
            return;
        }
        if !ui.is_enabled()
            || self.open
            || self.busy()
            || self.physical.input.is_none()
            || image.slab.is_some()
            || ui.input(|input| !input.focused || input.modifiers != egui::Modifiers::NONE)
            || crate::viewport::navigation_override(ui, response)
            || response.double_clicked()
        {
            self.physical.gesture = None;
            return;
        }
        if self
            .physical
            .gesture
            .as_ref()
            .is_some_and(|capture| capture.plane != plane)
        {
            return;
        }
        if self.physical.gesture.as_ref().is_some_and(|capture| {
            !capture.matches(image, view, response.rect, plane, self.physical.epoch)
        }) {
            self.physical.gesture = None;
            self.message = "The displayed plane changed; the pointer path was discarded.".into();
            self.physical.open = true;
            return;
        }
        let started = response.hovered() && ui.input(|input| input.pointer.primary_pressed());
        if started {
            self.invalidate_physical();
            match Path::new() {
                Ok(path) => {
                    self.physical.gesture = Some(Capture {
                        image: image.clone(),
                        view: view.clone(),
                        rect: response.rect,
                        plane,
                        epoch: self.physical.epoch,
                        path,
                    })
                }
                Err(error) => {
                    self.message = error.into();
                    self.physical.open = true;
                    return;
                }
            }
        }
        if let Some(capture) = &mut self.physical.gesture {
            let released = match ui.input(|input| capture.events(input, started)) {
                Ok(released) => released,
                Err(error) => {
                    self.physical.gesture = None;
                    self.message = error.into();
                    self.physical.open = true;
                    return;
                }
            };
            if released && let Some(capture) = self.physical.gesture.take() {
                self.physical.path = Some(Arc::new(capture.path));
                self.submit_physical(ui.ctx());
                self.physical.open = true;
            }
        }
    }
    pub fn physical_pointer_guide(
        &self,
        ui: &egui::Ui,
        response: &egui::Response,
        image: &Frame,
        view: &ViewState,
    ) {
        if !self.physical.drawing || self.physical.open {
            return;
        }
        let painter = ui.painter_at(response.rect);
        if response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
            if let Some(point) = point_at(response, image, view)
                && let Some(geometry) = &image.geometry
            {
                let project = |world| {
                    crate::annotations::screen_at(
                        response.rect,
                        image,
                        view,
                        geometry.world_to_pixel(world).ok()?,
                    )
                };
                if let Some(center) = project(point) {
                    let ends = [0, 1].map(|axis| {
                        let world = std::array::from_fn(|i| {
                            point[i] + geometry.orientation[axis * 3 + i] * self.physical.radius
                        });
                        project(world).map(|end| end - center)
                    });
                    if let [Some(a), Some(b)] = ends {
                        painter.add(egui::Shape::ellipse_stroke(
                            center,
                            egui::vec2(a.x.abs() + b.x.abs(), a.y.abs() + b.y.abs()),
                            egui::Stroke::new(1., crate::theme::PAPER),
                        ));
                    }
                }
            }
        }
        painter.text(
            response.rect.left_top() + egui::vec2(5., 20.),
            egui::Align2::LEFT_TOP,
            "PHYSICAL PATH · ESC STOPS",
            egui::FontId::monospace(9.),
            crate::theme::PAPER,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn setup() -> (LabelUi, egui::Context, Arc<Frame>, ViewState) {
        let mut labels = super::super::tests::state();
        let source = labels.current.as_ref().unwrap().source_arc();
        labels.sync_physical(&super::super::tests::context(&source));
        labels.physical.open = false;
        labels.physical.drawing = true;
        (
            labels,
            egui::Context::default(),
            source.frames()[1].clone(),
            ViewState::default(),
        )
    }
    fn draw(
        labels: &mut LabelUi,
        ctx: &egui::Context,
        image: &Arc<Frame>,
        view: &ViewState,
        events: Vec<egui::Event>,
    ) -> egui::Rect {
        let mut rect = egui::Rect::NOTHING;
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(600., 600.),
                )),
                events,
                ..Default::default()
            },
            |ui| {
                let (_, response) =
                    ui.allocate_exact_size(egui::vec2(400., 400.), egui::Sense::click_and_drag());
                rect = response.rect;
                labels.physical_pointer(ui, &response, image, view, 0);
            },
        );
        output.textures_delta.clear();
        rect
    }
    fn button(position: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos: position,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        }
    }
    fn wait(labels: &mut LabelUi) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while labels.busy() {
            labels.poll();
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
    }

    #[test]
    fn transformed_raw_path_keeps_turns_in_one_batch_and_releases_one_private_continuous_edit() {
        for turns in 0..4 {
            let (mut labels, ctx, image, mut view) = setup();
            view.transform.quarter_turns = turns;
            view.transform.flip_horizontal = true;
            let base = labels.current.as_ref().unwrap().clone();
            let rect = draw(&mut labels, &ctx, &image, &view, vec![]);
            let points = [[1., 1.], [5., 1.], [5., 5.]];
            let screen = points
                .map(|point| crate::annotations::screen_at(rect, &image, &view, point).unwrap());
            draw(
                &mut labels,
                &ctx,
                &image,
                &view,
                vec![egui::Event::PointerMoved(screen[0])],
            );
            draw(
                &mut labels,
                &ctx,
                &image,
                &view,
                vec![button(screen[0], true)],
            );
            assert!(!labels.busy() && labels.physical.gesture.is_some());
            draw(
                &mut labels,
                &ctx,
                &image,
                &view,
                vec![
                    egui::Event::PointerMoved(screen[1]),
                    egui::Event::PointerMoved(screen[2]),
                    button(screen[2], false),
                ],
            );
            assert!(labels.busy() && labels.physical.open);
            wait(&mut labels);
            let preview = labels.physical.preview.as_ref().unwrap();
            assert_eq!(preview.stroke.points_mm().len(), 3);
            for (actual, expected) in
                preview
                    .stroke
                    .points_mm()
                    .iter()
                    .zip([[1., 1., 1.], [5., 1., 1.], [5., 5., 1.]])
            {
                assert!(
                    actual
                        .iter()
                        .zip(expected)
                        .all(|(a, b)| (a - b).abs() < 0.00001)
                );
            }
            assert_eq!(preview.stroke.value([3, 1, 1]), Some(1));
            assert_eq!(preview.stroke.value([5, 3, 1]), Some(1));
            assert_eq!(preview.stroke.value([3, 3, 1]), Some(0));
            assert!(Arc::ptr_eq(labels.current.as_ref().unwrap(), &base));
            labels.apply_physical();
            assert_eq!(labels.undo.len(), 1);
        }
    }
    #[test]
    fn changed_frame_transform_epoch_or_escaped_pointer_discards_the_whole_path() {
        for change in 0..5 {
            let (mut labels, ctx, mut image, mut view) = setup();
            let rect = draw(&mut labels, &ctx, &image, &view, vec![]);
            let position = rect.center();
            draw(
                &mut labels,
                &ctx,
                &image,
                &view,
                vec![egui::Event::PointerMoved(position)],
            );
            draw(
                &mut labels,
                &ctx,
                &image,
                &view,
                vec![button(position, true)],
            );
            match change {
                0 => image = Arc::new((*image).clone()),
                1 => view.pan[0] = 0.1,
                2 => labels.physical.epoch += 1,
                3 => labels.stop_physical_drawing(),
                _ => (),
            }
            let end = if change == 4 {
                rect.max + egui::vec2(30., 30.)
            } else {
                position
            };
            draw(
                &mut labels,
                &ctx,
                &image,
                &view,
                vec![egui::Event::PointerMoved(end), button(end, false)],
            );
            assert!(labels.physical.gesture.is_none());
            assert!(!labels.busy() && !labels.physical_preview_visible());
            assert!(!labels.dirty && labels.undo.is_empty());
        }
    }
    #[test]
    fn pointer_capacity_refuses_the_entire_trace_and_deduplicates_stationary_events() {
        let mut path = Path::new().unwrap();
        for index in 0..MAX_STROKE_POINTS {
            let point = [index as f64, 0., 0.];
            path.push(point).unwrap();
            path.push(point).unwrap();
        }
        assert_eq!(path.points().len(), MAX_STROKE_POINTS);
        assert!(path.push([-1., 0., 0.]).is_err());
    }

    #[test]
    fn transient_navigation_inside_one_input_batch_cannot_publish_a_partial_trace() {
        for secondary in [false, true] {
            let (mut labels, ctx, image, view) = setup();
            let rect = draw(&mut labels, &ctx, &image, &view, vec![]);
            let point = rect.center();
            draw(
                &mut labels,
                &ctx,
                &image,
                &view,
                vec![egui::Event::PointerMoved(point)],
            );
            draw(&mut labels, &ctx, &image, &view, vec![button(point, true)]);
            let event = if secondary {
                egui::Event::PointerButton {
                    pos: point,
                    button: egui::PointerButton::Secondary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                }
            } else {
                egui::Event::Key {
                    key: egui::Key::Space,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }
            };
            draw(
                &mut labels,
                &ctx,
                &image,
                &view,
                vec![event, button(point, false)],
            );
            assert!(labels.physical.gesture.is_none());
            assert!(!labels.busy() && !labels.physical_preview_visible() && !labels.dirty);
        }
    }
}
