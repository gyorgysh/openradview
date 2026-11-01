// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
fn key(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers,
    }
}
fn frame(ctx: &egui::Context, events: Vec<egui::Event>, run: impl FnMut(&mut egui::Ui)) {
    let mut output = ctx.run_ui(
        egui::RawInput {
            events,
            ..Default::default()
        },
        run,
    );
    output.textures_delta.clear();
}
#[test]
fn remapping_rejects_conflicts_and_changes_input_and_labels_together() {
    let ctx = egui::Context::default();
    let mut bindings = Bindings::default();
    assert!(
        bindings
            .set(Action::Pan, Some('Z'))
            .unwrap_err()
            .contains("Zoom")
    );
    assert_eq!(bindings, Bindings::default());
    for invalid in ['p', '1', 'é', '\0'] {
        assert!(bindings.set(Action::Pan, Some(invalid)).is_err());
    }
    assert!(bindings.set(Action::Next, None).is_err());
    bindings.set(Action::Pan, Some('B')).unwrap();
    bindings.set(Action::Ellipse, Some('E')).unwrap();
    install(&ctx, bindings);
    assert_eq!(label(&ctx, Action::Pan), "Pan [B]");
    assert_eq!(label(&ctx, Action::Ellipse), "Ellipse ROI [E]");
    frame(&ctx, vec![key(egui::Key::P, egui::Modifiers::NONE)], |ui| {
        assert!(!pressed(ui, Action::Pan));
    });
    frame(&ctx, vec![key(egui::Key::B, egui::Modifiers::NONE)], |ui| {
        assert!(pressed(ui, Action::Pan));
        assert!(
            !pressed(ui, Action::Pan),
            "One active owner consumes the event"
        );
    });
    for modifiers in [
        egui::Modifiers::COMMAND,
        egui::Modifiers::CTRL,
        egui::Modifiers::SHIFT,
        egui::Modifiers::ALT,
    ] {
        frame(&ctx, vec![key(egui::Key::B, modifiers)], |ui| {
            assert!(!pressed(ui, Action::Pan));
        });
    }
}
#[test]
fn text_focus_disabled_ui_dialogs_and_menus_keep_their_keys() {
    let ctx = egui::Context::default();
    let mut text = String::new();
    frame(&ctx, vec![], |ui| {
        ui.text_edit_singleline(&mut text).request_focus();
    });
    frame(
        &ctx,
        vec![
            key(egui::Key::P, egui::Modifiers::NONE),
            egui::Event::Text("p".into()),
        ],
        |ui| {
            assert!(!pressed(ui, Action::Pan));
            ui.text_edit_singleline(&mut text);
        },
    );
    assert_eq!(text, "p");
    if let Some(id) = ctx.memory(|memory| memory.focused()) {
        ctx.memory_mut(|memory| memory.surrender_focus(id));
    }
    frame(&ctx, vec![key(egui::Key::P, egui::Modifiers::NONE)], |ui| {
        ui.add_enabled_ui(false, |ui| assert!(!pressed(ui, Action::Pan)));
        view_input_enabled(ui.ctx(), false);
        assert!(!pressed(ui, Action::Pan));
        view_input_enabled(ui.ctx(), true);
        assert!(pressed(ui, Action::Pan));
    });
    let popup = egui::Id::new("shortcut-menu-test");
    egui::Popup::open_id(&ctx, popup);
    frame(&ctx, vec![key(egui::Key::P, egui::Modifiers::NONE)], |ui| {
        assert!(!pressed(ui, Action::Pan))
    });
    egui::Popup::close_all(&ctx);
}
#[test]
fn unsupported_tools_do_not_activate_or_consume_their_events() {
    let ctx = egui::Context::default();
    let mut bindings = Bindings::default();
    bindings.set(Action::Crosshair, Some('C')).unwrap();
    bindings.set(Action::Ellipse, Some('E')).unwrap();
    install(&ctx, bindings);
    for (key_value, scalar, crosshair, measurements, expected) in [
        (egui::Key::W, false, false, true, Tool::Pan),
        (egui::Key::W, true, false, true, Tool::Window),
        (egui::Key::C, true, false, true, Tool::Pan),
        (egui::Key::C, true, true, true, Tool::Crosshair),
        (egui::Key::E, true, true, false, Tool::Pan),
        (egui::Key::E, true, true, true, Tool::EllipseRoi),
    ] {
        let mut tool = Tool::Pan;
        frame(&ctx, vec![key(key_value, egui::Modifiers::NONE)], |ui| {
            tool_input(ui, &mut tool, scalar, crosshair, measurements);
            assert!(tool == expected);
            if expected == Tool::Pan {
                assert!(ui.input(|i| i.key_pressed(key_value)));
            }
        });
    }
}

#[test]
fn fixed_open_uses_platform_command_modifier_and_retains_modal_ownership() {
    let ctx = egui::Context::default();
    let mut text = String::new();
    frame(&ctx, vec![], |ui| {
        ui.text_edit_singleline(&mut text).request_focus();
    });
    for (modifiers, expected) in [
        (egui::Modifiers::NONE, false),
        (egui::Modifiers::COMMAND, true),
        (
            egui::Modifiers {
                ctrl: true,
                command: true,
                ..Default::default()
            },
            true,
        ),
        (
            egui::Modifiers {
                mac_cmd: true,
                command: true,
                ..Default::default()
            },
            true,
        ),
        (
            egui::Modifiers {
                shift: true,
                command: true,
                ..Default::default()
            },
            false,
        ),
        (
            egui::Modifiers {
                alt: true,
                command: true,
                ..Default::default()
            },
            false,
        ),
        (
            egui::Modifiers {
                ctrl: true,
                mac_cmd: true,
                command: true,
                ..Default::default()
            },
            false,
        ),
    ] {
        frame(&ctx, vec![key(egui::Key::O, modifiers)], |ui| {
            assert_eq!(pressed(ui, Action::OpenFiles), expected);
            ui.text_edit_singleline(&mut text);
        });
    }
    view_input_enabled(&ctx, false);
    frame(
        &ctx,
        vec![key(egui::Key::O, egui::Modifiers::COMMAND)],
        |ui| assert!(!pressed(ui, Action::OpenFiles)),
    );
}
