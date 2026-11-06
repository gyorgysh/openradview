// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use crate::labels::{
    tests::labels,
    worker::{self, Output},
};
use std::sync::atomic::AtomicBool;

fn scope() -> StrokeScope {
    StrokeScope::new(
        &labels(),
        1,
        &orv_core::ViewState::default(),
        egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400., 400.)),
    )
}
fn state() -> LabelUi {
    let mut state = LabelUi {
        current: Some(labels()),
        ..Default::default()
    };
    let map = state.current.as_ref().unwrap().clone();
    state.regions.sync(&map, 1, Some(1));
    state.regions.seeded.add([1, 1, 1], SeedClass::Include);
    state.regions.seeded.add([6, 6, 1], SeedClass::Exclude);
    state.regions.sync(&map, 1, Some(1));
    state
}
fn preview(state: &LabelUi) -> Preview {
    let map = state.current.as_ref().unwrap().clone();
    let seeded = &state.regions.seeded;
    let request = seeded.request(&map, state.label, Some(1)).unwrap();
    let (seeds, reservation) = seeded.copy_for_worker().unwrap();
    let result = worker::run(
        Operation::SeededPreview(
            map,
            request,
            seeds,
            reservation,
            seeded.version,
            seeded.epoch,
        ),
        &AtomicBool::new(false),
    )
    .unwrap();
    let Output::SeededPreview(preview) = result else {
        panic!("expected seeded preview")
    };
    *preview
}
#[test]
fn preview_is_private_and_apply_is_one_undoable_revision() {
    let mut state = state();
    let base = state.current.as_ref().unwrap().clone();
    let p = preview(&state);
    assert!(p.summary.added > 0);
    assert_eq!(p.summary.removed, 0);
    assert!(state.regions.seeded.accept(&p, &base));
    state.regions.seeded.preview = Some(p);
    assert!(state.region_preview_visible());
    assert!(state.illustration_masks(0).is_err());
    assert!(Arc::ptr_eq(state.current.as_ref().unwrap(), &base));
    assert!(!state.dirty);
    assert!(!Arc::ptr_eq(state.regions.displayed(&base), &base));
    state.apply_seeded_preview(Some(1));
    assert!(state.dirty);
    assert_eq!(state.undo.len(), 1);
    assert!(!state.region_preview_visible());
    let applied = state.current.as_ref().unwrap().clone();
    assert!(applied.values().iter().any(|v| *v == 1));
    state.undo_edit();
    assert!(Arc::ptr_eq(state.current.as_ref().unwrap(), &base));
    state.redo_edit();
    assert!(Arc::ptr_eq(state.current.as_ref().unwrap(), &applied));
}
#[test]
fn changed_seed_rectangle_label_slice_base_or_mode_rejects_old_preview() {
    for change in 0..8 {
        let mut state = state();
        let p = preview(&state);
        let base = state.current.as_ref().unwrap().clone();
        let mut slice = Some(1);
        match change {
            0 => state.regions.seeded.add([3, 3, 1], SeedClass::Include),
            1 => state.regions.seeded.origin = [2, 1],
            2 => state.regions.seeded.size = [7, 8],
            3 => state.label = 2,
            4 => slice = Some(2),
            5 => state.current = Some(Arc::new(base.try_clone(|| false).unwrap())),
            6 => state.stop_painting(),
            _ => state.regions.seeded.size = [4, 8],
        }
        state
            .regions
            .sync(state.current.as_ref().unwrap(), state.label, slice);
        assert!(
            !state
                .regions
                .seeded
                .accept(&p, state.current.as_ref().unwrap())
        );
        state.regions.seeded.preview = Some(p);
        state.apply_seeded_preview(slice);
        assert!(!state.dirty);
        assert!(state.undo.is_empty());
        assert!(
            state
                .current
                .as_ref()
                .unwrap()
                .values()
                .iter()
                .all(|v| *v == 0)
        );
    }
}
#[test]
fn changing_source_clears_seed_positions_and_conflicts_refuse_results() {
    let mut state = state();
    let p = preview(&state);
    let replacement = labels();
    state.current = Some(replacement.clone());
    state.regions.sync(&replacement, 1, Some(1));
    assert!(!state.regions.seeded.accept(&p, &replacement));
    assert!(state.regions.seeded.seeds.is_empty());
    assert_eq!(state.regions.seeded.slice, None);
    state.regions.seeded.add([2, 2, 1], SeedClass::Include);
    state.regions.seeded.add([2, 2, 1], SeedClass::Exclude);
    state.regions.sync(&replacement, 1, Some(1));
    let seeded = &state.regions.seeded;
    let request = seeded.request(&replacement, 1, Some(1)).unwrap();
    let (seeds, reservation) = seeded.copy_for_worker().unwrap();
    assert!(
        worker::run(
            Operation::SeededPreview(
                replacement,
                request,
                seeds,
                reservation,
                seeded.version,
                seeded.epoch
            ),
            &AtomicBool::new(false)
        )
        .is_err()
    );
}
#[test]
fn selecting_seed_on_another_image_replaces_the_old_seed_set() {
    let mut state = state();
    state.regions.seed_picking = Some(SeedClass::Exclude);
    state.regions.picking = true;
    state.regions.pick([3, 4, 2]);
    assert!(!state.regions.picking);
    assert!(state.regions.seed_picking.is_none());
    assert_eq!(state.regions.seeded.slice, Some(2));
    assert_eq!(
        state.regions.seeded.seeds,
        [SeedPoint {
            position: [3, 4],
            class: SeedClass::Exclude
        }]
    );
    let map = state.current.as_ref().unwrap().clone();
    state.regions.sync(&map, 1, Some(2));
    assert!(state.regions.seeded.request(&map, 1, Some(2)).is_none());
}

