// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use orv_core::{
    resources::MemoryBudget,
    slab::{Slab, SlabProjection},
};
use std::time::{Duration, Instant};

fn device() -> (wgpu::Device, wgpu::Queue) {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: orv_render::configured_backends().unwrap(),
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapter = pollster::block_on(instance.request_adapter(&Default::default()))
        .expect("Actual GPU required");
    pollster::block_on(adapter.request_device(&orv_render::device_descriptor(&adapter))).unwrap()
}
fn source(subnormal: bool) -> Vec<Arc<Frame>> {
    let template = tests::frames().remove(0);
    (0usize..16)
        .map(|z| {
            let mut f = (*template).clone();
            f.width = 257;
            f.height = 129;
            f.geometry.as_mut().unwrap().position = [0., 0., z as f64 * 3.];
            f.pixels = (0..257 * 129)
                .map(|i| {
                    if subnormal {
                        f32::MIN_POSITIVE / 2.
                    } else {
                        ((i % 257) * 13 + (i / 257) * 7 + z * 17) as f32 - 500.
                    }
                })
                .collect::<Vec<_>>()
                .into();
            f.validity = Some(
                (0..257 * 129)
                    .map(|i| u8::from(!(i + z).is_multiple_of(29)))
                    .collect::<Vec<_>>()
                    .into(),
            );
            Arc::new(f)
        })
        .collect()
}
fn settled(worker: &MprWorker, gpu: bool) -> MprSnapshot {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let snapshot = tests::wait(worker).unwrap();
        if !snapshot.preview
            && snapshot
                .gpu_images
                .iter()
                .all(|image| image.is_some() == gpu)
        {
            return snapshot;
        }
        assert!(
            Instant::now() < deadline,
            "Requested backend did not settle"
        );
    }
}
fn verify(snapshot: &MprSnapshot) {
    for (i, plane) in PLANES.into_iter().enumerate() {
        let expected = if let Some(slab) = snapshot.images[i].slab {
            snapshot
                .volume
                .reformat_slab(
                    &orv_core::oblique::ObliquePlane::anatomical(plane, snapshot.point).unwrap(),
                    slab,
                    snapshot.reformat_side,
                    || false,
                )
                .unwrap()
        } else {
            snapshot
                .volume
                .reformat(plane, snapshot.point, snapshot.reformat_side, || false)
                .unwrap()
        };
        let image = &snapshot.images[i];
        assert_eq!(image.geometry, expected.geometry);
        assert_eq!(
            [image.width, image.height],
            [expected.width, expected.height]
        );
        for (&a, &b) in image.pixels.iter().zip(expected.pixels.iter()) {
            assert_eq!(a.is_finite(), b.is_finite());
            if b.is_finite() {
                assert!((a - b).abs() <= 0.0002 * b.abs(), "{a}/{b}");
            }
        }
        if let Some(gpu) = &snapshot.gpu_images[i] {
            assert!(Arc::ptr_eq(gpu.frame(), image));
            assert!(Arc::ptr_eq(gpu.source().volume(), &snapshot.volume));
        }
    }
}
fn fence(device: &wgpu::Device) {
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(Duration::from_secs(10)),
        })
        .unwrap();
}

#[test]
fn changed_sampling_cancels_gpu_preview_and_the_next_view_still_matches_the_source() {
    use std::cell::Cell;

    let (device, queue) = device();
    let mut producer = MprWorker::start(source(false), egui::Context::default());
    let volume = tests::wait(&producer).unwrap().volume;
    let handle = producer.worker.take().unwrap();
    drop(producer);
    handle.join().unwrap();
    let worker = MprWorker {
        shared: Arc::new(Shared::default()),
        worker: None,
    };
    let mut gpu = Gpu::default();
    gpu.offer((device.clone(), queue.clone()));
    let mut cache = PlaneCache::default();
    let slab = Slab {
        projection: SlabProjection::Maximum,
        thickness_mm: 4.,
        intervals: 64,
    };
    worker.request_slab(volume.center(), 512, None);
    let warmup = lock(&worker.shared.work).latest.clone().unwrap();
    let first = render(&volume, &warmup, true, &mut cache, &mut gpu, || false).unwrap();
    assert!(first.gpu_images.iter().all(Option::is_some));
    let mut point = volume.center();
    for value in &mut point {
        *value += 0.25;
    }
    worker.request_slab(point, 512, Some(slab));
    let request = lock(&worker.shared.work).latest.clone().unwrap();
    let calls = Cell::new(0);
    let cancelled = render(&volume, &request, true, &mut cache, &mut gpu, || {
        calls.set(calls.get() + 1);
        if calls.get() == 8 {
            worker.request_slab(point, 512, None);
        }
        worker.shared.preview_cancelled(&request)
    });
    assert!(calls.get() >= 8);
    assert!(matches!(cancelled, Err(GeometryError::Cancelled)));
    let next = lock(&worker.shared.work).latest.clone().unwrap();
    let final_view = render(&volume, &next, true, &mut cache, &mut gpu, || false).unwrap();
    assert!(final_view.gpu_images.iter().all(Option::is_some));
    assert_eq!(final_view.point, point);
    assert!(final_view.images.iter().all(|image| image.slab.is_none()));
    verify(&final_view);
    drop((first, final_view, cache, gpu, volume, worker));
    fence(&device);
    assert_eq!(MemoryBudget::gpu().used(), 0);
}

