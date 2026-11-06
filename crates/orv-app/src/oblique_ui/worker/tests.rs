// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use orv_core::{WindowLevel, study::Geometry};
use std::time::{Duration, Instant};

fn frames() -> Vec<Arc<Frame>> {
    (0..8)
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
                pixels: vec![z as f32; 64].into(),
                geometry: Some(Geometry {
                    nifti_transform: None,
                    frame_of_reference: Some("synthetic-oblique-worker".into()),
                    position: [0., 0., f64::from(z) * 2.],
                    orientation: [1., 0., 0., 0., 1., 0.],
                    spacing: [1., 1.],
                }),
                instance: z,
                frame_index: 0,
                monochrome1: false,
                default_window: WindowLevel::SOFT_TISSUE,
            })
        })
        .collect()
}
fn wait(worker: &Worker) -> Result<Snapshot, String> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(result) = worker.take() {
            return result;
        }
        assert!(Instant::now() < deadline, "Oblique worker did not finish");
        std::thread::sleep(Duration::from_millis(2));
    }
}
#[test]
fn rapid_plane_requests_keep_only_latest_physical_geometry() {
    let worker = Worker::start(frames(), egui::Context::default());
    let first = wait(&worker).unwrap();
    assert_eq!(first.generation, 0);
    let mut target = first.plane.clone();
    for i in 0..100 {
        target = ObliquePlane::from_angles(first.volume.center(), [f64::from(i) * 0.5, 20., -30.])
            .unwrap();
        worker.request(target.clone(), 64);
    }
    let latest = wait(&worker).unwrap();
    assert_eq!(latest.generation, 100);
    assert_eq!(latest.plane, target);
    let geometry = latest.image.geometry.as_ref().unwrap();
    assert_eq!(geometry.orientation[..3], target.column_direction);
    assert_eq!(geometry.orientation[3..], target.row_direction);
    let normal = target.normal_direction();
    let offset: f64 = (0..3)
        .map(|i| (geometry.position[i] - target.center_mm[i]) * normal[i])
        .sum();
    assert!(offset.abs() < 1e-9);
}
#[test]
fn closing_idle_or_busy_worker_releases_source_and_does_not_lose_wakeup() {
    for busy in [false, true] {
        for _ in 0..10 {
            let worker = Worker::start(frames(), egui::Context::default());
            let snapshot = wait(&worker).unwrap();
            let weak = Arc::downgrade(&snapshot.volume);
            if busy {
                worker.request(
                    ObliquePlane::from_angles(snapshot.volume.center(), [32., -44., 18.]).unwrap(),
                    512,
                );
            }
            drop(snapshot);
            drop(worker);
            let deadline = Instant::now() + Duration::from_secs(3);
            while weak.upgrade().is_some() {
                assert!(
                    Instant::now() < deadline,
                    "Closed oblique worker retains private source data"
                );
                std::thread::sleep(Duration::from_millis(2));
            }
        }
    }
}
#[test]
fn missing_geometry_is_explicit_and_never_replaced_by_a_fake_plane() {
    let worker = Worker::start(vec![], egui::Context::default());
    assert!(wait(&worker).is_err());
}

#[test]
fn initialization_error_survives_later_view_requests() {
    let worker = Worker::start(vec![], egui::Context::default());
    let first = wait(&worker).err().unwrap();
    let plane = ObliquePlane::from_angles([0.; 3], [0.; 3]).unwrap();
    worker.request(plane, 64);
    assert_eq!(worker.take().unwrap().err().unwrap(), first);
}

#[test]
fn superseded_preview_errors_do_not_replace_a_newer_target() {
    let worker = Worker {
        shared: Arc::new(Shared::default()),
    };
    let ctx = egui::Context::default();
    let plane = ObliquePlane::from_angles([0.; 3], [0.; 3]).unwrap();
    worker.request(plane.clone(), 64);
    let old = worker.generation();
    worker.request(plane.clone(), 64);
    publish_preview(&worker.shared, old, Err(GeometryError::Limit), &ctx);
    assert!(worker.take().is_none());

    publish_preview(
        &worker.shared,
        worker.generation(),
        Err(GeometryError::Limit),
        &ctx,
    );
    assert!(matches!(worker.take(), Some(Err(_))));
    publish_preview(
        &worker.shared,
        worker.generation(),
        Err(GeometryError::Limit),
        &ctx,
    );
    worker.request(plane, 64);
    assert!(worker.take().is_none());
}