#[test]
fn cancelled_worker_keeps_the_applied_map_and_does_not_publish_a_preview() {
    let state = state();
    let base = state.current.as_ref().unwrap().clone();
    let seeded = &state.regions.seeded;
    let request = seeded.request(&base, state.label, Some(1)).unwrap();
    let (seeds, reservation) = seeded.copy_for_worker().unwrap();
    let result = worker::run(
        Operation::SeededPreview(
            base.clone(),
            request,
            seeds,
            reservation,
            seeded.version,
            seeded.epoch,
        ),
        &AtomicBool::new(true),
    );
    assert!(result.is_err() || matches!(result, Ok(Output::Cancelled)));
    assert!(base.values().iter().all(|v| *v == 0));
    assert!(!state.dirty);
    assert!(!state.region_preview_visible());
}

#[test]
fn transformed_source_click_records_the_requested_seed_without_painting() {
    for turns in 0..4 {
        let mut state = state();
        state.regions.seed_picking = Some(SeedClass::Include);
        state.regions.picking = true;
        let image = state.current.as_ref().unwrap().source().frames()[1].clone();
        let view = orv_core::ViewState {
            transform: orv_core::DisplayTransform {
                quarter_turns: turns,
                flip_horizontal: true,
                flip_vertical: true,
            },
            ..Default::default()
        };
        let ctx = egui::Context::default();
        let mut position = None;
        for pressed in [None, Some(true), Some(false)] {
            let mut events = Vec::new();
            if let Some(pos) = position {
                events.push(egui::Event::PointerMoved(pos));
                if let Some(pressed) = pressed {
                    events.push(egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    });
                }
            }
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(500., 500.),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| {
                    let (_, response) = ui
                        .allocate_exact_size(egui::vec2(400., 400.), egui::Sense::click_and_drag());
                    position =
                        crate::annotations::screen_at(response.rect, &image, &view, [2.2, 4.9]);
                    state.canvas(ui, &response, &image, &view, 0);
                },
            );
            output.textures_delta.clear();
        }
        assert!(state.regions.seeded.seeds.contains(&SeedPoint {
            position: [2, 5],
            class: SeedClass::Include
        }));
        assert!(state.open);
        assert!(!state.regions.picking);
        assert!(!state.dirty);
        assert!(
            state
                .current
                .as_ref()
                .unwrap()
                .values()
                .iter()
                .all(|v| *v == 0)
        );
    }
}

