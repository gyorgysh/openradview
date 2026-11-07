// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use crate::labels::RegionMap;
use std::time::{Duration, Instant};

struct Fixture {
    engine: Arc<Mutex<Option<Engine>>>,
    frame: SectionPickFrame,
    current: SectionPickState,
    input: Input,
}
impl Fixture {
    fn new() -> Self {
        Self::with_clip(None)
    }
    fn with_clip(clip: Option<orv_core::volume_view::PhysicalClip>) -> Self {
        let gpu = super::super::tests::source();
        let mut scene = super::super::tests::scene(gpu.clone());
        scene.state.clip = clip;
        scene.pick = false;
        scene.erase = true;
        scene.planes = [None, None, None];
        scene.style.material =
            orv_core::volume_material::VolumeMaterial::new(&[-1000., 1000.].map(|value| {
                orv_core::volume_material::MaterialPoint {
                    value,
                    luminance: 0.3,
                    extinction_per_mm: 0.,
                }
            }))
            .unwrap();
        let empty = Arc::new(Labels::empty(gpu.volume().clone()).unwrap());
        let brush = PhysicalBrush {
            label: 7,
            action: BrushAction::Add,
            radius_mm: 4.,
            shape: BrushShape::Sphere,
            clip: None,
            protected_ids: [false; 256],
        };
        let p = empty
            .propose_physical_stroke(&brush, &[gpu.volume().center()], || false)
            .unwrap();
        let base = p.preview().clone();
        scene.selection = Some(RegionSelection::new(RegionMap::Manual(base.clone()), 7).unwrap());
        let mut engine = Engine::new(gpu, wgpu::TextureFormat::Rgba8UnormSrgb).unwrap();
        let image = engine.render(&scene, &|| false, &|| false).unwrap();
        let frame = SectionPickFrame::new(image, PickScope::new().unwrap());
        let current = SectionPickState::from_frame(&frame).unwrap();
        Self {
            engine: Arc::new(Mutex::new(Some(engine))),
            frame,
            current,
            input: Input {
                base,
                number: 7,
                series: 2,
                link_epoch: 11,
            },
        }
    }
    fn job(&self) -> Job {
        Job {
            engine: self.engine.clone(),
            frame: self.frame.clone(),
            current: self.current.clone(),
            pixel: [32, 24],
            brush: self.input.brush(1.5, &self.current),
        }
    }
}

#[test]
fn clipped_surface_stamp_never_erases_cells_outside_the_retained_side() {
    use orv_core::volume_view::PhysicalClip;

    let uncut = Fixture::new();
    let unrestricted = run(uncut.job(), &|| false).unwrap();
    let mut excluded_without_cut = 0;
    for z in 0..9 {
        for y in 0..23 {
            for x in 0..15 {
                let voxel = [x, y, z];
                if uncut.input.base.value(voxel)
                    != unrestricted.proposal.proposal().preview().value(voxel)
                {
                    excluded_without_cut += 1;
                }
            }
        }
    }
    assert!(
        excluded_without_cut > 0,
        "the cut must affect the stamp footprint"
    );
    let f = Fixture::with_clip(Some(PhysicalClip {
        minimum_mm: [15., -5., -5.],
        maximum_mm: [35., 28., 13.],
    }));
    let output = run(f.job(), &|| false).unwrap();
    let preview = output.proposal.proposal().preview();
    assert!(output.proposal.summary().changed_voxels > 0);
    let mut changed = 0;
    for z in 0..9 {
        for y in 0..23 {
            for x in 0..31 {
                let voxel = [x, y, z];
                let before = f.input.base.value(voxel).unwrap();
                let after = preview.value(voxel).unwrap();
                if before != after {
                    assert_eq!((before, after), (7, 0));
                    assert!(x >= 15, "the stamp crossed the physical cut");
                    changed += 1;
                }
            }
        }
    }
    assert_eq!(changed, output.proposal.summary().changed_voxels);
    let mut later = f.current.clone();
    later.scene.volume.clip.as_mut().unwrap().minimum_mm[0] = 16.;
    assert!(!output.proposal.is_current(
        &later,
        &f.input.brush(1.5, &later),
        output.proposal.proposal().points_mm(),
    ));
    assert_eq!(f.input.base.value([15, 11, 4]), Some(7));
}
fn frame(
    ctx: &egui::Context,
    correction: &mut Correction,
    f: &Fixture,
    events: Vec<egui::Event>,
    extent: f32,
) {
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(500., 400.),
            )),
            events,
            ..Default::default()
        },
        |ui| {
            let (_, response) =
                ui.allocate_exact_size(egui::vec2(extent, 160.), egui::Sense::click_and_drag());
            correction.show(
                ui,
                &response,
                Some((f.frame.clone(), f.current.clone())),
                f.engine.clone(),
                false,
            );
        },
    );
    output.textures_delta.clear();
}

