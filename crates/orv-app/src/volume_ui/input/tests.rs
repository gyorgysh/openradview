// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use egui::{Event, Modifiers, PointerButton, Pos2, RawInput, Rect, Vec2};
use orv_core::{study::demo_study, volume::Volume};

fn drag(button: PointerButton) -> (VolumeView, ViewState) {
    drag_with_space(button, false)
}
fn drag_with_space(button: PointerButton, space: bool) -> (VolumeView, ViewState) {
    gesture(button, space, false)
}
fn gesture(
    button: PointerButton,
    space: bool,
    release_with_motion: bool,
) -> (VolumeView, ViewState) {
    let ctx = egui::Context::default();
    let volume = Volume::new(demo_study().series[0].frames.clone()).unwrap();
    let mut state = VolumeView::fit(&volume);
    let mut display = ViewState::default();
    let mut input = Input::default();
    let start = Pos2::new(200., 200.);
    let mut motion = vec![Event::PointerMoved(Pos2::new(250., 225.))];
    if release_with_motion {
        motion.push(Event::PointerButton {
            pos: Pos2::new(250., 225.),
            button,
            pressed: false,
            modifiers: Modifiers::NONE,
        });
    }
    for (index, mut events) in [
        vec![],
        vec![
            Event::PointerMoved(start),
            Event::PointerButton {
                pos: start,
                button,
                pressed: true,
                modifiers: Modifiers::NONE,
            },
        ],
        motion,
    ]
    .into_iter()
    .enumerate()
    {
        if space && index == 1 {
            events.push(Event::Key {
                key: egui::Key::Space,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Modifiers::NONE,
            });
        }
        let mut output = ctx.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::splat(500.))),
                events,
                ..Default::default()
            },
            |ui| {
                let (_, response) =
                    ui.allocate_exact_size(Vec2::splat(400.), egui::Sense::click_and_drag());
                input.interact(ui, &response, &mut state, &mut display);
            },
        );
        output.textures_delta.clear();
    }
    (state, display)
}

#[test]
fn native_pointer_events_orbit_and_secondary_drag_only_pans() {
    let (orbit, orbit_display) = drag(PointerButton::Primary);
    assert!((orbit.camera.yaw_degrees - 20.).abs() < 1e-6);
    assert!((orbit.camera.pitch_degrees + 10.).abs() < 1e-6);
    assert_eq!(orbit_display.pan, [0.; 2]);
    let (pan, pan_display) = drag(PointerButton::Secondary);
    assert_eq!(pan.camera.yaw_degrees, 0.);
    assert_eq!(pan.camera.pitch_degrees, 0.);
    assert_eq!(pan_display.pan, [0.125, 0.0625]);
    orbit.validate().unwrap();
    pan.validate().unwrap();
    for (state, display) in [
        drag(PointerButton::Middle),
        drag_with_space(PointerButton::Primary, true),
    ] {
        assert_eq!(state.camera.yaw_degrees, 0.);
        assert_eq!(state.camera.pitch_degrees, 0.);
        assert_eq!(display.pan, [0.125, 0.0625]);
    }
}

#[test]
fn camera_motion_is_not_lost_when_movement_and_release_arrive_in_the_same_frame() {
    for (button, space) in [
        (PointerButton::Primary, false),
        (PointerButton::Secondary, false),
        (PointerButton::Middle, false),
        (PointerButton::Primary, true),
    ] {
        let (expected, expected_display) = gesture(button, space, false);
        let (actual, display) = gesture(button, space, true);
        assert_eq!(actual, expected, "{button:?}, space {space}");
        assert_eq!(display, expected_display, "{button:?}, space {space}");
    }
}

