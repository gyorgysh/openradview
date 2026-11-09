// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use crate::labels::{tests::labels, worker};
use std::sync::atomic::AtomicBool;

pub(super) fn state() -> LabelUi {
    let mut state = LabelUi {
        current: Some(labels()),
        ..Default::default()
    };
    state.open_physical_tools();
    state.physical.radius = 1.;
    state.mark_saved();
    state
}
pub(super) fn context(source: &Arc<Volume>) -> PhysicalContext<'_> {
    PhysicalContext {
        source,
        selected: 0,
        point: [3., 3., 1.],
        plane: 0,
        generation: 71,
        ready: true,
    }
}
fn prepared(state: &LabelUi) -> Preview {
    let input = state.physical.input.as_ref().unwrap();
    let worker::Output::Physical(preview) = worker::run(
        Operation::Physical(
            input.base.clone(),
            Box::new(input.brush.clone()),
            input.point.into(),
            state.physical.epoch,
        ),
        &AtomicBool::new(false),
    )
    .unwrap() else {
        panic!("Expected physical preview")
    };
    *preview
}

#[test]
fn opening_physical_tools_preserves_the_supplied_overlay_until_manual_selection() {
    let mut state = LabelUi {
        supplied_active: true,
        ..Default::default()
    };
    state.open_physical_tools();
    assert!(state.supplied_active);
    state.select_manual_for_physical();
    assert!(!state.supplied_active);
}

#[test]
fn surface_erase_requires_a_resolved_physical_edit() {
    let mut state = state();
    let original = state.current.as_ref().unwrap().clone();
    let source = original.source_arc();
    state.sync_physical(&context(&source));
    assert!(!state.can_start_surface_erase(0, &source));
    state.physical.open = false;
    assert!(state.can_start_surface_erase(0, &source));
    assert!(state.surface_erase_base(0, &source).is_some());

    let preview = prepared(&state);
    assert!(state.physical.completed(preview, Some(&original)));
    assert!(!state.can_start_surface_erase(0, &source));
    assert!(state.surface_erase_base(0, &source).is_none());
    state.invalidate_physical();
    assert!(state.can_start_surface_erase(0, &source));

    state.physical.drawing = true;
    assert!(!state.can_start_surface_erase(0, &source));
    assert!(state.surface_erase_base(0, &source).is_none());
    state.physical.drawing = false;
    assert!(state.surface_erase_base(0, &source).is_some());
}

#[test]
fn private_stamp_is_shared_by_planes_and_3d_then_apply_and_history_retain_exact_maps() {
    let mut state = state();
    let original = state.current.as_ref().unwrap().clone();
    let source = original.source_arc();
    state.sync_physical(&context(&source));
    let preview = prepared(&state);
    // Closed unit sphere: centre and its six face neighbors on this unit grid.
    assert_eq!(preview.stroke.summary().changed_voxels, 7);
    let proposed = preview.stroke.preview().clone();
    assert!(state.physical.completed(preview, Some(&original)));
    assert!(state.physical_preview_visible());
    let selected =
        crate::labels::RegionSelection::new(crate::labels::RegionMap::Manual(proposed.clone()), 1)
            .unwrap();
    assert!(state.is_physical_preview_region(&selected));
    let other_id =
        crate::labels::RegionSelection::new(crate::labels::RegionMap::Manual(proposed.clone()), 2)
            .unwrap();
    assert!(!state.is_physical_preview_region(&other_id));
    let applied =
        crate::labels::RegionSelection::new(crate::labels::RegionMap::Manual(original.clone()), 1)
            .unwrap();
    assert!(!state.is_physical_preview_region(&applied));
    let supplied = Arc::new(
        orv_core::segments::Segments::from_binary(
            source.clone(),
            &[1],
            &[(1, 0)],
            &[0xff; 8],
            None,
            || false,
        )
        .unwrap(),
    );
    let supplied =
        crate::labels::RegionSelection::new(crate::labels::RegionMap::Supplied(supplied), 1)
            .unwrap();
    assert!(!state.is_physical_preview_region(&supplied));
    assert!(Arc::ptr_eq(state.displayed_manual(&original), &proposed));
    assert!(Arc::ptr_eq(
        &state.region_maps(0, &source).manual.unwrap().unwrap(),
        &proposed
    ));
    assert_eq!(original.value([3, 3, 1]), Some(0));
    assert_eq!(proposed.value([3, 3, 1]), Some(1));
    assert!(!state.dirty && state.undo.is_empty());
    assert!(state.illustration_masks(0).is_err());
    state.apply_physical();
    assert!(!state.is_physical_preview_region(&selected));
    assert!(Arc::ptr_eq(state.current.as_ref().unwrap(), &proposed));
    assert!(state.dirty && state.undo.len() == 1);
    assert!(!state.physical_preview_visible());
    assert!(state.illustration_masks(0).is_ok());
    state.undo_edit();
    assert!(Arc::ptr_eq(state.current.as_ref().unwrap(), &original));
    assert!(!state.dirty);
    state.redo_edit();
    assert!(Arc::ptr_eq(state.current.as_ref().unwrap(), &proposed));
    assert!(state.dirty);
    state.apply_physical();
    assert_eq!(state.undo.len(), 1);
}

