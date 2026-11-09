// SPDX-License-Identifier: GPL-3.0-or-later
use eframe::egui;
use orv_core::{ViewState, volume_view::VolumeView};

#[derive(Default)]
pub(super) struct Input {
    drag: Option<Drag>,
    completed_frame: Option<(u64, bool, bool)>,
}
struct Drag {
    id: egui::Id,
    layer: egui::LayerId,
    rect: egui::Rect,
    clip: egui::Rect,
    button: egui::PointerButton,
    origin: egui::Pos2,
    last: egui::Pos2,
    decided: bool,
}
#[derive(Clone, Copy)]
enum Event {
    Move(egui::Pos2),
    Button(egui::Pos2, egui::PointerButton, bool),
    Space(bool),
    Cancel,
}
impl Event {
    fn read(event: &egui::Event) -> Option<Self> {
        match *event {
            egui::Event::PointerMoved(pos) => Some(Self::Move(pos)),
            egui::Event::PointerButton {
                pos,
                button,
                pressed,
                ..
            } => Some(Self::Button(pos, button, pressed)),
            egui::Event::Key {
                key: egui::Key::Space,
                pressed,
                repeat: false,
                ..
            } => Some(Self::Space(pressed)),
            egui::Event::PointerGone | egui::Event::WindowFocused(false) => Some(Self::Cancel),
            _ => None,
        }
    }
}
impl Input {
    /// Consume ordered pointer events rather than only the final held-button
    /// state. Native integrations can deliver movement and release together.
    pub fn interact(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        state: &mut VolumeView,
        display: &mut ViewState,
    ) -> (bool, bool) {
        let frame = ui.ctx().cumulative_frame_nr();
        if let Some((done, active, fit)) = self.completed_frame
            && done == frame
        {
            return (active, fit);
        }
        let result = self.frame(ui, response, state, display);
        self.completed_frame = Some((frame, result.0, result.1));
        result
    }
    fn frame(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        state: &mut VolumeView,
        display: &mut ViewState,
    ) -> (bool, bool) {
        let rect = response.rect;
        let enabled = ui.is_enabled()
            && response.enabled()
            && !egui::Popup::is_any_open(ui.ctx())
            && ui
                .ctx()
                .memory(|memory| memory.allows_interaction(response.layer_id))
            && ui.input(|input| input.focused && !input.key_pressed(egui::Key::Escape));
        if !enabled {
            self.drag = None;
            return (false, false);
        }
        if self.drag.as_ref().is_some_and(|drag| {
            drag.id != response.id
                || drag.layer != response.layer_id
                || drag.rect != rect
                || drag.clip != ui.clip_rect()
                || ui.ctx().layer_id_at(drag.origin) != Some(drag.layer)
        }) {
            self.drag = None;
        }
        let mut active = false;
        let owner = crate::viewport::zoom_scroll_owner(ui, response, true);
        if response.hovered() && owner.is_none_or(|owner| owner == response.id) {
            ui.input(|input| {
                let zoom = input.zoom_delta();
                let factor = if (zoom - 1.).abs() > f32::EPSILON {
                    zoom
                } else {
                    (input.smooth_scroll_delta.y * 0.004).exp()
                };
                display.zoom = (display.zoom * factor).clamp(0.05, 50.);
                active |= input.smooth_scroll_delta.y.abs() > 0.01 || (zoom - 1.).abs() > 0.001;
            });
        }
        let threshold = ui
            .ctx()
            .options(|options| options.input_options.max_click_dist);
        let (count, mut space) = ui.input(|input| {
            // InputState describes the final keyboard state; rewind the first
            // transition so a batched Space release cannot turn a pan into orbit.
            let initial = input
                .events
                .iter()
                .find_map(|event| match Event::read(event) {
                    Some(Event::Space(pressed)) => Some(!pressed),
                    _ => None,
                })
                .unwrap_or_else(|| input.key_down(egui::Key::Space));
            (input.events.len(), initial)
        });
        for index in 0..count {
            // Copy only a small pointer/key event. Context hit testing happens
            // outside the input lock, without cloning pasted text or event lists.
            let event = ui.input(|input| input.events.get(index).and_then(Event::read));
            match event {
                Some(Event::Button(pos, button, true)) => {
                    if self.drag.is_some() {
                        self.drag = None;
                    } else if pos.is_finite()
                        && matches!(
                            button,
                            egui::PointerButton::Primary
                                | egui::PointerButton::Secondary
                                | egui::PointerButton::Middle
                        )
                        && rect.contains(pos)
                        && ui.clip_rect().contains(pos)
                        && ui.ctx().layer_id_at(pos) == Some(response.layer_id)
                    {
                        response.request_focus();
                        self.drag = Some(Drag {
                            id: response.id,
                            layer: response.layer_id,
                            rect,
                            clip: ui.clip_rect(),
                            button,
                            origin: pos,
                            last: pos,
                            decided: false,
                        });
                    }
                }
                Some(Event::Move(pos)) => {
                    active |= self.move_to(pos, space, threshold, state, display);
                }
                Some(Event::Button(pos, button, false)) => {
                    if self.drag.as_ref().is_some_and(|drag| drag.button == button) {
                        active |= self.move_to(pos, space, threshold, state, display);
                        self.drag = None;
                    }
                }
                Some(Event::Space(pressed)) => space = pressed,
                Some(Event::Cancel) => self.drag = None,
                None => (),
            }
        }
        if self
            .drag
            .as_ref()
            .is_some_and(|drag| !ui.input(|input| input.pointer.button_down(drag.button)))
        {
            self.drag = None;
        }
        active |= self.drag.as_ref().is_some_and(|drag| drag.decided);
        let fit = !active && response.double_clicked();
        if fit {
            display.zoom = 1.;
            display.pan = [0.; 2];
        }
        (active, fit)
    }
    fn move_to(
        &mut self,
        pos: egui::Pos2,
        space: bool,
        threshold: f32,
        state: &mut VolumeView,
        display: &mut ViewState,
    ) -> bool {
        if !pos.is_finite() {
            self.drag = None;
            return false;
        }
        let Some(drag) = &mut self.drag else {
            return false;
        };
        if !drag.decided {
            drag.decided = pos.distance(drag.origin) > threshold || !drag.rect.contains(pos);
            if !drag.decided {
                return false;
            }
        }
        let delta = pos - drag.last;
        if !delta.is_finite() {
            self.drag = None;
            return false;
        }
        drag.last = pos;
        if space
            || matches!(
                drag.button,
                egui::PointerButton::Secondary | egui::PointerButton::Middle
            )
        {
            display.pan[0] =
                (display.pan[0] + delta.x / drag.rect.width().max(1.)).clamp(-10., 10.);
            display.pan[1] =
                (display.pan[1] + delta.y / drag.rect.height().max(1.)).clamp(-10., 10.);
        } else {
            state.camera.yaw_degrees =
                ((state.camera.yaw_degrees + f64::from(delta.x) * 0.4 + 180.).rem_euclid(360.))
                    - 180.;
            state.camera.pitch_degrees =
                (state.camera.pitch_degrees - f64::from(delta.y) * 0.4).clamp(-90., 90.);
        }
        true
    }
}

#[cfg(test)]
mod tests;