#[test]
fn hint_click_is_recorded_when_press_and_release_arrive_in_one_frame() {
    let mut state = state();
    state.regions.seed_picking = Some(SeedClass::Include);
    state.regions.picking = true;
    let image = state.current.as_ref().unwrap().source().frames()[1].clone();
    let view = orv_core::ViewState::default();
    let ctx = egui::Context::default();
    let mut position = None;
    for click in [false, true] {
        let mut events = Vec::new();
        if click {
            let pos = position.unwrap();
            events.push(egui::Event::PointerMoved(pos));
            for pressed in [true, false] {
                events.push(egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                });
            }
        }
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(500., 500.),
                )),
                events,
                ..Default::default()
            },
            |ui| {
                let (_, response) =
                    ui.allocate_exact_size(egui::vec2(400., 400.), egui::Sense::click_and_drag());
                position = crate::annotations::screen_at(response.rect, &image, &view, [2., 5.]);
                state.canvas(ui, &response, &image, &view, 0);
            },
        );
        output.textures_delta.clear();
    }
    assert!(state.regions.seeded.seeds.contains(&SeedPoint {
        position: [2, 5],
        class: SeedClass::Include,
    }));
    assert!(!state.regions.picking);
    assert!(!state.dirty);
}

#[test]
fn space_navigation_completed_in_one_frame_does_not_add_a_hint() {
    let mut state = state();
    let original_seeds = state.regions.seeded.seeds.clone();
    state.regions.seed_picking = Some(SeedClass::Include);
    state.regions.picking = true;
    let image = state.current.as_ref().unwrap().source().frames()[1].clone();
    let view = orv_core::ViewState::default();
    let ctx = egui::Context::default();
    let mut position = None;
    for click in [false, true] {
        let mut events = Vec::new();
        if click {
            let pos = position.unwrap();
            events.push(egui::Event::Key {
                key: egui::Key::Space,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            });
            events.push(egui::Event::PointerMoved(pos));
            for pressed in [true, false] {
                events.push(egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                });
            }
            events.push(egui::Event::Key {
                key: egui::Key::Space,
                physical_key: None,
                pressed: false,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            });
        }
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(500., 500.),
                )),
                events,
                ..Default::default()
            },
            |ui| {
                let (_, response) =
                    ui.allocate_exact_size(egui::vec2(400., 400.), egui::Sense::click_and_drag());
                position = crate::annotations::screen_at(response.rect, &image, &view, [2., 5.]);
                state.canvas(ui, &response, &image, &view, 0);
            },
        );
        output.textures_delta.clear();
    }
    assert_eq!(state.regions.seeded.seeds, original_seeds);
    assert!(state.regions.picking);
    assert!(!state.dirty);
}

#[test]
fn hint_drag_keeps_last_motion_when_release_arrives_in_the_same_frame() {
    let mut state = state();
    state.regions.seed_picking = Some(SeedClass::Include);
    state.regions.picking = true;
    let image = state.current.as_ref().unwrap().source().frames()[1].clone();
    let view = orv_core::ViewState::default();
    let ctx = egui::Context::default();
    let mut positions: Option<[egui::Pos2; 2]> = None;
    for step in 0..3 {
        let mut events = Vec::new();
        if let Some([start, end]) = positions {
            let pos = if step == 1 { start } else { end };
            events.push(egui::Event::PointerMoved(pos));
            events.push(egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: step == 1,
                modifiers: egui::Modifiers::NONE,
            });
        }
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(500., 500.),
                )),
                events,
                ..Default::default()
            },
            |ui| {
                let (_, response) =
                    ui.allocate_exact_size(egui::vec2(400., 400.), egui::Sense::click_and_drag());
                positions = Some([[1., 3.], [6., 3.]].map(|p| {
                    crate::annotations::screen_at(response.rect, &image, &view, p).unwrap()
                }));
                state.canvas(ui, &response, &image, &view, 0);
            },
        );
        output.textures_delta.clear();
    }
    for x in 1..=6 {
        assert!(state.regions.seeded.seeds.contains(&SeedPoint {
            position: [x, 3],
            class: SeedClass::Include,
        }));
    }
    assert!(!state.regions.picking);
    assert!(!state.dirty);
}