#[test]
fn changed_context_or_edit_inputs_reject_late_private_stamps() {
    for change in 0..13 {
        let mut state = state();
        let original = state.current.as_ref().unwrap().clone();
        let source = original.source_arc();
        let mut context = context(&source);
        state.sync_physical(&context);
        let late = prepared(&state);
        match change {
            0 => context.point[0] += 1.,
            1 => context.generation += 1,
            2 => context.ready = false,
            3 => context.selected += 1,
            4 => state.physical.radius += 0.5,
            5 => state.physical.erase = true,
            6 => state.physical.shape = Shape::Plane,
            7 => state.display.visible_ids[1] = false,
            8 => state.label = 2,
            9 => state.current = Some(Arc::new(original.try_clone(|| false).unwrap())),
            10 => state.supplied_active = true,
            11 => state.open = true,
            _ => state.end_physical_tools(),
        }
        state.sync_physical(&context);
        assert!(
            !state.physical.completed(late, state.current.as_ref()),
            "change {change}"
        );
        state.apply_physical();
        assert!(!state.dirty && state.undo.is_empty(), "change {change}");
    }
    let mut state = state();
    let source = state.current.as_ref().unwrap().source_arc();
    state.sync_physical(&context(&source));
    let late = prepared(&state);
    let copy = Arc::new(Volume::new(source.frames().to_vec()).unwrap());
    state.sync_physical(&context(&copy));
    assert!(!state.physical.completed(late, state.current.as_ref()));
}

#[test]
fn disk_orientation_thickness_erase_protection_and_noop_are_explicit() {
    let mut state = state();
    let original = state.current.as_ref().unwrap().clone();
    let source = original.source_arc();
    let mut context = context(&source);
    state.physical.shape = Shape::Plane;
    state.physical.thickness = 0.5;
    for plane in 0..3 {
        context.plane = plane;
        state.sync_physical(&context);
        let preview = prepared(&state);
        assert_eq!(preview.stroke.summary().changed_voxels, 5);
        let axis = [2, 1, 0][plane];
        for (index, value) in preview.stroke.preview().values().iter().enumerate() {
            if *value != 0 {
                let voxel = [index % 8, (index / 8) % 8, index / 64];
                assert_eq!(voxel[axis] as f64, context.point[axis]);
            }
        }
    }
    let preview = prepared(&state);
    assert!(state.physical.completed(preview, Some(&original)));
    state.apply_physical();
    let filled = state.current.as_ref().unwrap().clone();
    state.physical.erase = true;
    state.sync_physical(&context);
    let preview = prepared(&state);
    assert_eq!(preview.stroke.summary().changed_voxels, 5);
    state.display.visible_ids[1] = false;
    state.sync_physical(&context);
    assert!(!state.physical.completed(preview, Some(&filled)));
    let input = state.physical.input.as_ref().unwrap();
    assert!(
        worker::run(
            Operation::Physical(
                input.base.clone(),
                Box::new(input.brush.clone()),
                input.point.into(),
                state.physical.epoch
            ),
            &AtomicBool::new(false),
        )
        .is_err()
    );
    state.physical.erase = false;
    state.display.visible_ids[1] = true;
    state.sync_physical(&context);
    let noop = prepared(&state);
    assert_eq!(noop.stroke.summary().changed_voxels, 0);
    assert!(state.physical.completed(noop, Some(&filled)));
    state.apply_physical();
    assert!(Arc::ptr_eq(state.current.as_ref().unwrap(), &filled));
    assert_eq!(state.undo.len(), 1);
}

