// SPDX-License-Identifier: GPL-3.0-or-later
//! Shared placement for auxiliary windows; `open` supplies egui's close button.
use eframe::egui;

pub fn window(title: &'static str, ctx: &egui::Context) -> egui::Window<'static> {
    egui::Window::new(title)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .max_width((ctx.content_rect().width() - 48.).max(280.))
        .max_height((ctx.content_rect().height() - 64.).max(200.))
        .collapsible(false)
        .vscroll(true)
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
}

/// Escape closes an informational dialog, unless a text field or popup menu is
/// using the key. Consent, unsaved-work and key-capture dialogs do not use this.
pub fn escape(ctx: &egui::Context, open: &mut bool) {
    if *open
        && !ctx.egui_wants_keyboard_input()
        && !ctx.any_popup_open()
        && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
    {
        *open = false;
    }
}

/// Reveal a newly focused control in this dialog without an idle scroll timer.
pub fn reveal_focus(ui: &egui::Ui) {
    if let Some(id) = ui.memory(|memory| memory.focused())
        && let Some(response) = ui.ctx().read_response(id)
        && response.gained_focus()
        && response.layer_id == ui.layer_id()
        && ui.min_rect().contains_rect(response.rect)
    {
        response.scroll_to_me_animation(None, egui::style::ScrollAnimation::none());
    }
}

#[derive(Clone, Copy, Default)]
struct FocusReturn {
    /// Last focused control in the main window's panels.
    main: Option<egui::Id>,
    /// Focus left the main window while a dialog or popup was open.
    pending: bool,
}

/// Call once per frame after all UI. When the dialogs and popups that took
/// keyboard focus from the main window have closed, focus returns to the
/// main-window control focused before them, usually the one that opened them.
/// Focus the user moved within the main window is left alone.
pub fn return_focus(ctx: &egui::Context) {
    let key = egui::Id::new("openradview-focus-return");
    let mut state: FocusReturn = ctx.data(|data| data.get_temp(key)).unwrap_or_default();
    let focused = ctx.memory(|memory| memory.focused()).and_then(|id| {
        ctx.read_response(id)
            .map(|response| (id, response.layer_id))
    });
    // Windows sit in the middle order and popups in the foreground; tooltips
    // and the main panels do not take focus away.
    let dialog_open = ctx.memory(|memory| {
        memory
            .areas()
            .visible_layer_ids()
            .iter()
            .any(|layer| matches!(layer.order, egui::Order::Middle | egui::Order::Foreground))
    });
    match focused {
        Some((id, layer)) if layer.order == egui::Order::Background => {
            state = FocusReturn {
                main: Some(id),
                pending: false,
            };
        }
        Some(_) => state.pending = true,
        None if dialog_open => state.pending |= state.main.is_some(),
        None => {
            if std::mem::take(&mut state.pending)
                && let Some(main) = state.main
                && ctx.read_response(main).is_some()
            {
                ctx.memory_mut(|memory| memory.request_focus(main));
                ctx.request_repaint();
            }
        }
    }
    ctx.data_mut(|data| data.insert_temp(key, state));
}

#[cfg(feature = "mcp")]
pub fn heading(ui: &mut egui::Ui, title: &str) -> bool {
    ui.horizontal(|ui| {
        ui.heading(title);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.button("×").on_hover_text("Close dialog").clicked()
        })
        .inner
    })
    .inner
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn closing_a_focused_dialog_returns_focus_to_its_opener_but_not_while_it_stays_open() {
        let ctx = egui::Context::default();
        let mut open = false;
        let ids = std::cell::Cell::new((egui::Id::NULL, egui::Id::NULL));
        let mut time = 0.;
        let mut render = |focus_opener: bool, focus_dialog: bool, close: bool| {
            // Advance past the window's closing fade.
            time += 1.;
            let input = egui::RawInput {
                time: Some(time),
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    let opener = ui.button("Image info…");
                    let _ = ui.button("Other control");
                    if focus_opener {
                        opener.request_focus();
                        open = true;
                    }
                    ids.set((opener.id, ids.get().1));
                });
                if close {
                    open = false;
                }
                window("Focus test", ui.ctx())
                    .open(&mut open)
                    .show(ui.ctx(), |ui| {
                        let inside = ui.button("Inside");
                        if focus_dialog {
                            inside.request_focus();
                        }
                        ids.set((ids.get().0, inside.id));
                    });
                return_focus(ui.ctx());
            });
            output.textures_delta.clear();
            ctx.memory(|memory| memory.focused())
        };
        render(true, false, false);
        render(false, true, false);
        assert_eq!(render(false, false, false), Some(ids.get().1));
        // Clicking away inside an open dialog does not jump focus behind it.
        ctx.memory_mut(|memory| memory.surrender_focus(ids.get().1));
        render(false, false, false);
        assert_eq!(render(false, false, false), None);
        render(false, true, false);
        render(false, false, true);
        render(false, false, false);
        assert_eq!(render(false, false, false), Some(ids.get().0));
        // Opening can drop focus before it ever enters the dialog.
        render(true, false, false);
        ctx.memory_mut(|memory| memory.surrender_focus(ids.get().0));
        render(false, false, false);
        render(false, false, true);
        render(false, false, false);
        assert_eq!(render(false, false, false), Some(ids.get().0));
        // Without a dialog, focus cleared in the main window stays cleared.
        ctx.memory_mut(|memory| memory.surrender_focus(ids.get().0));
        for _ in 0..3 {
            assert_eq!(render(false, false, false), None);
        }
    }
    #[test]
    fn tab_reveals_a_dialog_control_below_the_fold_without_continuous_repaints() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut last = None;
        let mut time = 0.;
        let mut render = |events, first_focus: bool| {
            time += 1.;
            let mut output = ctx.run_ui(
                egui::RawInput {
                    time: Some(time),
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(860., 640.),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| {
                    window("Keyboard scroll test", ui.ctx())
                        .default_height(240.)
                        .show(ui.ctx(), |ui| {
                            let first = ui.button("First action");
                            if first_focus {
                                first.request_focus();
                            }
                            for _ in 0..30 {
                                ui.label("More dialog content");
                            }
                            let response = ui.button("Last action");
                            last = Some((response.id, response.rect, ui.clip_rect()));
                            reveal_focus(ui);
                        });
                },
            );
            output.textures_delta.clear();
            output.viewport_output[&egui::ViewportId::ROOT].repaint_delay
        };
        render(vec![], true);
        render(vec![], false);
        render(
            vec![egui::Event::Key {
                key: egui::Key::Tab,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            false,
        );
        for _ in 0..3 {
            render(vec![], false);
        }
        let delay = render(vec![], false);
        let (id, rect, clip) = last.unwrap();
        assert_eq!(ctx.memory(|memory| memory.focused()), Some(id));
        assert!(
            clip.contains_rect(rect),
            "Focused control clipped: {rect:?} / {clip:?}"
        );
        assert!(delay > std::time::Duration::from_secs(1), "{delay:?}");
    }
    #[test]
    fn auxiliary_window_remains_centered_and_inside_resized_viewports() {
        let ctx = egui::Context::default();
        for size in [
            egui::vec2(1280., 840.),
            egui::vec2(860., 640.),
            egui::vec2(1280., 840.),
        ] {
            let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
            for pass in 0..3 {
                let mut bounds = None;
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(screen),
                        ..Default::default()
                    },
                    |ui| {
                        bounds = window("Centered help test", ui.ctx())
                            .open(&mut true)
                            .default_width(430.)
                            .show(ui.ctx(), |ui| {
                                ui.label("Help content");
                                ui.add_space(180.);
                            })
                            .map(|r| r.response.rect);
                    },
                );
                output.textures_delta.clear();
                let rect = bounds.unwrap();
                assert!(screen.contains_rect(rect), "{rect:?} outside {screen:?}");
                assert!(
                    pass == 0 || rect.center().distance(screen.center()) < 1.5,
                    "Window drifted: {rect:?}"
                );
            }
        }
    }
    #[test]
    fn overflowing_dialog_can_scroll_its_entire_last_line_into_view() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        for size in [egui::vec2(1280., 840.), egui::vec2(860., 640.)] {
            for pass in 0..4 {
                let mut output = ctx.run_ui(egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                    ..Default::default()
                }, |ui| {
                    window("Scrolling dialog test", ui.ctx()).open(&mut true)
                        .default_width(470.).default_height(600.)
                        .show(ui.ctx(), |ui| {
                            for _ in 0..30 { ui.label("Wrapped dialog content, including further help and privacy information."); }
                            ui.label("Last line must be readable in full.");
                            ui.add_space(12.);
                            ui.scroll_to_cursor_animation(Some(egui::Align::BOTTOM), egui::style::ScrollAnimation::none());
                        });
                });
                output.textures_delta.clear();
                if pass == 3 {
                    let shape = output
                        .shapes
                        .iter()
                        .find_map(|s| match &s.shape {
                            egui::epaint::Shape::Text(t)
                                if t.galley.job.text == "Last line must be readable in full." =>
                            {
                                Some((s.clip_rect, t.galley.rect.translate(t.pos.to_vec2())))
                            }
                            _ => None,
                        })
                        .unwrap();
                    assert!(
                        shape.0.contains_rect(shape.1),
                        "Last line clipped: {shape:?}"
                    );
                }
            }
        }
    }
    #[test]
    fn escape_closes_an_informational_dialog_but_not_while_text_is_edited() {
        let ctx = egui::Context::default();
        let escape_key = || {
            vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }]
        };
        let mut text = String::new();
        for editing in [true, false] {
            let mut open = true;
            let field = egui::Id::new("escape-test-field");
            for events in [vec![], escape_key()] {
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        events,
                        ..Default::default()
                    },
                    |ui| {
                        if editing {
                            ui.memory_mut(|memory| memory.request_focus(field));
                        } else {
                            ui.memory_mut(|memory| memory.surrender_focus(field));
                        }
                        ui.add(egui::TextEdit::singleline(&mut text).id(field));
                        escape(ui.ctx(), &mut open);
                    },
                );
                output.textures_delta.clear();
            }
            assert_eq!(open, editing, "editing {editing}");
        }
    }
}