#[test]
fn hint_drag_keeps_both_endpoints_when_all_events_arrive_in_one_frame() {
    let mut state = state();
    state.regions.seed_picking = Some(SeedClass::Include);
    state.regions.picking = true;
    let image = state.current.as_ref().unwrap().source().frames()[1].clone();
    let view = orv_core::ViewState::default();
    let ctx = egui::Context::default();
    let mut positions: Option<[egui::Pos2; 3]> = None;
    for draw in [false, true] {
        let mut events = Vec::new();
        if draw {
            let [start, turn, end] = positions.unwrap();
            events.push(egui::Event::PointerMoved(start));
            events.push(egui::Event::PointerButton {
                pos: start,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            });
            events.push(egui::Event::PointerMoved(turn));
            events.push(egui::Event::PointerMoved(end));
            events.push(egui::Event::PointerButton {
                pos: end,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            });
        }
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(500., 500.),
                )),
                events,
                ..Default::default()
            },
            |ui| {
                let (_, response) =
                    ui.allocate_exact_size(egui::vec2(400., 400.), egui::Sense::click_and_drag());
                positions = Some([[1., 2.], [1., 5.], [6., 5.]].map(|p| {
                    crate::annotations::screen_at(response.rect, &image, &view, p).unwrap()
                }));
                state.canvas(ui, &response, &image, &view, 0);
            },
        );
        output.textures_delta.clear();
    }
    for y in 2..=5 {
        assert!(state.regions.seeded.seeds.contains(&SeedPoint {
            position: [1, y],
            class: SeedClass::Include,
        }));
    }
    for x in 1..=6 {
        assert!(
            state.regions.seeded.seeds.contains(&SeedPoint {
                position: [x, 5],
                class: SeedClass::Include,
            }),
            "missing column {x}"
        );
    }
    assert!(!state.regions.seeded.seeds.contains(&SeedPoint {
        position: [3, 3],
        class: SeedClass::Include,
    }));
    assert!(!state.regions.picking);
    assert!(!state.dirty);
}

#[test]
fn hint_strokes_fill_gaps_remove_as_one_and_cancel_without_mutating_labels() {
    let mut state = state();
    let original = state.current.as_ref().unwrap().clone();
    let old_preview = preview(&state);
    let seed = &mut state.regions.seeded;
    let before = seed.seeds.clone();
    assert!(seed.begin_stroke([1, 3, 1], SeedClass::Include, scope()));
    assert!(seed.extend_stroke([6, 3, 1]));
    seed.finish_stroke();
    for x in 1..=6 {
        assert!(seed.seeds.contains(&SeedPoint {
            position: [x, 3],
            class: SeedClass::Include
        }));
    }
    state.regions.sync(&original, 1, Some(1));
    assert!(!state.regions.seeded.accept(&old_preview, &original));
    let updated_preview = preview(&state);
    assert!(state.regions.seeded.accept(&updated_preview, &original));
    state.regions.seeded.remove_last();
    assert_eq!(state.regions.seeded.seeds, before);
    assert!(
        state
            .regions
            .seeded
            .begin_stroke([2, 4, 1], SeedClass::Exclude, scope())
    );
    assert!(state.regions.seeded.extend_stroke([5, 4, 1]));
    state.stop_painting();
    assert_eq!(state.regions.seeded.seeds, before);
    assert!(Arc::ptr_eq(state.current.as_ref().unwrap(), &original));
    assert!(!state.dirty);
    assert!(state.undo.is_empty());
}

