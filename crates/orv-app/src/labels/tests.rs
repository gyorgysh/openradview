// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use orv_core::{
    ViewState, WindowLevel,
    study::{Frame, Geometry},
    volume::Volume,
};
pub(super) fn labels() -> Arc<Labels> {
    let frames = (0..3)
        .map(|z| {
            Arc::new(Frame {
                memory: None,
                presentation: None,
                calibration: None,
                projection: None,
                sop: None,
                slab: None,
                scalar: None,
                validity: None,
                color: None,
                nifti: None,
                rgba: None,
                width: 8,
                height: 8,
                pixels: vec![20.; 64].into(),
                geometry: Some(Geometry {
                    nifti_transform: None,
                    frame_of_reference: Some("synthetic-brush-grid".into()),
                    position: [0., 0., z as f64],
                    orientation: [1., 0., 0., 0., 1., 0.],
                    spacing: [1., 1.],
                }),
                instance: z,
                frame_index: 0,
                monochrome1: false,
                default_window: WindowLevel {
                    function: Default::default(),
                    width: 100.,
                    center: 50.,
                },
            })
        })
        .collect();
    Arc::new(Labels::empty(Arc::new(Volume::new(frames).unwrap())).unwrap())
}
#[test]
fn primary_click_paints_on_source_grid_and_keeps_undo_checkpoint() {
    let ctx = egui::Context::default();
    let current = labels();
    let image = current.source().frames()[1].clone();
    let mut state = LabelUi {
        current: Some(current.clone()),
        paint: true,
        ..Default::default()
    };
    let pos = egui::pos2(200., 200.);
    for pressed in [None, Some(true), Some(false)] {
        let mut events = vec![egui::Event::PointerMoved(pos)];
        if let Some(pressed) = pressed {
            events.push(egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
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
                state.canvas(ui, &response, &image, &ViewState::default(), 0);
            },
        );
        output.textures_delta.clear();
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while state.busy() && std::time::Instant::now() < deadline {
        state.poll();
        std::thread::yield_now();
    }
    assert!(!state.busy(), "brush worker did not finish");
    assert!(state.dirty);
    assert_eq!(state.undo.len(), 1);
    assert!(current.values().iter().all(|v| *v == 0));
    assert!(
        state
            .current
            .as_ref()
            .unwrap()
            .values()
            .iter()
            .skip(64)
            .take(64)
            .any(|v| *v == 1)
    );
    assert!(
        state
            .current
            .as_ref()
            .unwrap()
            .values()
            .iter()
            .take(64)
            .all(|v| *v == 0)
    );
    assert!(Arc::ptr_eq(state.undo.front().unwrap(), &current));
}
#[test]
fn cancelled_edit_does_not_change_the_approved_source_or_labels() {
    let current = labels();
    let flag = std::sync::atomic::AtomicBool::new(true);
    let result = worker::run(
        Operation::Threshold(current.clone(), None, [0., 100.], 2),
        &flag,
    );
    assert!(result.is_err() || matches!(result, Ok(Output::Cancelled)));
    assert!(current.values().iter().all(|v| *v == 0));
    assert_eq!(current.source().frames()[0].pixels[0], 20.);
}

#[test]
fn oversized_label_grids_are_refused_from_metadata_before_invalid_pixels_are_prepared() {
    for (width, height, depth, reason) in [
        (2049, 2048, 2, "four million pixels per slice"),
        (1024, 1024, 33, "32 million voxels"),
    ] {
        let mut series = orv_core::study::demo_study().series[0].clone();
        series.source = None;
        let original = labels();
        series.frames = (0..depth)
            .map(|z| {
                let mut frame = (*original.source().frames()[0]).clone();
                frame.width = width;
                frame.height = height;
                frame.pixels = Vec::new().into();
                frame.geometry.as_mut().unwrap().position[2] = z as f64;
                Arc::new(frame)
            })
            .collect();
        let result = worker::run(
            Operation::Start(Arc::new(series)),
            &std::sync::atomic::AtomicBool::new(false),
        );
        assert!(matches!(result, Err(error) if error.contains(reason)));
    }
}

#[test]
fn edits_without_cell_changes_keep_the_current_revision_and_undo_history() {
    let current = labels();
    let mut state = LabelUi {
        current: Some(current.clone()),
        ..Default::default()
    };
    state.commit_change(Arc::new(current.try_clone(|| false).unwrap()));
    assert!(!state.dirty);
    assert!(state.undo.is_empty());
    assert!(Arc::ptr_eq(state.current.as_ref().unwrap(), &current));
    let mut edited = current.try_clone(|| false).unwrap();
    edited.paint(1, &[[3., 3.]], 1, 7, || false).unwrap();
    let edited = Arc::new(edited);
    state.commit_change(edited.clone());
    let mut repeated = edited.try_clone(|| false).unwrap();
    repeated.paint(1, &[[3., 3.]], 1, 7, || false).unwrap();
    state.commit_change(Arc::new(repeated));
    assert_eq!(state.undo.len(), 1);
    assert!(state.dirty);
    assert!(Arc::ptr_eq(state.current.as_ref().unwrap(), &edited));
    state.commit_change(Arc::new(Labels::empty(current.source_arc()).unwrap()));
    assert_eq!(
        state.undo.len(),
        2,
        "A replacement object must not be treated as an unchanged edit"
    );
}

#[test]
fn history_restores_exact_maps_and_saved_revision_without_scanning_cells() {
    let original = labels();
    let mut state = LabelUi {
        current: Some(original.clone()),
        ..Default::default()
    };
    state.mark_saved();
    let mut first = original.try_clone(|| false).unwrap();
    first.paint(1, &[[3., 3.]], 1, 7, || false).unwrap();
    let first = Arc::new(first);
    state.commit_change(first.clone());
    state.mark_saved();
    let mut second = first.try_clone(|| false).unwrap();
    second.paint(1, &[[6., 6.]], 1, 9, || false).unwrap();
    let second = Arc::new(second);
    state.commit_change(second.clone());
    assert!(state.dirty);
    state.undo_edit();
    assert!(Arc::ptr_eq(state.current.as_ref().unwrap(), &first));
    assert!(!state.dirty, "Undo reached the exact exported revision");
    state.undo_edit();
    assert!(Arc::ptr_eq(state.current.as_ref().unwrap(), &original));
    assert!(state.dirty);
    state.redo_edit();
    assert!(Arc::ptr_eq(state.current.as_ref().unwrap(), &first));
    assert!(!state.dirty);
    state.redo_edit();
    assert!(Arc::ptr_eq(state.current.as_ref().unwrap(), &second));
    assert!(state.dirty);
    assert!(state.redo.is_empty());
    state.undo_edit();
    let mut no_op = first.try_clone(|| false).unwrap();
    no_op.paint(1, &[[3., 3.]], 1, 7, || false).unwrap();
    state.commit_change(Arc::new(no_op));
    assert_eq!(state.redo.len(), 1, "No-op edits preserve redo");
    let mut branched = first.try_clone(|| false).unwrap();
    branched.paint(0, &[[1., 1.]], 1, 3, || false).unwrap();
    state.commit_change(Arc::new(branched));
    assert!(
        state.redo.is_empty(),
        "A real edit creates a new history branch"
    );
    assert!(state.dirty);
}

#[test]
fn history_retains_at_most_sixteen_changes_and_drops_them_on_discard() {
    let original = labels();
    let first_weak = Arc::downgrade(&original);
    let mut state = LabelUi {
        current: Some(original),
        ..Default::default()
    };
    state.mark_saved();
    for id in 1..=20 {
        let mut edited = state.current.as_ref().unwrap().try_clone(|| false).unwrap();
        edited.paint(1, &[[3., 3.]], 1, id, || false).unwrap();
        state.commit_change(Arc::new(edited));
    }
    assert_eq!(state.undo.len(), HISTORY_STEPS);
    assert!(
        first_weak.upgrade().is_none(),
        "The saved marker must not retain an old map"
    );
    for id in (4..20).rev() {
        state.undo_edit();
        assert_eq!(state.current.as_ref().unwrap().value([3, 3, 1]), Some(id));
    }
    assert!(state.undo.is_empty());
    assert_eq!(state.redo.len(), HISTORY_STEPS);
    state.undo_edit();
    assert_eq!(state.redo.len(), HISTORY_STEPS);
    for id in 5..=20 {
        state.redo_edit();
        assert_eq!(state.current.as_ref().unwrap().value([3, 3, 1]), Some(id));
    }
    assert_eq!(state.undo.len(), HISTORY_STEPS);
    state.undo_edit();
    let retained = Arc::downgrade(state.current.as_ref().unwrap());
    state.discard_manual();
    assert!(retained.upgrade().is_none());
    assert!(state.current.is_none());
    assert!(state.undo.is_empty() && state.redo.is_empty());
    assert!(state.saved.is_none() && !state.dirty);
}