#[test]
fn interactive_slabs_refine_after_quiet_and_drop_busy_private_sources() {
    use orv_core::slab::*;
    let worker = Worker::start(frames(), egui::Context::default());
    let first = wait(&worker).unwrap();
    let weak = Arc::downgrade(&first.volume);
    let slab = Slab {
        projection: SlabProjection::Maximum,
        thickness_mm: 8.,
        intervals: 64,
    };
    worker.request_projection(first.plane.clone(), 512, Some(slab), true);
    let mut saw_preview = false;
    loop {
        let snapshot = wait(&worker).unwrap();
        if snapshot.preview {
            saw_preview = true;
            assert_eq!(snapshot.image.slab, Some(slab));
            assert_eq!(snapshot.reformat_side, 512);
        } else {
            assert_eq!(snapshot.image.slab, Some(slab));
            break;
        }
    }
    assert!(saw_preview);
    worker.request_projection(first.plane.clone(), 1024, Some(slab), true);
    drop(first);
    drop(worker);
    let deadline = Instant::now() + Duration::from_secs(3);
    while weak.upgrade().is_some() {
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
}

#[test]
fn gpu_interactive_oblique_keeps_full_detail_and_settles_to_canonical_cpu() {
    use orv_core::slab::{Slab, SlabProjection};
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: orv_render::configured_backends().unwrap(),
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapter = pollster::block_on(instance.request_adapter(&Default::default()))
        .expect("Actual GPU required");
    let (device, queue) =
        pollster::block_on(adapter.request_device(&orv_render::device_descriptor(&adapter)))
            .unwrap();
    let mut source = frames();
    for (z, frame) in source.iter_mut().enumerate() {
        let frame = Arc::make_mut(frame);
        frame.width = 65;
        frame.height = 49;
        frame.pixels = (0..65 * 49)
            .map(|i| ((i % 65) * 3 + (i / 65) * 5 + z * 7) as f32)
            .collect::<Vec<_>>()
            .into();
        frame.validity = Some(
            (0..65 * 49)
                .map(|i| u8::from(!(i + z).is_multiple_of(29)))
                .collect::<Vec<_>>()
                .into(),
        );
    }
    let worker = Worker::start(source, egui::Context::default());
    let initial = wait(&worker).unwrap();
    worker.offer_gpu(&device, &queue);
    let plane = ObliquePlane::from_angles(initial.volume.center(), [13., -18., 24.]).unwrap();
    let mut gpu_source = None;
    for slab in [
        None,
        Some(Slab {
            projection: SlabProjection::Maximum,
            thickness_mm: 6.,
            intervals: 64,
        }),
    ] {
        worker.request_projection(plane.clone(), 512, slab, true);
        let moving = wait(&worker).unwrap();
        assert!(moving.preview);
        assert_eq!(moving.reformat_side, 512);
        assert_eq!(moving.image.slab, slab);
        let gpu = moving.gpu_image.as_ref().expect("GPU preview required");
        gpu_source = Some(Arc::downgrade(gpu.source()));
        assert!(Arc::ptr_eq(gpu.frame(), &moving.image));
        assert!(Arc::ptr_eq(gpu.source().volume(), &moving.volume));
        let reference = if let Some(slab) = slab {
            moving
                .volume
                .reformat_slab(&plane, slab, 512, || false)
                .unwrap()
        } else {
            moving
                .volume
                .reformat_oblique(&plane, 512, || false)
                .unwrap()
        };
        assert_eq!(moving.image.geometry, reference.geometry);
        assert_eq!(
            [moving.image.width, moving.image.height],
            [reference.width, reference.height]
        );
        for (&a, &b) in moving.image.pixels.iter().zip(reference.pixels.iter()) {
            assert_eq!(a.is_finite(), b.is_finite());
            if b.is_finite() {
                assert!((a - b).abs() <= 0.0002 * b.abs(), "{a}/{b}");
            }
        }
        let settled = wait(&worker).unwrap();
        assert!(!settled.preview);
        assert!(settled.gpu_image.is_none());
        assert_eq!(settled.image.geometry, reference.geometry);
        assert!(
            settled
                .image
                .pixels
                .iter()
                .zip(reference.pixels.iter())
                .all(|(a, b)| a.to_bits() == b.to_bits())
        );
    }
    drop(worker);
    let until = Instant::now() + Duration::from_secs(5);
    let weak = gpu_source.unwrap();
    while weak.upgrade().is_some() {
        assert!(
            Instant::now() < until,
            "Closed oblique worker retained its GPU source"
        );
        std::thread::sleep(Duration::from_millis(2));
    }

    let mut subnormal = frames();
    for frame in &mut subnormal {
        Arc::make_mut(frame).pixels = vec![f32::MIN_POSITIVE / 2.; 64].into();
    }
    let worker = Worker::start(subnormal, egui::Context::default());
    let first = wait(&worker).unwrap();
    worker.offer_gpu(&device, &queue);
    worker.request_projection(first.plane.clone(), 512, None, true);
    let fallback = wait(&worker).unwrap();
    assert!(fallback.preview);
    assert!(fallback.gpu_image.is_none());
    let reference = fallback
        .volume
        .reformat_oblique(&fallback.plane, 512, || false)
        .unwrap();
    assert!(
        fallback
            .image
            .pixels
            .iter()
            .zip(reference.pixels.iter())
            .all(|(a, b)| a.to_bits() == b.to_bits())
    );
}