#[test]
fn navigation_cancels_worker_and_blank_creation_uses_exact_workbench_source() {
    let mut state = state();
    let original = state.current.as_ref().unwrap().clone();
    let source = original.source_arc();
    let mut context = context(&source);
    state.sync_physical(&context);
    state.preview_physical(&egui::Context::default());
    assert!(state.busy());
    context.ready = false;
    context.generation += 1;
    state.sync_physical(&context);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while state.busy() {
        state.poll();
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert!(!state.physical_preview_visible());
    assert!(Arc::ptr_eq(state.current.as_ref().unwrap(), &original));
    let worker::Output::Start(blank) = worker::run(
        Operation::StartVolume(source.clone()),
        &AtomicBool::new(false),
    )
    .unwrap() else {
        panic!("Expected blank labels")
    };
    assert!(Arc::ptr_eq(&blank.source_arc(), &source));
}

#[test]
fn hidden_background_display_bit_does_not_protect_empty_source_cells() {
    let mut state = state();
    let source = state.current.as_ref().unwrap().source_arc();
    state.display.visible_ids.fill(false);
    state.display.visible_ids[1] = true;
    state.sync_physical(&context(&source));
    let preview = prepared(&state);
    assert_eq!(preview.stroke.summary().changed_voxels, 7);
    assert_eq!(preview.stroke.value([3, 3, 1]), Some(1));
}

#[test]
fn single_point_path_does_not_also_move_the_linked_crosshair() {
    use egui::{Event, Modifiers, PointerButton, Pos2, RawInput, Rect};
    use std::time::{Duration, Instant};

    let ctx = egui::Context::default();
    let mut session = crate::mpr::MprSession::start(
        crate::mpr_worker::tests::frames(),
        orv_core::WindowLevel::SOFT_TISSUE,
        ctx.clone(),
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    while !session.is_settled() {
        session.poll();
        assert!(Instant::now() < deadline, "initial MPR view did not settle");
        std::thread::yield_now();
    }
    let snapshot = session.snapshot.as_ref().unwrap().clone();
    let original_point = snapshot.point;
    let mut labels = LabelUi {
        current: Some(Arc::new(Labels::empty(snapshot.volume.clone()).unwrap())),
        ..Default::default()
    };
    labels.open_physical_tools();
    labels.sync_physical(&session.physical_context(&snapshot, 0));
    labels.physical.open = false;
    labels.physical.drawing = true;
    let area = Rect::from_min_size(Pos2::ZERO, egui::vec2(500., 500.));
    let mut frame = |session: &mut crate::mpr::MprSession, events: Vec<Event>| {
        let mut image_rect = Rect::NOTHING;
        let mut output = ctx.run_ui(
            RawInput {
                screen_rect: Some(area),
                events,
                ..Default::default()
            },
            |ui| {
                image_rect = session
                    .prepare_plane(ui, 0, area, &snapshot, &mut labels, 0)
                    .image_rect();
                labels.sync_physical(&session.physical_context(&snapshot, 0));
            },
        );
        output.textures_delta.clear();
        image_rect
    };
    let rect = frame(&mut session, vec![]);
    let world = [2., 2., original_point[2]];
    let image = &snapshot.images[0];
    let pixel = image
        .geometry
        .as_ref()
        .unwrap()
        .world_to_pixel(world)
        .unwrap();
    let point = crate::annotations::screen_at(rect, image, &session.views[0], pixel).unwrap();
    frame(&mut session, vec![Event::PointerMoved(point)]);
    for pressed in [true, false] {
        frame(
            &mut session,
            vec![Event::PointerButton {
                pos: point,
                button: PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            }],
        );
    }
    drop(frame);
    assert_eq!(
        session.target,
        Some(original_point),
        "a path click must not navigate"
    );
    while labels.busy() {
        labels.poll();
        assert!(
            Instant::now() < deadline,
            "physical preview did not complete"
        );
        std::thread::yield_now();
    }
    assert!(labels.physical_preview_visible());
    assert_eq!(
        labels
            .physical
            .preview
            .as_ref()
            .unwrap()
            .stroke
            .points_mm()
            .len(),
        1
    );
}