#[test]
fn hint_stroke_stops_at_boundaries_and_reverts_on_image_change() {
    let mut seeded = Seeded::default();
    seeded.add([0, 0, 1], SeedClass::Include);
    let original_typed = seeded.typed;
    assert!(seeded.begin_stroke([1, 1, 1], SeedClass::Exclude, scope()));
    assert!(seeded.extend_stroke([3, 3, 1]));
    seeded.break_stroke();
    assert!(seeded.extend_stroke([7, 3, 1]));
    assert!(!seeded.seeds.contains(&SeedPoint {
        position: [5, 3],
        class: SeedClass::Exclude
    }));
    assert!(!seeded.extend_stroke([7, 3, 2]));
    assert_eq!(
        seeded.seeds,
        [SeedPoint {
            position: [0, 0],
            class: SeedClass::Include
        }]
    );
    assert_eq!(seeded.typed, original_typed);
    assert!(!seeded.drawing());
}

#[test]
fn active_hint_cancels_when_its_source_label_or_view_changes() {
    for change in 0..7 {
        let mut state = state();
        let base = state.current.as_ref().unwrap().clone();
        let before = state.regions.seeded.seeds.clone();
        let mut image = base.source().frames()[1].clone();
        let mut view = orv_core::ViewState::default();
        let ctx = egui::Context::default();
        let mut rect = egui::Rect::NOTHING;
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            (rect, _) =
                ui.allocate_exact_size(egui::vec2(400., 400.), egui::Sense::click_and_drag());
        });
        output.textures_delta.clear();
        assert!(state.regions.seeded.begin_stroke(
            [2, 4, 1],
            SeedClass::Include,
            StrokeScope::new(&base, state.label, &view, rect),
        ));
        state.regions.picking = true;
        state.regions.seed_picking = Some(SeedClass::Include);
        let mut selected = 0;
        let mut size = egui::vec2(400., 400.);
        match change {
            0 => state.current = Some(Arc::new(base.try_clone(|| false).unwrap())),
            1 => state.label = 2,
            2 => view.zoom = 2.,
            3 => view.transform.flip_horizontal = true,
            4 => size = egui::vec2(380., 400.),
            5 => selected = 1,
            _ => image = base.source().frames()[2].clone(),
        }
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let (_, response) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());
            state.canvas(ui, &response, &image, &view, selected);
        });
        output.textures_delta.clear();
        assert_eq!(state.regions.seeded.seeds, before, "change {change}");
        assert!(!state.regions.seeded.drawing(), "change {change}");
        assert!(!state.regions.picking, "change {change}");
        assert!(state.open, "change {change}");
        assert!(!state.dirty, "change {change}");
    }
}

#[test]
fn sync_cancels_active_hint_before_the_next_canvas_frame() {
    for change in 0..5 {
        let mut state = state();
        let base = state.current.as_ref().unwrap().clone();
        let previous = state.regions.seeded.seeds.clone();
        assert!(state.regions.seeded.begin_stroke(
            [2, 4, 1],
            SeedClass::Include,
            StrokeScope::new(
                &base,
                1,
                &orv_core::ViewState::default(),
                egui::Rect::EVERYTHING
            ),
        ));
        state.regions.picking = true;
        state.regions.seed_picking = Some(SeedClass::Include);
        assert!(!state.regions.sync(&base, 1, Some(1)));
        let replacement = match change {
            0 => Arc::new(base.try_clone(|| false).unwrap()),
            1 => labels(),
            _ => base.clone(),
        };
        let label = if change == 2 { 2 } else { 1 };
        let slice = match change {
            3 => Some(2),
            4 => None,
            _ => Some(1),
        };
        assert!(state.regions.sync(&replacement, label, slice));
        if change == 1 {
            assert!(state.regions.seeded.seeds.is_empty());
        } else {
            assert_eq!(state.regions.seeded.seeds, previous);
        }
        assert!(!state.regions.seeded.drawing());
        assert!(!state.regions.picking);
        assert!(state.regions.seed_picking.is_none());
        assert!(!state.dirty);
    }
}

