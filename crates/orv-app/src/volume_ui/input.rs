// SPDX-License-Identifier: GPL-3.0-or-later
use eframe::egui;
use orv_core::{ViewState, volume_view::VolumeView};

/// Return whether interaction is active and whether the physical fit is requested.
pub(super) fn interact(
    ui: &egui::Ui,
    response: &egui::Response,
    state: &mut VolumeView,
    display: &mut ViewState,
) -> (bool, bool) {
    let rect = response.rect;
    let mut active = response.dragged();
    if response.hovered() {
        ui.input(|i| {
            let zoom = i.zoom_delta();
            let factor = if (zoom - 1.).abs() > f32::EPSILON {
                zoom
            } else {
                (i.smooth_scroll_delta.y * 0.004).exp()
            };
            display.zoom = (display.zoom * factor).clamp(0.05, 50.);
            active |= i.smooth_scroll_delta.y.abs() > 0.01 || (i.zoom_delta() - 1.).abs() > 0.001;
        });
    }
    if response.dragged() {
        let delta = ui.input(|i| i.pointer.delta());
        if crate::viewport::navigation_override(ui, response) {
            display.pan[0] = (display.pan[0] + delta.x / rect.width().max(1.)).clamp(-10., 10.);
            display.pan[1] = (display.pan[1] + delta.y / rect.height().max(1.)).clamp(-10., 10.);
        } else {
            state.camera.yaw_degrees =
                ((state.camera.yaw_degrees + f64::from(delta.x) * 0.4 + 180.).rem_euclid(360.))
                    - 180.;
            state.camera.pitch_degrees =
                (state.camera.pitch_degrees - f64::from(delta.y) * 0.4).clamp(-90., 90.);
        }
    }
    let fit = response.double_clicked();
    if fit {
        display.zoom = 1.;
        display.pan = [0.; 2];
    }
    (active, fit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Event, Modifiers, PointerButton, Pos2, RawInput, Rect, Vec2};
    use orv_core::{study::demo_study, volume::Volume};

    fn drag(button: PointerButton) -> (VolumeView, ViewState) {
        drag_with_space(button, false)
    }
    fn drag_with_space(button: PointerButton, space: bool) -> (VolumeView, ViewState) {
        let ctx = egui::Context::default();
        let volume = Volume::new(demo_study().series[0].frames.clone()).unwrap();
        let mut state = VolumeView::fit(&volume);
        let mut display = ViewState::default();
        let start = Pos2::new(200., 200.);
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
            vec![Event::PointerMoved(Pos2::new(250., 225.))],
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
                    interact(ui, &response, &mut state, &mut display);
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
}
