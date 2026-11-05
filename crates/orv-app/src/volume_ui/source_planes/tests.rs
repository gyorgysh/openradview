// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use orv_core::{
    Plane, WindowLevel,
    resources::MemoryBudget,
    study::{Geometry, demo_study},
    volume::Volume,
    volume_material::{MaterialPoint, VolumeMaterial},
    volume_view::Projection,
};
use orv_render::reslice::GpuReslicer;
use std::time::{Duration, Instant};

fn source() -> Arc<GpuVolumeSource> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: orv_render::configured_backends().unwrap(),
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapter = pollster::block_on(instance.request_adapter(&Default::default()))
        .expect("Actual GPU required for native source planes");
    let (d, q) =
        pollster::block_on(adapter.request_device(&orv_render::device_descriptor(&adapter)))
            .unwrap();
    let template = demo_study().series.remove(0).frames.remove(0);
    let volume = Arc::new(
        Volume::new(
            (0..9)
                .map(|z| {
                    let mut f = (*template).clone();
                    f.width = 31;
                    f.height = 23;
                    f.pixels = vec![10.; 31 * 23].into();
                    f.validity = None;
                    f.scalar = None;
                    f.rgba = None;
                    f.monochrome1 = false;
                    f.geometry = Some(Geometry {
                        position: [0., 0., f64::from(z)],
                        orientation: [1., 0., 0., 0., 1., 0.],
                        spacing: [1.; 2],
                        frame_of_reference: Some("generated native source planes".into()),
                        nifti_transform: None,
                    });
                    Arc::new(f)
                })
                .collect(),
        )
        .unwrap(),
    );
    GpuVolumeSource::new(&d, &q, volume, || false).unwrap()
}
fn scene(source: Arc<GpuVolumeSource>) -> Scene {
    let cpu = source.volume();
    let grids = [Plane::Axial, Plane::Coronal, Plane::Sagittal]
        .map(|p| cpu.reformat_grid(p, cpu.center(), 32).unwrap());
    let mut reslice = GpuReslicer::new(source.clone()).unwrap();
    let images = reslice
        .render_frames(grids.each_ref().map(Some), || false)
        .unwrap();
    Scene {
        planes: images.map(|image| {
            image.map(|image| SectionPlane {
                image,
                window: WindowLevel::source_range(0., 20.),
                invert: false,
            })
        }),
        selection: None,
        display: ViewState::default(),
        state: VolumeView {
            projection: Projection::Composite,
            quality: Quality::Refined,
            ..VolumeView::fit(cpu)
        },
        style: OpticalStyle {
            material: VolumeMaterial::new(&[-1000., 1000.].map(|value| MaterialPoint {
                value,
                luminance: 0.3,
                extinction_per_mm: 0.03,
            }))
            .unwrap(),
            lighting: Lighting::UNLIT,
        },
        luminance: 0.8,
        surface_lighting: true,
        size: [65, 49],
    }
}
fn until(mut done: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(15);
    while !done() {
        assert!(Instant::now() < end, "Native source-plane worker timed out");
        std::thread::yield_now();
    }
}
fn finish(view: &mut View, scene: &Scene) {
    until(|| {
        view.update(scene);
        assert!(view.error.is_none(), "{:?}", view.error);
        view.front
            .as_ref()
            .is_some_and(|front| front.scene.same(scene))
            && view.pending.is_none()
    });
}
#[test]
fn coalesced_planes_keep_their_completed_identity_requested_detail_and_idle_generation() {
    let source = source();
    let original = scene(source.clone());
    let mut latest = original.clone();
    let mut reslice = GpuReslicer::new(source.clone()).unwrap();
    let grid = source
        .volume()
        .reformat_grid(Plane::Axial, [15., 11., 5.], 32)
        .unwrap();
    latest.planes[0].as_mut().unwrap().image = reslice
        .render_frames([Some(&grid), None, None], || false)
        .unwrap()[0]
        .take()
        .unwrap();
    let mut view = View::new(
        source.clone(),
        wgpu::TextureFormat::Rgba8UnormSrgb,
        egui::Context::default(),
    )
    .unwrap();
    let engine = view.engine.clone();
    let blocked = engine.lock().unwrap();
    view.update(&original);
    let generation = view.generation;
    view.update(&latest);
    assert_eq!(
        view.generation, generation,
        "Moving planes must not starve the running complete image"
    );
    assert!(view.pending.as_ref().unwrap().same(&original));
    drop(blocked);
    until(|| {
        view.update(&latest);
        assert!(view.error.is_none(), "{:?}", view.error);
        view.front.is_some()
    });
    let old = &view.front.as_ref().unwrap();
    assert!(old.scene.same(&original));
    assert!(Arc::ptr_eq(
        &old.image.scene().planes[0].as_ref().unwrap().image,
        &original.planes[0].as_ref().unwrap().image
    ));
    finish(&mut view, &latest);
    let image = &view.front.as_ref().unwrap().image;
    assert!(Arc::ptr_eq(image.source(), &source));
    assert_eq!(image.scene().size, [65, 49]);
    assert_eq!(image.scene().volume.quality, Quality::Refined);
    assert!(Arc::ptr_eq(
        &image.scene().planes[0].as_ref().unwrap().image,
        &latest.planes[0].as_ref().unwrap().image
    ));
    let idle = view.generation;
    for _ in 0..100 {
        view.update(&latest);
    }
    assert_eq!(view.generation, idle);
    assert!(view.pending.is_none());
    assert!(view.worker.take().is_none());
    let plane =
        orv_core::oblique::ObliquePlane::from_angles(source.volume().center(), [0.; 3]).unwrap();
    let slab = orv_core::slab::Slab {
        projection: orv_core::slab::SlabProjection::Mean,
        thickness_mm: 3.,
        intervals: 2,
    };
    let grid = source
        .volume()
        .oblique_grid(&plane, 32, Some(slab))
        .unwrap();
    let mut inputs = latest
        .planes
        .each_ref()
        .map(|p| p.as_ref().map(|p| p.image.clone()));
    inputs[0] = reslice
        .render_frames([Some(&grid), None, None], || false)
        .unwrap()[0]
        .take();
    let views = std::array::from_fn(|_| ViewState::default());
    let settings = Settings {
        enabled: true,
        ..Default::default()
    };
    assert!(
        settings
            .planes(Some(LinkedPlanes {
                images: &inputs,
                views: &views
            }))
            .is_err()
    );
    assert!(settings.planes(None).is_err());
    assert_eq!(view.generation, idle);
    view.suspend();
    assert!(view.front.is_none());
    assert!(view.pending.is_none());
}
#[test]
fn physical_preview_apply_and_undo_keep_exact_region_owners_in_source_plane_images() {
    use crate::labels::RegionMap;
    use orv_core::labels::{BrushAction, BrushShape, Labels, PhysicalBrush};
    let before = [MemoryBudget::gpu().used(), MemoryBudget::volumes().used()];
    let device;
    {
        let source = source();
        device = source.device().clone();
        let mut desired = scene(source.clone());
        let base = Arc::new(Labels::empty(source.volume().clone()).unwrap());
        desired.selection = Some(RegionSelection::new(RegionMap::Manual(base.clone()), 7).unwrap());
        let mut view = View::new(
            source.clone(),
            wgpu::TextureFormat::Rgba8UnormSrgb,
            egui::Context::default(),
        )
        .unwrap();
        finish(&mut view, &desired);
        let initial = view.front.as_ref().unwrap().image.clone();
        assert_eq!(
            initial
                .scene()
                .surface
                .as_ref()
                .unwrap()
                .image
                .request()
                .surface
                .surface()
                .face_count(),
            0
        );
        let brush = PhysicalBrush {
            label: 7,
            action: BrushAction::Add,
            radius_mm: 2.,
            shape: BrushShape::Sphere,
            clip: None,
            protected_ids: [false; 256],
        };
        let point = source.volume().center();
        let proposal = base
            .propose_physical_stroke(&brush, &[point], || false)
            .unwrap();
        let preview = proposal.preview().clone();
        let engine = view.engine.clone();
        let blocked = engine.lock().unwrap();
        let mut moved = desired.clone();
        moved.state.camera.yaw_degrees += 3.;
        view.update(&moved);
        desired.selection =
            Some(RegionSelection::new(RegionMap::Manual(preview.clone()), 7).unwrap());
        view.update(&desired);
        assert!(
            view.front.is_none(),
            "A new map cannot inherit the old completed region"
        );
        drop(blocked);
        finish(&mut view, &desired);
        let preview_image = view.front.as_ref().unwrap().image.clone();
        let surface = preview_image
            .scene()
            .surface
            .as_ref()
            .unwrap()
            .image
            .request()
            .surface
            .surface();
        assert!(surface.face_count() > 0);
        assert_eq!(surface.revision(), preview.revision());
        let RegionMap::Manual(owner) = surface.owner() else {
            panic!("Expected the exact manual preview");
        };
        assert!(Arc::ptr_eq(owner, &preview));
        let applied = proposal.apply(&base, &brush, &[point], || false).unwrap();
        assert!(Arc::ptr_eq(&applied, &preview));
        desired.selection = Some(RegionSelection::new(RegionMap::Manual(applied), 7).unwrap());
        let generation = view.generation;
        view.update(&desired);
        assert_eq!(view.generation, generation);
        desired.selection = Some(RegionSelection::new(RegionMap::Manual(base.clone()), 7).unwrap());
        view.update(&desired);
        assert!(view.front.is_none());
        finish(&mut view, &desired);
        let undo = &view.front.as_ref().unwrap().image;
        let surface = undo
            .scene()
            .surface
            .as_ref()
            .unwrap()
            .image
            .request()
            .surface
            .surface();
        assert_eq!(surface.face_count(), 0);
        let RegionMap::Manual(owner) = surface.owner() else {
            panic!("Expected original manual map");
        };
        assert!(Arc::ptr_eq(owner, &base));
        assert!(
            preview_image
                .scene()
                .surface
                .as_ref()
                .unwrap()
                .image
                .request()
                .surface
                .surface()
                .face_count()
                > 0
        );
        let weak = Arc::downgrade(&view.engine);
        view.suspend();
        drop(view);
        drop(engine);
        until(|| weak.upgrade().is_none());
        assert!(Arc::ptr_eq(preview_image.source(), &source));
    }
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(Duration::from_secs(30)),
        })
        .unwrap();
    assert_eq!(
        before,
        [MemoryBudget::gpu().used(), MemoryBudget::volumes().used()]
    );
}
