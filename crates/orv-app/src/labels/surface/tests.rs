// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use eframe::egui_wgpu::wgpu;
use orv_core::{
    ViewState,
    labels::{BrushAction, BrushShape},
    mask::MaskOwner,
    study::{Geometry, demo_study},
    surface::{CellSurface, SurfaceLimits},
    volume_material::{MaterialPoint, VolumeMaterial},
    volume_view::{Projection, Quality, VolumeView},
};
use orv_render::{
    material_volume::Lighting,
    section_volume::pick::SectionPickFrame,
    section_volume::{SectionRenderer, SectionScene, SectionSurface},
    surface_pick::PickScope,
    surface_projection::{GpuSurfaceProjector, SurfaceProjection},
    surface_source::GpuCellSurface,
    volume_source::GpuVolumeSource,
};

struct Fixture {
    base: Arc<Labels>,
    source: Arc<Volume>,
    frame: SectionPickFrame,
    current: SectionPickState,
    renderer: SectionRenderer,
    brush: PhysicalBrush,
}
impl Fixture {
    fn new() -> Self {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: orv_render::configured_backends().unwrap(),
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = pollster::block_on(instance.request_adapter(&Default::default()))
            .expect("Actual GPU required for native correction owner tests");
        let (device, queue) =
            pollster::block_on(adapter.request_device(&orv_render::device_descriptor(&adapter)))
                .unwrap();
        let template = demo_study().series.remove(0).frames.remove(0);
        let source = Arc::new(
            Volume::new(
                (0..9)
                    .map(|z| {
                        let mut frame = (*template).clone();
                        frame.width = 31;
                        frame.height = 23;
                        frame.pixels = vec![10.; 31 * 23].into();
                        frame.scalar = None;
                        frame.rgba = None;
                        frame.validity = None;
                        frame.geometry = Some(Geometry {
                            position: [0., 0., f64::from(z)],
                            orientation: [1., 0., 0., 0., 1., 0.],
                            spacing: [1.; 2],
                            frame_of_reference: Some("generated correction owner".into()),
                            nifti_transform: None,
                        });
                        Arc::new(frame)
                    })
                    .collect(),
            )
            .unwrap(),
        );
        let gpu = GpuVolumeSource::new(&device, &queue, source.clone(), || false).unwrap();
        let mut brush = PhysicalBrush {
            label: 7,
            action: BrushAction::Add,
            radius_mm: 4.,
            shape: BrushShape::Sphere,
            clip: None,
            protected_ids: [false; 256],
        };
        let empty = Arc::new(Labels::empty(source.clone()).unwrap());
        let proposal = empty
            .propose_physical_stroke(&brush, &[source.center()], || false)
            .unwrap();
        let base = proposal.preview().clone();
        let geometry = CellSurface::build(
            MaskOwner::Manual(base.clone()),
            7,
            None,
            SurfaceLimits::default(),
            || false,
        )
        .unwrap()
        .0;
        let surface = GpuCellSurface::upload(gpu.clone(), geometry, None, || false)
            .unwrap()
            .0;
        let volume = VolumeView {
            projection: Projection::Composite,
            quality: Quality::Refined,
            ..VolumeView::fit(&source)
        };
        let navigation = ViewState::default();
        let size = [65, 49];
        let projected = GpuSurfaceProjector::new(gpu.clone())
            .unwrap()
            .render(
                SurfaceProjection {
                    surface,
                    camera: volume.camera.clone(),
                    display: navigation.clone(),
                    clip: None,
                    size,
                },
                || false,
            )
            .unwrap();
        let scene = SectionScene {
            surface: Some(SectionSurface {
                image: projected,
                luminance: 0.8,
                lighting: Lighting::HEADLIGHT,
            }),
            planes: [None, None, None],
            navigation,
            volume,
            material: Arc::new(
                VolumeMaterial::new(&[-1000., 1000.].map(|value| MaterialPoint {
                    value,
                    luminance: 0.3,
                    extinction_per_mm: 0.,
                }))
                .unwrap(),
            ),
            lighting: Lighting::UNLIT,
            size,
        };
        let mut renderer = SectionRenderer::new(gpu, wgpu::TextureFormat::Rgba8UnormSrgb).unwrap();
        let image = renderer
            .render_rows(&scene, 0, 0..size[1], || false)
            .unwrap()
            .unwrap();
        let frame = SectionPickFrame::new(image, PickScope::new().unwrap());
        let current = SectionPickState::from_frame(&frame).unwrap();
        brush.action = BrushAction::Erase;
        brush.radius_mm = 1.5;
        Self {
            base,
            source,
            frame,
            current,
            renderer,
            brush,
        }
    }
    fn owner(&self) -> LabelUi {
        LabelUi {
            current: Some(self.base.clone()),
            label: 7,
            series: 2,
            ..Default::default()
        }
    }
    fn proposal(&mut self) -> SectionFaceErase {
        self.renderer
            .propose_erase_stamp(&self.frame, &self.current, [32, 24], &self.brush, || false)
            .unwrap()
            .0
    }
}