#[test]
fn correction_worker_keeps_base_frame_current_and_discards_cancelled_preview_owners() {
    let f = Fixture::new();
    let out = run(f.job(), &|| false).unwrap();
    assert!(out.proposal.summary().changed_voxels > 0);
    assert!(out.proposal.is_current(
        &f.current,
        &f.input.brush(1.5, &f.current),
        out.proposal.proposal().points_mm()
    ));
    assert!(f.current.matches(&f.frame));
    assert!(f.frame.scope().is_active());
    assert_eq!(
        f.frame
            .image()
            .scene()
            .surface
            .as_ref()
            .unwrap()
            .image
            .revision(),
        f.input.base.revision()
    );
    assert_eq!(
        out.image.scene().surface.as_ref().unwrap().image.revision(),
        out.proposal.proposal().preview().revision()
    );
    let weak = Arc::downgrade(out.proposal.proposal().preview());
    drop(out);
    assert!(weak.upgrade().is_none());
    assert!(run(f.job(), &|| true).is_err());
    let mut stale = f.job();
    stale.current.scene.volume.camera.yaw_degrees += 1.;
    assert!(run(stale, &|| false).is_err());
    f.current.scope.revoke();
    assert!(run(f.job(), &|| false).is_err());
}

#[test]
fn correction_handler_rechecks_source_context_radius_epoch_and_viewport_before_accepting_results() {
    let f = Fixture::new();
    let ctx = egui::Context::default();
    let mut correction = Correction::new(ctx.clone()).unwrap();
    correction.radius = 1.5;
    correction.sync(Some(f.input.clone()));
    correction.panel = false;
    frame(&ctx, &mut correction, &f, vec![], 200.);
    let generation = correction.worker.submit(f.job());
    correction.pending = Some(Pending {
        generation,
        input: f.input.clone(),
        brush: f.input.brush(1.5, &f.current),
        viewport: correction.viewport.unwrap(),
    });
    let deadline = Instant::now() + Duration::from_secs(15);
    while correction.preview.is_none() {
        assert!(
            Instant::now() < deadline,
            "Private correction worker timed out"
        );
        frame(&ctx, &mut correction, &f, vec![], 200.);
        std::thread::yield_now();
    }
    assert!(correction.panel);
    let weak = Arc::downgrade(
        correction
            .preview
            .as_ref()
            .unwrap()
            .proposal
            .proposal()
            .preview(),
    );
    for change in 0..4 {
        let mut input = f.input.clone();
        match change {
            0 => input.series += 1,
            1 => input.link_epoch += 1,
            2 => input.number = 8,
            _ => input.base = Arc::new(f.input.base.try_clone(|| false).unwrap()),
        }
        assert!(!input.same(&f.input));
    }
    let mut input = f.input.clone();
    input.link_epoch += 1;
    correction.sync(Some(input));
    assert!(correction.preview.is_none());
    assert!(weak.upgrade().is_none());
    correction.sync(Some(f.input.clone()));
    let out = run(f.job(), &|| false).unwrap();
    assert!(correction.accepts(&out, &f.current));
    correction.radius = 2.;
    assert!(!correction.accepts(&out, &f.current));
    correction.radius = 1.5;
    correction.preview = Some(out);
    frame(&ctx, &mut correction, &f, vec![], 220.);
    assert!(correction.preview.is_none());
    correction.preview = Some(run(f.job(), &|| false).unwrap());
    frame(
        &ctx,
        &mut correction,
        &f,
        vec![egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }],
        220.,
    );
    assert!(correction.preview.is_none());
    correction.preview = Some(run(f.job(), &|| false).unwrap());
    correction.sync(None);
    assert!(correction.preview.is_none());
    assert!(f.current.matches(&f.frame));
}

