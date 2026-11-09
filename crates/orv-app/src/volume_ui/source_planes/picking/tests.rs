// SPDX-License-Identifier: GPL-3.0-or-later
use super::super::tests::{finish, scene, source, until};
use super::*;
use crate::labels::{RegionMap, RegionSelection};
use orv_core::labels::{BrushAction, BrushShape, Labels, PhysicalBrush};

fn selected(source: &Arc<GpuVolumeSource>) -> Scene {
    let mut scene = scene(source.clone());
    let base = Arc::new(Labels::empty(source.volume().clone()).unwrap());
    let proposal = base
        .propose_physical_stroke(
            &PhysicalBrush {
                label: 7,
                action: BrushAction::Add,
                radius_mm: 2.,
                shape: BrushShape::Sphere,
                clip: None,
                protected_ids: [false; 256],
            },
            &[[15., 17., 4.]],
            || false,
        )
        .unwrap();
    scene.selection =
        Some(RegionSelection::new(RegionMap::Manual(proposal.preview().clone()), 7).unwrap());
    scene.pick = true;
    scene
}
fn view(source: Arc<GpuVolumeSource>) -> View {
    View::new(
        source,
        wgpu::TextureFormat::Rgba8UnormSrgb,
        egui::Context::default(),
    )
    .unwrap()
}
#[test]
fn native_plane_probe_gates_live_display_map_owner_and_real_plane_occlusion() {
    let source = source();
    let mut desired = selected(&source);
    let mut view = view(source.clone());
    finish(&mut view, &desired);
    let (frame, state) = current(view.front.as_ref().unwrap(), &desired).unwrap();
    let old = run(
        Job {
            engine: view.engine.clone(),
            frame,
            current: state,
            pixel: [32, 24],
        },
        &|| false,
    )
    .unwrap();
    assert_eq!(old.kind(), SurfaceProbeKind::Obscured);
    assert!(old.hit().is_none());
    let edits: [fn(&mut Scene); 9] = [
        |s| s.pick = false,
        |s| s.display.pan[0] += 0.01,
        |s| s.state.camera.yaw_degrees += 1.,
        |s| s.style.lighting = Lighting::HEADLIGHT,
        |s| s.luminance = 0.5,
        |s| s.surface_lighting = false,
        |s| s.planes[0].as_mut().unwrap().window.center += 1.,
        |s| s.planes[1].as_mut().unwrap().invert = true,
        |s| s.size[0] += 1,
    ];
    for edit in edits {
        let mut changed = desired.clone();
        edit(&mut changed);
        assert!(current(view.front.as_ref().unwrap(), &changed).is_none());
    }
    let mut changed = desired.clone();
    changed.selection = Some(
        RegionSelection::new(
            RegionMap::Manual(Arc::new(Labels::empty(source.volume().clone()).unwrap())),
            7,
        )
        .unwrap(),
    );
    assert!(current(view.front.as_ref().unwrap(), &changed).is_none());
    // Only the frontal coronal plane covers this small region. Other planes
    // are parallel to this original centre ray and do not invent a blocker.
    desired.planes[1] = None;
    finish(&mut view, &desired);
    let (frame, state) = current(view.front.as_ref().unwrap(), &desired).unwrap();
    assert!(!old.is_current(&state, [32, 24]));
    let hit = run(
        Job {
            engine: view.engine.clone(),
            frame,
            current: state,
            pixel: [32, 24],
        },
        &|| false,
    )
    .unwrap();
    assert_eq!(hit.kind(), SurfaceProbeKind::Hit);
    assert_eq!(hit.hit().unwrap().key.number, 7);
    let generation = view.generation;
    for _ in 0..100 {
        view.update(&desired);
    }
    assert_eq!(view.generation, generation);
}