struct Harness {
    ctx: egui::Context,
    input: Input,
    state: VolumeView,
    display: ViewState,
    side: f32,
    enabled: bool,
    overlay: bool,
    multipass: bool,
}
impl Harness {
    fn new() -> Self {
        let volume = Volume::new(demo_study().series[0].frames.clone()).unwrap();
        let mut harness = Self {
            ctx: Default::default(),
            input: Default::default(),
            state: VolumeView::fit(&volume),
            display: Default::default(),
            side: 400.,
            enabled: true,
            overlay: false,
            multipass: false,
        };
        harness.frame(vec![]);
        harness
    }
    fn frame(&mut self, events: Vec<Event>) -> (bool, bool) {
        let mut result = (false, false);
        let mut output = self.ctx.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::splat(600.))),
                events,
                ..Default::default()
            },
            |ui| {
                if self.overlay {
                    egui::Window::new("Camera test settings")
                        .fixed_pos(egui::pos2(100., 100.))
                        .fixed_size(egui::vec2(250., 250.))
                        .show(ui.ctx(), |ui| {
                            ui.label("Controls cover the source viewport");
                        });
                }
                ui.add_enabled_ui(self.enabled, |ui| {
                    let (_, response) = ui
                        .allocate_exact_size(Vec2::splat(self.side), egui::Sense::click_and_drag());
                    result = self
                        .input
                        .interact(ui, &response, &mut self.state, &mut self.display);
                });
                if self.multipass && ui.ctx().current_pass_index() == 0 {
                    ui.ctx().request_discard("Camera test layout pass");
                }
            },
        );
        output.textures_delta.clear();
        result
    }
}
fn button(pos: Pos2, button: PointerButton, pressed: bool) -> Event {
    Event::PointerButton {
        pos,
        button,
        pressed,
        modifiers: Modifiers::NONE,
    }
}
fn key(key: egui::Key, pressed: bool) -> Event {
    Event::Key {
        key,
        physical_key: None,
        pressed,
        repeat: false,
        modifiers: Modifiers::NONE,
    }
}
#[test]
fn a_complete_batched_drag_and_space_release_match_held_input_and_do_not_repeat_when_idle() {
    for (button_id, space) in [
        (PointerButton::Primary, false),
        (PointerButton::Secondary, false),
        (PointerButton::Middle, false),
        (PointerButton::Primary, true),
    ] {
        let mut h = Harness::new();
        h.multipass = true;
        let start = egui::pos2(200., 200.);
        let end = egui::pos2(250., 225.);
        let mut events = vec![];
        if space {
            events.push(key(egui::Key::Space, true));
        }
        events.extend([
            Event::PointerMoved(start),
            button(start, button_id, true),
            Event::PointerMoved(end),
            button(end, button_id, false),
        ]);
        if space {
            events.push(key(egui::Key::Space, false));
        }
        assert_eq!(h.frame(events), (true, false));
        let (expected, display) = gesture(button_id, space, false);
        assert_eq!(h.state, expected);
        assert_eq!(h.display, display);
        assert!(h.input.drag.is_none());
        for _ in 0..100 {
            assert_eq!(h.frame(vec![]), (false, false));
        }
        assert_eq!(h.state, expected);
        assert_eq!(h.display, display);
    }
}
#[test]
fn camera_capture_stays_in_its_view_and_cancels_on_escape_loss_resize_or_disabled_controls() {
    for cause in 0..6 {
        let mut h = Harness::new();
        let start = egui::pos2(200., 200.);
        let end = egui::pos2(250., 225.);
        h.frame(vec![
            Event::PointerMoved(start),
            button(start, PointerButton::Primary, true),
        ]);
        assert!(h.input.drag.is_some());
        let events = match cause {
            0 => vec![key(egui::Key::Escape, true), Event::PointerMoved(end)],
            1 => vec![Event::PointerGone, Event::PointerMoved(end)],
            2 => {
                h.side = 350.;
                vec![Event::PointerMoved(end)]
            }
            3 => {
                h.enabled = false;
                vec![Event::PointerMoved(end)]
            }
            4 => vec![Event::WindowFocused(false), Event::PointerMoved(end)],
            _ => vec![
                button(start, PointerButton::Secondary, true),
                Event::PointerMoved(end),
            ],
        };
        h.frame(events);
        assert!(h.input.drag.is_none(), "capture retained for {cause}");
        assert_eq!(h.state.camera.yaw_degrees, 0.);
        assert_eq!(h.state.camera.pitch_degrees, 0.);
        assert_eq!(h.display.pan, [0.; 2]);
    }
    let mut h = Harness::new();
    let outside = egui::pos2(550., 550.);
    let inside = egui::pos2(200., 200.);
    h.frame(vec![
        Event::PointerMoved(outside),
        button(outside, PointerButton::Primary, true),
        Event::PointerMoved(inside),
        button(inside, PointerButton::Primary, false),
    ]);
    assert_eq!(h.state.camera.yaw_degrees, 0.);
    assert!(h.input.drag.is_none());
    h.overlay = true;
    h.frame(vec![]);
    h.frame(vec![]);
    h.frame(vec![
        Event::PointerMoved(inside),
        button(inside, PointerButton::Primary, true),
        Event::PointerMoved(egui::pos2(250., 225.)),
        button(inside, PointerButton::Primary, false),
    ]);
    assert!(h.input.drag.is_none());
    assert_eq!(h.state.camera.yaw_degrees, 0.);
}
#[test]
fn release_coordinates_finish_the_drag_and_stationary_clicks_keep_double_click_fit() {
    let mut h = Harness::new();
    let start = egui::pos2(200., 200.);
    let end = egui::pos2(250., 225.);
    h.frame(vec![
        Event::PointerMoved(start),
        button(start, PointerButton::Primary, true),
    ]);
    assert_eq!(
        h.frame(vec![button(end, PointerButton::Primary, false)]),
        (true, false)
    );
    assert_eq!(h.state.camera.yaw_degrees, 20.);
    assert_eq!(h.state.camera.pitch_degrees, -10.);
    let mut h = Harness::new();
    h.display.zoom = 2.;
    h.display.pan = [0.25, -0.15];
    for index in 0..2 {
        h.frame(vec![
            Event::PointerMoved(start),
            button(start, PointerButton::Primary, true),
        ]);
        assert_eq!(
            h.frame(vec![button(start, PointerButton::Primary, false)]),
            (false, index == 1)
        );
    }
    assert_eq!(h.display.zoom, 1.);
    assert_eq!(h.display.pan, [0.; 2]);
    assert_eq!(h.state.camera.yaw_degrees, 0.);
}
