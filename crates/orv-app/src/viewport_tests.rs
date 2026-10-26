// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use egui::{Event, Modifiers, PointerButton, Pos2, RawInput, Rect, Vec2};

fn input_frame(ctx: &egui::Context, view: &mut ViewState, tool: Tool, events: Vec<Event>) {
    let mut output = ctx.run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::splat(500.0))),
            events,
            ..Default::default()
        },
        |ui| {
            let (_, response) =
                ui.allocate_exact_size(Vec2::splat(400.0), egui::Sense::click_and_drag());
            interact(ui, &response, view, tool, 48, &mut 0.0);
        },
    );
    // This test drives input only; there is no GPU integration to apply textures.
    output.textures_delta.clear();
}
fn drag(tool: Tool, button: PointerButton) -> ViewState {
    let ctx = egui::Context::default();
    let mut view = ViewState::default();
    let start = Pos2::new(200.0, 200.0);
    input_frame(&ctx, &mut view, tool, vec![]);
    input_frame(
        &ctx,
        &mut view,
        tool,
        vec![
            Event::PointerMoved(start),
            Event::PointerButton {
                pos: start,
                button,
                pressed: true,
                modifiers: Modifiers::NONE,
            },
        ],
    );
    input_frame(
        &ctx,
        &mut view,
        tool,
        vec![Event::PointerMoved(Pos2::new(250.0, 225.0))],
    );
    view
}
#[test]
fn native_pointer_events_adjust_window_pan_and_zoom() {
    let window = drag(Tool::Window, PointerButton::Primary);
    assert!(window.window.width > ViewState::default().window.width);
    assert!(window.window.center < 40.0);
    let pan = drag(Tool::Pan, PointerButton::Primary);
    assert!(pan.pan[0] > 0.0 && pan.pan[1] > 0.0);
    assert_eq!(pan.window, ViewState::default().window);
    let right_pan = drag(Tool::Window, PointerButton::Secondary);
    assert!(right_pan.pan[0] > 0.0 && right_pan.pan[1] > 0.0);
    let zoom = drag(Tool::Zoom, PointerButton::Primary);
    assert!(zoom.zoom < 1.0 && zoom.zoom >= 0.05);
    for view in [window, pan, right_pan, zoom] {
        assert!(view.validate().is_ok());
    }
}

fn wheel(tool: Tool, pinch: bool) -> ViewState {
    let ctx = egui::Context::default();
    let mut view = ViewState {
        slice: 24,
        zoom: 2.0,
        ..Default::default()
    };
    let mut scroll = 0.0;
    for index in 0..8 {
        let mut events = vec![Event::PointerMoved(Pos2::new(200., 200.))];
        if index == 1 {
            events.push(Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: Vec2::new(25., -100.),
                phase: egui::TouchPhase::Move,
                modifiers: Modifiers::NONE,
            });
            if pinch {
                events.push(Event::Zoom(1.2));
            }
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
                interact(ui, &response, &mut view, tool, 48, &mut scroll);
            },
        );
        output.textures_delta.clear();
    }
    view
}

#[test]
fn trackpad_navigation_never_browses_images_in_pan_or_zoom_tools() {
    assert_eq!(Tool::default(), Tool::Pan);
    let moved = wheel(Tool::Pan, false);
    assert_eq!(moved.slice, 24);
    assert_eq!(moved.zoom, 2.0);
    assert!(moved.pan[0] > 0. && moved.pan[1] < 0.);
    let zoomed = wheel(Tool::Zoom, false);
    assert_eq!(zoomed.slice, 24);
    assert_ne!(zoomed.zoom, 2.0);
    assert_eq!(zoomed.pan, [0.; 2]);
    assert!(wheel(Tool::Browse, false).slice > 24);
    assert!(wheel(Tool::Crosshair, false).slice > 24);
    let pinched = wheel(Tool::Browse, true);
    assert_eq!(
        pinched.slice, 24,
        "one pinch batch must never also advance a slice"
    );
    assert!((pinched.zoom - 2.4).abs() < 0.001);
}

#[test]
fn space_drag_temporarily_pans_from_contrast_and_crosshair_tools() {
    for tool in [Tool::Window, Tool::Crosshair, Tool::Distance, Tool::Zoom] {
        let ctx = egui::Context::default();
        let mut view = ViewState::default();
        input_frame(&ctx, &mut view, tool, vec![]);
        input_frame(
            &ctx,
            &mut view,
            tool,
            vec![
                Event::Key {
                    key: egui::Key::Space,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: Modifiers::NONE,
                },
                Event::PointerMoved(Pos2::new(200., 200.)),
                Event::PointerButton {
                    pos: Pos2::new(200., 200.),
                    button: PointerButton::Primary,
                    pressed: true,
                    modifiers: Modifiers::NONE,
                },
            ],
        );
        input_frame(
            &ctx,
            &mut view,
            tool,
            vec![Event::PointerMoved(Pos2::new(260., 230.))],
        );
        assert!(view.pan[0] > 0. && view.pan[1] > 0.);
        assert_eq!(view.window, ViewState::default().window);
        assert_eq!(view.slice, 0);
        assert_eq!(view.zoom, 1.);
    }
    let middle = drag(Tool::Window, PointerButton::Middle);
    assert!(middle.pan[0] > 0. && middle.pan[1] > 0.);
    assert_eq!(middle.window, ViewState::default().window);
}