#[test]
fn erasing_without_visible_source_planes_has_its_own_originating_scope_and_does_not_retag_picks() {
    let f = Fixture::new();
    let source = f.frame.image().source().clone();
    let mut desired = super::super::tests::scene(source.clone());
    desired.planes = [None, None, None];
    desired.pick = false;
    desired.erase = true;
    desired.selection =
        Some(RegionSelection::new(RegionMap::Manual(f.input.base.clone()), 7).unwrap());
    let mut view = View::new(
        source,
        wgpu::TextureFormat::Rgba8UnormSrgb,
        egui::Context::default(),
    )
    .unwrap();
    super::super::tests::finish(&mut view, &desired);
    let scope = view.front.as_ref().unwrap().scope.clone().unwrap();
    assert!(scope.is_active());
    assert!(picking::current(view.front.as_ref().unwrap(), &desired).is_some());
    desired.erase = false;
    desired.pick = true;
    view.update(&desired);
    assert!(!scope.is_active());
    assert!(!view.current_front(&desired));
    super::super::tests::finish(&mut view, &desired);
    let pick_scope = view.front.as_ref().unwrap().scope.as_ref().unwrap();
    assert!(pick_scope.is_active());
    assert!(!Arc::ptr_eq(&scope, pick_scope));
    let mut settings = Settings {
        erase: true,
        ..Default::default()
    };
    assert!(settings.active());
    assert!(settings.planes(None).unwrap().iter().all(Option::is_none));
    settings.erase = false;
    assert!(!settings.active());
}

#[test]
fn native_correction_clicks_require_choose_mode_and_preserve_navigation_overrides() {
    let f = Fixture::new();
    let ctx = egui::Context::default();
    let mut correction = Correction::new(ctx.clone()).unwrap();
    correction.radius = 1.5;
    correction.sync(Some(f.input.clone()));
    frame(&ctx, &mut correction, &f, vec![], 200.);
    let centre = correction.viewport.unwrap().2.center();
    let click = |button| {
        vec![
            egui::Event::PointerMoved(centre),
            egui::Event::PointerButton {
                pos: centre,
                button,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::PointerButton {
                pos: centre,
                button,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
        ]
    };
    frame(
        &ctx,
        &mut correction,
        &f,
        click(egui::PointerButton::Primary),
        200.,
    );
    assert!(correction.pending.is_none());
    assert!(correction.preview.is_none());
    correction.panel = false;
    frame(
        &ctx,
        &mut correction,
        &f,
        click(egui::PointerButton::Secondary),
        200.,
    );
    assert!(correction.pending.is_none());
    let mut events = vec![egui::Event::Key {
        key: egui::Key::Space,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }];
    events.extend(click(egui::PointerButton::Primary));
    frame(&ctx, &mut correction, &f, events, 200.);
    assert!(correction.pending.is_none());
    frame(
        &ctx,
        &mut correction,
        &f,
        vec![egui::Event::Key {
            key: egui::Key::Space,
            physical_key: None,
            pressed: false,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }],
        200.,
    );
    frame(&ctx, &mut correction, &f, vec![], 200.);
    frame(
        &ctx,
        &mut correction,
        &f,
        click(egui::PointerButton::Primary),
        200.,
    );
    assert!(correction.pending.is_some() || correction.preview.is_some());
    correction.clear();
    assert!(correction.pending.is_none());
    assert!(correction.preview.is_none());
    assert!(f.current.matches(&f.frame));
}