#[test]
fn changed_cut_revokes_old_pick_and_new_view_only_picks_visible_cells() {
    use orv_core::volume_view::PhysicalClip;

    let source = source();
    let mut desired = selected(&source);
    // The coronal source plane otherwise hides the small generated region.
    desired.planes[1] = None;
    let mut view = view(source);
    finish(&mut view, &desired);
    let (frame, original) = current(view.front.as_ref().unwrap(), &desired).unwrap();
    let old_scope = original.scope.clone();
    let pixel = [32, 24];
    let old = run(
        Job {
            engine: view.engine.clone(),
            frame,
            current: original.clone(),
            pixel,
        },
        &|| false,
    )
    .unwrap();
    assert_eq!(old.kind(), SurfaceProbeKind::Hit);
    assert_eq!(old.hit().unwrap().face.cell(), [15, 15, 4]);

    let mut cut = desired.clone();
    cut.state.clip = Some(PhysicalClip {
        minimum_mm: [-5., 17., -5.],
        maximum_mm: [35., 28., 13.],
    });
    let engine = view.engine.clone();
    let blocked = engine.lock().unwrap();
    view.update(&cut);
    assert!(!old_scope.is_active());
    assert!(!old.is_current(&original, pixel));
    assert!(current(view.front.as_ref().unwrap(), &cut).is_none());
    drop(blocked);
    finish(&mut view, &cut);
    let (frame, clipped) = current(view.front.as_ref().unwrap(), &cut).unwrap();
    let clipped_hit = run(
        Job {
            engine: view.engine.clone(),
            frame,
            current: clipped,
            pixel,
        },
        &|| false,
    )
    .unwrap();
    assert_eq!(clipped_hit.kind(), SurfaceProbeKind::Hit);
    assert_eq!(clipped_hit.hit().unwrap().face.cell(), [15, 19, 4]);

    cut.state.clip.as_mut().unwrap().minimum_mm[1] = -5.;
    cut.state.clip.as_mut().unwrap().maximum_mm[1] = 14.;
    finish(&mut view, &cut);
    let (frame, hidden) = current(view.front.as_ref().unwrap(), &cut).unwrap();
    assert!(!clipped_hit.is_current(&hidden, pixel));
    let result = run(
        Job {
            engine: view.engine.clone(),
            frame,
            current: hidden,
            pixel,
        },
        &|| false,
    )
    .unwrap();
    assert_eq!(result.kind(), SurfaceProbeKind::Empty);
    assert!(result.hit().is_none());

    finish(&mut view, &desired);
    let (frame, restored) = current(view.front.as_ref().unwrap(), &desired).unwrap();
    assert!(!result.is_current(&restored, pixel));
    let hit = run(
        Job {
            engine: view.engine.clone(),
            frame,
            current: restored,
            pixel,
        },
        &|| false,
    )
    .unwrap();
    assert_eq!(hit.kind(), SurfaceProbeKind::Hit);
    assert_eq!(hit.hit().unwrap().face.cell(), [15, 15, 4]);
}
#[test]
fn plane_camera_round_trip_needs_a_new_originating_scope_and_complete_image() {
    let source = source();
    let desired = selected(&source);
    let mut view = view(source);
    finish(&mut view, &desired);
    let original = view.front.as_ref().unwrap().image.clone();
    let old = view.scope.clone().unwrap();
    let engine = view.engine.clone();
    let blocked = engine.lock().unwrap();
    let mut moved = desired.clone();
    moved.state.camera.yaw_degrees = 31.;
    view.update(&moved);
    assert!(!old.is_active());
    let middle = view.scope.clone().unwrap();
    view.update(&desired);
    assert!(!middle.is_active());
    assert!(
        !view.current_front(&desired),
        "An old front cannot receive the renewed scope"
    );
    assert!(Arc::ptr_eq(&original, &view.front.as_ref().unwrap().image));
    drop(blocked);
    finish(&mut view, &desired);
    let front = view.front.as_ref().unwrap();
    assert!(!Arc::ptr_eq(&original, &front.image));
    assert!(Arc::ptr_eq(
        view.scope.as_ref().unwrap(),
        front.scope.as_ref().unwrap()
    ));
    assert!(current(front, &desired).is_some());
}
#[test]
fn pending_plane_probe_retires_on_new_mpr_input_suspension_and_view_teardown() {
    for suspend in [false, true] {
        let source = source();
        let ctx = egui::Context::default();
        let desired = selected(&source);
        let mut view = view(source);
        finish(&mut view, &desired);
        view.set_link_epoch(1);
        let scope = view.scope.clone().unwrap();
        let original = view.front.as_ref().unwrap().image.clone();
        let (frame, current) = current(view.front.as_ref().unwrap(), &desired).unwrap();
        let engine = view.engine.clone();
        let weak = Arc::downgrade(&engine);
        let blocked = engine.lock().unwrap();
        let mut probe = Probe::new(ctx).unwrap();
        let generation = probe.worker.submit(Job {
            engine: engine.clone(),
            frame,
            current,
            pixel: [32, 24],
        });
        probe.pending = Some(Pending {
            generation,
            pixel: [32, 24],
            viewport: (
                egui::Id::new("test"),
                egui::LayerId::background(),
                egui::Rect::EVERYTHING,
                egui::Rect::EVERYTHING,
            ),
        });
        view.probe = Some(probe);
        view.picked_point = Some([1., 2., 3.]);
        let render_generation = view.generation;
        view.set_link_epoch(2);
        assert!(view.probe.as_ref().unwrap().pending.is_none());
        assert!(view.take_point().is_none());
        assert_eq!(
            view.generation, render_generation,
            "MPR input cannot rerender an unchanged scene"
        );
        assert!(scope.is_active());
        assert!(Arc::ptr_eq(&original, &view.front.as_ref().unwrap().image));
        if suspend {
            view.suspend();
            assert!(!scope.is_active());
            assert!(view.front.is_none());
        }
        drop(blocked);
        until(|| Arc::strong_count(&engine) == 2);
        assert!(view.probe.as_ref().unwrap().worker.take().is_none());
        drop(view);
        drop(engine);
        until(|| weak.upgrade().is_none());
        assert!(!scope.is_active());
    }
}

