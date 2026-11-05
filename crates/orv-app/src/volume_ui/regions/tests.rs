// SPDX-License-Identifier: GPL-3.0-or-later
#[cfg(feature = "gpu-tests")]
use super::*;

#[cfg(feature = "mcp")]
impl super::super::VolumeSession {
    pub(crate) fn enable_test_region(&mut self) {
        self.regions.settings.enabled = true;
    }
}

#[cfg(feature = "gpu-tests")]
mod gpu {
    use super::*;
    use crate::labels::{RegionMap, RegionSelection};
    use orv_core::{
        labels::Labels,
        segments::Segments,
        study::{Geometry, demo_study},
        volume_material::{MaterialPoint, VolumeMaterial},
        volume_view::Projection,
    };
    use orv_render::{mask_input::MaskInput, material_volume::Lighting};

    fn source() -> Arc<GpuVolumeSource> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: orv_render::configured_backends().unwrap(),
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = pollster::block_on(instance.request_adapter(&Default::default()))
            .expect("Actual GPU required for native region tests");
        let (device, queue) =
            pollster::block_on(adapter.request_device(&orv_render::device_descriptor(&adapter)))
                .unwrap();
        let frame = demo_study().series.remove(0).frames.remove(0);
        let volume = Arc::new(
            Volume::new(
                (0..4)
                    .map(|z| {
                        let mut frame = (*frame).clone();
                        frame.width = 8;
                        frame.height = 8;
                        frame.pixels = vec![20.; 64].into();
                        frame.geometry = Some(Geometry {
                            position: [0., 0., f64::from(z)],
                            orientation: [1., 0., 0., 0., 1., 0.],
                            spacing: [1., 1.],
                            frame_of_reference: Some("generated native region grid".into()),
                            nifti_transform: None,
                        });
                        Arc::new(frame)
                    })
                    .collect(),
            )
            .unwrap(),
        );
        GpuVolumeSource::new(&device, &queue, volume, || false).unwrap()
    }
    fn key(map: RegionMap, number: u16) -> Key {
        let mut state = VolumeView::fit(&map.source());
        state.projection = Projection::Composite;
        Key {
            selection: RegionSelection::new(map, number).unwrap(),
            display: ViewState::default(),
            state,
            style: OpticalStyle {
                material: VolumeMaterial::new(&[0., 100.].map(|value| MaterialPoint {
                    value,
                    luminance: 0.,
                    extinction_per_mm: 0.,
                }))
                .unwrap(),
                lighting: Lighting::UNLIT,
            },
            luminance: 0.8,
            interactive: [48, 32],
            refined: [96, 64],
        }
    }
    fn request(source: &Arc<GpuVolumeSource>, key: Key) -> Request {
        Request {
            engine: Arc::new(Mutex::new(None)),
            source: source.clone(),
            format: wgpu::TextureFormat::Rgba8Unorm,
            data_cancel: Arc::new(AtomicBool::new(false)),
            size: key.interactive,
            key,
            refined: false,
        }
    }
    fn until(mut condition: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !condition() {
            assert!(Instant::now() < deadline, "Native region worker timed out");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn native_worker_reuses_geometry_for_style_and_keeps_immutable_edit_and_undo_frames() {
        let source = source();
        let mut labels = Labels::empty(source.volume().clone()).unwrap();
        labels.threshold(None, [0., 100.], 7, || false).unwrap();
        let labels = Arc::new(labels);
        let original = key(RegionMap::Manual(labels.clone()), 7);
        let mut input = request(&source, original.clone());
        let make = |input: &Request| Request {
            engine: input.engine.clone(),
            source: input.source.clone(),
            format: input.format,
            data_cancel: input.data_cancel.clone(),
            key: input.key.clone(),
            size: input.size,
            refined: input.refined,
        };
        let first = worker::run(make(&input), &|| false).unwrap();
        assert!(first.key.same(&original));
        assert_eq!(
            first.image.texture().format(),
            wgpu::TextureFormat::Rgba16Float
        );
        input.key.luminance = 0.25;
        let styled = worker::run(make(&input), &|| false).unwrap();
        assert!(Arc::ptr_eq(
            &first.image.scene().mask,
            &styled.image.scene().mask
        ));
        assert_eq!(first.image.scene().mask_luminance, 0.8);
        assert_eq!(styled.image.scene().mask_luminance, 0.25);
        let mut edit = labels.try_clone(|| false).unwrap();
        edit.paint(1, &[[4., 4.]], 1, 0, || false).unwrap();
        input.key.selection = RegionSelection::new(RegionMap::Manual(Arc::new(edit)), 7).unwrap();
        let edited = worker::run(make(&input), &|| false).unwrap();
        assert_ne!(
            first.image.scene().mask.revision(),
            edited.image.scene().mask.revision()
        );
        let MaskInput::Manual(old) = &first.image.scene().mask.request().input else {
            panic!()
        };
        assert_eq!(old.labels().value([4, 4, 1]), Some(7));
        let MaskInput::Manual(new) = &edited.image.scene().mask.request().input else {
            panic!()
        };
        assert_eq!(new.labels().value([4, 4, 1]), Some(0));
        input.key = original;
        input.refined = true;
        input.size = input.key.refined;
        let undo = worker::run(make(&input), &|| false).unwrap();
        assert!(undo.refined);
        assert_eq!(
            undo.image.scene().mask.revision(),
            first.image.scene().mask.revision()
        );
        assert_eq!(undo.image.scene().mask.request().size, [96, 64]);
        input.key.selection = RegionSelection::new(
            RegionMap::Manual(Arc::new(
                Labels::empty(Arc::new(
                    Volume::new(source.volume().frames().to_vec()).unwrap(),
                ))
                .unwrap(),
            )),
            7,
        )
        .unwrap();
        assert!(worker::run(make(&input), &|| false).is_err());
        source.wait_submitted(|| false).unwrap();
    }

    #[test]
    fn native_supplied_worker_preserves_large_ids_and_sparse_coverage() {
        let source = source();
        let masks = Arc::new(
            Segments::from_binary(
                source.volume().clone(),
                &[50000],
                &[(50000, 1)],
                &[255; 8],
                None,
                || false,
            )
            .unwrap(),
        );
        let output = worker::run(
            request(&source, key(RegionMap::Supplied(masks.clone()), 50000)),
            &|| false,
        )
        .unwrap();
        let mask = &output.image.scene().mask;
        assert_eq!(mask.key().number, 50000);
        let MaskInput::Supplied(gpu) = &mask.request().input else {
            panic!()
        };
        assert!(Arc::ptr_eq(gpu.segments(), &masks));
        assert_eq!(gpu.number(), 50000);
        assert!(masks.mask(50000).unwrap().value([0, 0, 0]).is_none());
        assert_eq!(masks.mask(50000).unwrap().value([0, 0, 1]), Some(true));
        source.wait_submitted(|| false).unwrap();
    }

    #[test]
    fn view_coalesces_motion_refines_once_and_suspends_without_losing_its_data_cache() {
        let source = source();
        let labels = Arc::new(Labels::empty(source.volume().clone()).unwrap());
        let mut latest = key(RegionMap::Manual(labels), 1);
        let ctx = egui::Context::default();
        let mut view =
            View::new(source.clone(), wgpu::TextureFormat::Rgba8Unorm, ctx.clone()).unwrap();
        let engine = view.engine.clone();
        let blocked = engine.lock().unwrap();
        view.update(latest.clone(), true, &ctx);
        let first_generation = view.generation;
        let lease = view.data_cancel.clone();
        for yaw in 1..50 {
            latest.state.camera.yaw_degrees = f64::from(yaw);
            view.update(latest.clone(), true, &ctx);
        }
        assert!(Arc::ptr_eq(&lease, &view.data_cancel));
        assert!(!lease.load(Ordering::Acquire));
        assert_eq!(
            view.generation, first_generation,
            "Motion must let one running preview finish"
        );
        drop(blocked);
        until(|| {
            view.update(latest.clone(), true, &ctx);
            view.front.as_ref().is_some_and(|v| v.key.same(&latest))
        });
        assert!(view.error.is_none(), "{:?}", view.error);
        view.idle_after = Instant::now();
        until(|| {
            view.update(latest.clone(), false, &ctx);
            view.front.as_ref().is_some_and(|v| v.refined)
        });
        let generation = view.generation;
        for _ in 0..10 {
            view.update(latest.clone(), false, &ctx);
        }
        assert_eq!(
            view.generation, generation,
            "A quiet completed view must not resubmit"
        );
        let accepted = view.front.as_ref().unwrap().image.clone();
        latest.luminance = 2.;
        until(|| {
            view.update(latest.clone(), false, &ctx);
            view.error.is_some()
        });
        assert!(Arc::ptr_eq(&accepted, &view.front.as_ref().unwrap().image));
        let failed_generation = view.generation;
        for _ in 0..10 {
            view.update(latest.clone(), false, &ctx);
        }
        assert_eq!(
            view.generation, failed_generation,
            "A refusal must wait for retry or changed input"
        );
        latest.luminance = 0.8;
        latest.selection = RegionSelection::new(latest.selection.map.clone(), 2).unwrap();
        view.update(latest.clone(), false, &ctx);
        assert!(view.front.is_none(), "An old ID must disappear immediately");
        assert!(
            Arc::ptr_eq(&lease, &view.data_cancel),
            "Manual IDs share one uploaded grid"
        );
        until(|| {
            view.update(latest.clone(), false, &ctx);
            view.front.as_ref().is_some_and(|v| v.key.same(&latest))
        });
        let RegionMap::Manual(labels) = &latest.selection.map else {
            panic!()
        };
        let mut edit = labels.try_clone(|| false).unwrap();
        edit.paint(1, &[[4., 4.]], 1, 2, || false).unwrap();
        latest.selection = RegionSelection::new(RegionMap::Manual(Arc::new(edit)), 2).unwrap();
        view.update(latest.clone(), false, &ctx);
        assert!(
            view.front.is_none(),
            "An old revision must disappear immediately"
        );
        assert!(lease.load(Ordering::Acquire));
        let lease = view.data_cancel.clone();
        until(|| {
            view.update(latest.clone(), false, &ctx);
            view.front.as_ref().is_some_and(|v| v.key.same(&latest))
        });
        latest.state.quality = Quality::Interactive;
        until(|| {
            view.update(latest.clone(), false, &ctx);
            view.front.as_ref().is_some_and(|v| v.key.same(&latest))
        });
        view.idle_after = Instant::now();
        let generation = view.generation;
        for _ in 0..10 {
            view.update(latest.clone(), false, &ctx);
        }
        assert_eq!(view.generation, generation);
        assert!(
            !view.front.as_ref().unwrap().refined,
            "Explicit preview quality must not refine"
        );
        let retained_engine = view.engine.clone();
        view.suspend();
        assert!(view.front.is_none());
        assert!(view.pending.is_none());
        assert!(lease.load(Ordering::Acquire));
        assert!(Arc::ptr_eq(&retained_engine, &view.engine));
        view.update(latest.clone(), false, &ctx);
        assert!(!Arc::ptr_eq(&lease, &view.data_cancel));
        let drop_lease = view.data_cancel.clone();
        let engine_weak = Arc::downgrade(&view.engine);
        drop(retained_engine);
        drop(engine);
        drop(view);
        assert!(drop_lease.load(Ordering::Acquire));
        until(|| engine_weak.upgrade().is_none());
        source.wait_submitted(|| false).unwrap();
    }
}