#[test]
fn seed_index_is_bounded_deduplicated_and_rolls_back_an_overflowing_stroke() {
    let mut seeded = Seeded::default();
    for x in 0..MAX_SEEDS {
        seeded.add([x, 0, 0], SeedClass::Include);
    }
    assert_eq!(seeded.seeds.len(), MAX_SEEDS);
    for x in 0..MAX_SEEDS {
        seeded.add([x, 0, 0], SeedClass::Include);
    }
    assert_eq!(seeded.seeds.len(), MAX_SEEDS);
    seeded.remove_last();
    assert_eq!(seeded.seeds.len(), MAX_SEEDS - 1);
    assert!(seeded.begin_stroke([MAX_SEEDS - 1, 0, 0], SeedClass::Include, scope()));
    assert!(!seeded.extend_stroke([MAX_SEEDS, 0, 0]));
    assert_eq!(seeded.seeds.len(), MAX_SEEDS - 1);
    seeded.add([MAX_SEEDS - 1, 0, 0], SeedClass::Include);
    assert!(seeded.begin_stroke([MAX_SEEDS - 1, 0, 0], SeedClass::Include, scope()));
    assert!(!seeded.extend_stroke([MAX_SEEDS, 0, 0]));
    assert_eq!(seeded.seeds.len(), MAX_SEEDS);
    assert!(!seeded.drawing());
    assert!(seeded.error.is_some());
    seeded.remove_last();
    assert_eq!(seeded.seeds.len(), MAX_SEEDS - 1);
    seeded.add([MAX_SEEDS - 1, 0, 0], SeedClass::Exclude);
    assert_eq!(seeded.seeds.len(), MAX_SEEDS);
    assert_eq!(seeded.seeds.last().unwrap().class, SeedClass::Exclude);
}

#[test]
fn transformed_source_drag_records_a_continuous_hint() {
    for turns in 0..4 {
        let mut state = state();
        state.regions.seed_picking = Some(SeedClass::Include);
        state.regions.picking = true;
        let image = state.current.as_ref().unwrap().source().frames()[1].clone();
        let view = orv_core::ViewState {
            transform: orv_core::DisplayTransform {
                quarter_turns: turns,
                flip_horizontal: true,
                flip_vertical: true,
            },
            ..Default::default()
        };
        let ctx = egui::Context::default();
        let mut position = None;
        for step in 0..4 {
            let mut events = Vec::new();
            if let Some(pos) = position {
                events.push(egui::Event::PointerMoved(pos));
                if step == 1 || step == 3 {
                    events.push(egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed: step == 1,
                        modifiers: egui::Modifiers::NONE,
                    });
                }
            }
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(500., 500.),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| {
                    let (_, response) = ui
                        .allocate_exact_size(egui::vec2(400., 400.), egui::Sense::click_and_drag());
                    position = crate::annotations::screen_at(
                        response.rect,
                        &image,
                        &view,
                        if step == 0 { [1., 3.] } else { [6., 3.] },
                    );
                    state.canvas(ui, &response, &image, &view, 0);
                },
            );
            output.textures_delta.clear();
        }
        for x in 1..=6 {
            assert!(
                state.regions.seeded.seeds.contains(&SeedPoint {
                    position: [x, 3],
                    class: SeedClass::Include
                }),
                "turn {turns}, x {x}"
            );
        }
        assert!(state.open);
        assert!(!state.regions.picking);
        assert!(!state.dirty);
    }
}
