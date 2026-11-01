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
    state.regions.range = [20., 20.];
    state.regions.pick([2, 5, 1]);
    state
        .regions
        .sync(state.current.as_ref().unwrap(), state.label, Some(1));
    state
}
fn preview(state: &LabelUi) -> Preview {
    let map = state.current.as_ref().unwrap().clone();
    let request = state.regions.request(state.label, Some(1)).unwrap();
    let output = worker::run(
        Operation::Preview(map, request, state.regions.epoch),
        &AtomicBool::new(false),
    )
    .unwrap();
    let Output::Preview(preview) = output else {
        panic!("expected preview")
    };
    *preview
}
#[test]
fn an_unapplied_connected_preview_is_refused_for_illustration_without_discarding_it() {
    let mut state = state();
    let map = state.current.as_ref().unwrap().clone();
    state.regions.preview = Some(preview(&state));
    assert!(state.region_preview_visible());
    assert!(state.illustration_masks(0).is_err());
    assert!(state.region_preview_visible());
    assert!(Arc::ptr_eq(state.current.as_ref().unwrap(), &map));
    assert!(!state.dirty);
    state.apply_region_preview(Some(1));
    let captured = state.illustration_masks(0).unwrap().unwrap();
    let pixels = captured
        .prepare(&map.source_arc().frames()[1], 8, &|| false)
        .unwrap();
    assert_eq!(
        pixels.image.pixels.iter().filter(|p| p.a() != 0).count(),
        64
    );
}
#[test]
fn preview_is_private_and_explicit_apply_creates_one_undo_checkpoint() {
    let mut state = state();
    let original = state.current.as_ref().unwrap().clone();
    let preview = preview(&state);
    assert_eq!(preview.summary.changed_voxels, 64);
    assert_eq!(preview.summary.region_voxels, 64);
    assert!(state.regions.accept(&preview, &original));
    state.regions.preview = Some(preview);
    assert!(state.region_preview_visible());
    assert!(!Arc::ptr_eq(state.regions.displayed(&original), &original));
    assert!(!state.dirty);
    assert!(state.undo.is_empty());
    assert!(original.values().iter().all(|v| *v == 0));
    state.apply_region_preview(Some(1));
    assert!(state.dirty);
    assert_eq!(state.undo.len(), 1);
    assert!(Arc::ptr_eq(state.undo.front().unwrap(), &original));
    assert!(!state.region_preview_visible());
    assert_eq!(
        state
            .current
            .as_ref()
            .unwrap()
            .values()
            .iter()
            .filter(|v| **v == 1)
            .count(),
        64
    );
    state.apply_region_preview(Some(1));
    assert_eq!(state.undo.len(), 1);
}
#[test]
fn changed_interval_scope_seed_id_or_exact_map_invalidates_a_preview() {
    for change in 0..6 {
        let mut state = state();
        let prepared = preview(&state);
        let original = state.current.as_ref().unwrap().clone();
        let mut slice = Some(1);
        match change {
            0 => state.regions.range = [19., 21.],
            1 => state.regions.volume = true,
            2 => state.regions.pick([3, 5, 1]),
            3 => state.label = 2,
            4 => state.current = Some(Arc::new(original.try_clone(|| false).unwrap())),
            _ => slice = Some(2),
        }
        state
            .regions
            .sync(state.current.as_ref().unwrap(), state.label, slice);
        assert!(
            !state
                .regions
                .accept(&prepared, state.current.as_ref().unwrap())
        );
        state.regions.preview = Some(prepared);
        state.apply_region_preview(slice);
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
    let mut state = state();
    let prepared = preview(&state);
    state.stop_painting();
    assert!(
        !state
            .regions
            .accept(&prepared, state.current.as_ref().unwrap())
    );
}
#[test]
fn cancelled_preview_keeps_the_map_and_does_not_create_an_undo_entry() {
    let mut state = state();
    let current = state.current.as_ref().unwrap().clone();
    let result = worker::run(
        Operation::Preview(
            current.clone(),
            state.regions.request(1, Some(1)).unwrap(),
            state.regions.epoch,
        ),
        &AtomicBool::new(true),
    );
    assert!(result.is_err() || matches!(result, Ok(Output::Cancelled)));
    state.regions.preview = Some(preview(&state));
    state.regions.invalidate();
    assert!(Arc::ptr_eq(&current, state.current.as_ref().unwrap()));
    assert!(!state.dirty);
    assert!(state.undo.is_empty());
    assert!(!state.region_preview_visible());
}
#[test]
fn transformed_source_click_selects_the_nearest_seed_without_painting() {
    for turns in 0..4 {
        let mut state = state();
        state.regions.seed = None;
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
        assert_eq!(state.regions.seed, Some([2, 5, 1]));
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