#[test]
fn native_section_click_returns_only_the_checked_cell_centre_and_cancels_while_mpr_is_pending() {
    let source = source();
    let mut desired = selected(&source);
    desired.planes[1] = None;
    let mut view = view(source);
    let ctx = egui::Context::default();
    let mut time = 0.;
    let mut rect = egui::Rect::NOTHING;
    let mut draw = |view: &mut View, events, source_current| {
        time += 0.1;
        let mut output = ctx.run_ui(
            egui::RawInput {
                time: Some(time),
                events,
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(400., 300.),
                )),
                ..Default::default()
            },
            |ui| {
                let (bounds, response) =
                    ui.allocate_exact_size(egui::vec2(200., 160.), egui::Sense::click_and_drag());
                rect = bounds;
                view.show(ui, &response, desired.clone(), false, source_current);
            },
        );
        output.textures_delta.clear();
        (view.take_point(), rect)
    };
    let (_, rect) = draw(&mut view, vec![], true);
    finish(&mut view, &desired);
    let point = rect.center();
    let button = |pressed| egui::Event::PointerButton {
        pos: point,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    let _ = draw(
        &mut view,
        vec![egui::Event::PointerMoved(point), button(true)],
        true,
    );
    let mut picked = draw(&mut view, vec![button(false)], true).0;
    until(|| {
        if picked.is_none() {
            picked = draw(&mut view, vec![], true).0;
        }
        picked.is_some()
    });
    assert_eq!(picked, Some([15., 15., 4.]));
    let scope = view.scope.clone().unwrap();
    let generation = view.generation;
    let engine = view.engine.clone();
    let blocked = engine.lock().unwrap();
    // Move the test clock beyond the double-click interval before a new click.
    for _ in 0..10 {
        let _ = draw(&mut view, vec![], true);
    }
    let _ = draw(&mut view, vec![button(true)], true);
    let _ = draw(&mut view, vec![button(false)], true);
    assert!(view.probe.as_ref().unwrap().pending.is_some());
    let _ = draw(&mut view, vec![button(true)], true);
    let _ = draw(&mut view, vec![button(false)], true);
    assert!(
        view.probe.as_ref().unwrap().pending.is_none(),
        "Double-click Fit cancels the first asynchronous click"
    );
    for _ in 0..10 {
        let _ = draw(&mut view, vec![], true);
    }
    let _ = draw(&mut view, vec![button(true)], true);
    let _ = draw(&mut view, vec![button(false)], true);
    assert!(view.probe.as_ref().unwrap().pending.is_some());
    assert!(draw(&mut view, vec![], false).0.is_none());
    assert!(view.probe.as_ref().unwrap().pending.is_none());
    assert!(scope.is_active());
    assert_eq!(view.generation, generation);
    drop(blocked);
    until(|| Arc::strong_count(&engine) == 2);
    assert!(draw(&mut view, vec![], true).0.is_none());
}
