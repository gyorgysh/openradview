// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use crate::labels::{
    tests::labels,
    worker::{self, Output},
};
use std::sync::atomic::AtomicBool;

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