#[test]
fn surface_apply_commits_exact_prepared_owner_once_and_undo_redo_preserve_revisions() {
    let mut f = Fixture::new();
    let mut owner = f.owner();
    let p = f.proposal();
    assert!(p.summary().changed_voxels > 0);
    let expected = p.proposal().preview().clone();
    owner
        .apply_surface_erase(p, &f.current, &f.brush, 2, &f.source)
        .unwrap();
    assert!(Arc::ptr_eq(owner.current.as_ref().unwrap(), &expected));
    assert_eq!(owner.undo.len(), 1);
    assert!(owner.redo.is_empty());
    assert!(owner.dirty);
    owner.undo_edit();
    assert!(Arc::ptr_eq(owner.current.as_ref().unwrap(), &f.base));
    assert!(owner.undo.is_empty());
    assert_eq!(owner.redo.len(), 1);
    owner.redo_edit();
    assert!(Arc::ptr_eq(owner.current.as_ref().unwrap(), &expected));
    assert_eq!(owner.undo.len(), 1);
    assert!(owner.redo.is_empty());
    let late = f.proposal();
    assert!(
        owner
            .apply_surface_erase(late, &f.current, &f.brush, 2, &f.source)
            .is_err()
    );
    assert_eq!(owner.undo.len(), 1);
    assert!(Arc::ptr_eq(owner.current.as_ref().unwrap(), &expected));
}

#[test]
fn surface_owner_rejects_foreign_map_source_series_id_tools_and_stale_view_before_history() {
    let mut f = Fixture::new();
    for change in 0..11 {
        let mut owner = f.owner();
        match change {
            0 => owner.current = Some(Arc::new(f.base.try_clone(|| false).unwrap())),
            1 => owner.series = 3,
            2 => owner.label = 8,
            3 => owner.open = true,
            4 => owner.paint = true,
            5 => owner.physical.open = true,
            6 => owner.regions.picking = true,
            7 => owner.supplied_active = true,
            8 => owner.current = None,
            9 => owner.display.visible_ids[7] = false,
            _ => {
                owner.task = Some(Task::start(
                    Operation::StartVolume(f.source.clone()),
                    egui::Context::default(),
                ))
            }
        }
        let prior = owner.current.clone();
        let p = f.proposal();
        assert!(
            owner
                .apply_surface_erase(p, &f.current, &f.brush, 2, &f.source)
                .is_err(),
            "context {change}"
        );
        assert!(owner.undo.is_empty());
        assert!(owner.redo.is_empty());
        assert!(!owner.dirty);
        assert!(
            matches!((&prior, &owner.current), (None, None))
                || matches!((&prior,&owner.current),(Some(a),Some(b)) if Arc::ptr_eq(a,b))
        );
    }
    let mut owner = f.owner();
    let mut altered = f.current.clone();
    altered.scene.navigation.pan[0] += 0.1;
    let p = f.proposal();
    assert!(
        owner
            .apply_surface_erase(p, &altered, &f.brush, 2, &f.source)
            .is_err()
    );
    let p = f.proposal();
    assert!(
        owner
            .apply_surface_erase(p, &f.current, &f.brush, 3, &f.source)
            .is_err()
    );
    let other = Arc::new(Volume::new(f.source.frames().to_vec()).unwrap());
    let p = f.proposal();
    assert!(
        owner
            .apply_surface_erase(p, &f.current, &f.brush, 2, &other)
            .is_err()
    );
    let p = f.proposal();
    f.current.scope.revoke();
    assert!(
        owner
            .apply_surface_erase(p, &f.current, &f.brush, 2, &f.source)
            .is_err()
    );
    assert!(Arc::ptr_eq(owner.current.as_ref().unwrap(), &f.base));
    assert!(owner.undo.is_empty());
    assert!(owner.redo.is_empty());
    assert!(!owner.dirty);
}

#[test]
fn surface_noop_keeps_exact_map_saved_state_and_redo_history() {
    let mut f = Fixture::new();
    let mut owner = f.owner();
    owner.mark_saved();
    owner.redo.push_back(f.base.clone());
    let batch = f
        .renderer
        .probe_region(&f.frame, &f.current, [[0, 0], [1, 1]], || false)
        .unwrap();
    let p = SectionFaceErase::prepare(&f.current, &[batch], &f.brush, &[f.source.center()], || {
        false
    })
    .unwrap();
    assert_eq!(p.summary().changed_voxels, 0);
    owner
        .apply_surface_erase(p, &f.current, &f.brush, 2, &f.source)
        .unwrap();
    assert!(Arc::ptr_eq(owner.current.as_ref().unwrap(), &f.base));
    assert!(owner.undo.is_empty());
    assert_eq!(owner.redo.len(), 1);
    assert!(!owner.dirty);
}