#[test]
fn inactive_mpr_releases_gpu_source_and_stays_quiet_until_resumed_at_its_latest_point() {
    let (device, queue) = device();
    let mut worker = MprWorker::start(source(false), egui::Context::default());
    worker.offer_gpu(&device, &queue);
    let first = settled(&worker, true);
    let mut point = first.point;
    point[2] += 0.5;
    worker.request(point, 512);
    let moved = settled(&worker, true);
    let source = Arc::downgrade(moved.gpu_images[0].as_ref().unwrap().source());
    let volume = moved.volume.clone();
    worker.set_suspended(true);
    drop((first, moved));
    let until = Instant::now() + Duration::from_secs(10);
    while source.upgrade().is_some() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        source.upgrade().is_none(),
        "Inactive view retained its dense GPU source"
    );
    fence(&device);
    assert_eq!(MemoryBudget::gpu().used(), 0);
    worker.set_canonical(true);
    assert!(worker.take().is_none());
    worker.set_suspended(false);
    let canonical = settled(&worker, false);
    assert_eq!(canonical.point, point);
    assert!(Arc::ptr_eq(&volume, &canonical.volume));
    verify(&canonical);
    worker.set_canonical(false);
    let resumed = settled(&worker, true);
    assert_eq!(resumed.point, point);
    assert!(Arc::ptr_eq(&volume, &resumed.volume));
    verify(&resumed);
    let handle = worker.worker.take().unwrap();
    drop(worker);
    handle.join().unwrap();
    drop((canonical, resumed));
    fence(&device);
    assert_eq!(MemoryBudget::gpu().used(), 0);
}
#[test]
fn gpu_navigation_reuses_unchanged_planes_and_source_across_canonical_mode_and_releases_on_close() {
    let (device, queue) = device();
    let mut worker = MprWorker::start(source(false), egui::Context::default());
    drop(tests::wait(&worker).unwrap());
    worker.offer_gpu(&device, &queue);
    let first = settled(&worker, true);
    verify(&first);
    let owner = first.gpu_images[0].as_ref().unwrap().source().clone();
    let volume = Arc::downgrade(&first.volume);
    let mut point = first.point;
    point[2] += 1.1;
    worker.request_interactive(point, 512);
    let moving = tests::wait(&worker).unwrap();
    assert!(moving.preview);
    verify(&moving);
    assert_eq!(moving.reformat_side, 512);
    assert!(moving.images[0].width > 128);
    for i in [1, 2] {
        assert!(Arc::ptr_eq(&first.images[i], &moving.images[i]));
        assert!(Arc::ptr_eq(
            first.gpu_images[i].as_ref().unwrap(),
            moving.gpu_images[i].as_ref().unwrap()
        ));
    }
    let quiet = settled(&worker, true);
    for i in 0..3 {
        assert!(Arc::ptr_eq(&quiet.images[i], &moving.images[i]));
    }
    worker.set_canonical(true);
    let canonical = settled(&worker, false);
    assert_eq!(canonical.point, point);
    verify(&canonical);
    for (i, plane) in PLANES.into_iter().enumerate() {
        assert!(!Arc::ptr_eq(&canonical.images[i], &quiet.images[i]));
        let expected = canonical
            .volume
            .reformat(plane, point, 512, || false)
            .unwrap();
        assert_eq!(
            canonical.images[i]
                .pixels
                .iter()
                .map(|p| p.to_bits())
                .collect::<Vec<_>>(),
            expected
                .pixels
                .iter()
                .map(|p| p.to_bits())
                .collect::<Vec<_>>()
        );
    }
    worker.set_canonical(false);
    let restored = settled(&worker, true);
    assert_eq!(restored.point, point);
    assert!(Arc::ptr_eq(
        &owner,
        restored.gpu_images[0].as_ref().unwrap().source()
    ));
    verify(&restored);
    assert!(
        worker.take().is_none(),
        "Idle worker must not render continuously"
    );
    let handle = worker.worker.take().unwrap();
    drop(worker);
    handle.join().unwrap();
    drop((first, moving, quiet, canonical, restored, owner));
    fence(&device);
    assert!(volume.upgrade().is_none());
    assert_eq!(MemoryBudget::gpu().used(), 0);
}
#[test]
fn gpu_slab_navigation_keeps_full_sampling_and_subnormal_fallback_keeps_exact_values() {
    let (device, queue) = device();
    for subnormal in [false, true] {
        let mut worker = MprWorker::start(source(subnormal), egui::Context::default());
        drop(tests::wait(&worker).unwrap());
        worker.offer_gpu(&device, &queue);
        let first = settled(&worker, !subnormal);
        verify(&first);
        for (projection, intervals) in [(SlabProjection::Maximum, 64), (SlabProjection::Mean, 256)]
        {
            let slab = Slab {
                projection,
                thickness_mm: 4.,
                intervals,
            };
            let mut point = first.point;
            point[2] += 0.75;
            worker.request_slab(point, 512, Some(slab));
            let moving = tests::wait(&worker).unwrap();
            assert!(moving.preview);
            assert_eq!(moving.reformat_side, 512);
            assert!(moving.images[0].width > 128);
            assert!(moving.images.iter().all(|f| f.slab == Some(slab)));
            assert!(moving.gpu_images.iter().all(|i| i.is_some() != subnormal));
            verify(&moving);
            let quiet = settled(&worker, !subnormal);
            for i in 0..3 {
                assert!(Arc::ptr_eq(&quiet.images[i], &moving.images[i]));
            }
        }
        let handle = worker.worker.take().unwrap();
        drop(worker);
        handle.join().unwrap();
    }
    fence(&device);
    assert_eq!(MemoryBudget::gpu().used(), 0);
}
