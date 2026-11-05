// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use orv_core::{WindowLevel, study::Geometry};
use std::time::{Duration, Instant};

pub(crate) fn frames() -> Vec<Arc<Frame>> {
    (0..4)
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
                width: 4,
                height: 4,
                pixels: (0..16)
                    .map(|p| (p + z * 16) as f32)
                    .collect::<Vec<_>>()
                    .into(),
                geometry: Some(Geometry {
                    nifti_transform: None,
                    frame_of_reference: Some("synthetic-worker".into()),
                    position: [0., 0., z as f64 * 3.],
                    orientation: [1., 0., 0., 0., 1., 0.],
                    spacing: [2., 1.],
                }),
                instance: z,
                frame_index: 0,
                monochrome1: false,
                default_window: WindowLevel::SOFT_TISSUE,
            })
        })
        .collect()
}
fn wait(worker: &MprWorker) -> Result<MprSnapshot, String> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(result) = worker.take() {
            return result;
        }
        assert!(
            Instant::now() < deadline,
            "MPR worker did not publish a result"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}
#[test]
fn rapid_crosshair_requests_publish_the_latest_consistent_three_plane_set() {
    let worker = MprWorker::start(frames(), egui::Context::default());
    let first = wait(&worker).unwrap();
    assert_eq!(first.generation, 0);
    for i in 0..100 {
        worker.request([1., 2., i as f64 / 99. * 9.], 512);
    }
    let final_set = wait(&worker).unwrap();
    assert_eq!(final_set.generation, 100);
    assert_eq!(final_set.point, [1., 2., 9.]);
    for (i, axis) in [2, 1, 0].into_iter().enumerate() {
        assert_eq!(
            final_set.images[i].geometry.as_ref().unwrap().position[axis],
            final_set.point[axis]
        );
    }
}
#[test]
fn unavailable_geometry_returns_an_error_instead_of_a_fabricated_view() {
    let worker = MprWorker::start(vec![], egui::Context::default());
    assert!(matches!(wait(&worker),Err(message) if message.contains("at least two")));
}

#[test]
fn closing_a_session_wakes_the_worker_and_releases_its_volume() {
    for _ in 0..20 {
        let worker = MprWorker::start(frames(), egui::Context::default());
        let snapshot = wait(&worker).unwrap();
        let volume = Arc::downgrade(&snapshot.volume);
        drop(snapshot);
        drop(worker);
        let deadline = Instant::now() + Duration::from_secs(5);
        while volume.upgrade().is_some() {
            assert!(
                Instant::now() < deadline,
                "Closed session still retains its volume"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}

#[test]
fn continuous_input_publishes_previews_then_refines_the_latest_point() {
    let worker = MprWorker::start(frames(), egui::Context::default());
    wait(&worker).unwrap();
    let mut previews = 0;
    for i in 0..30 {
        worker.request_interactive([1., 2., i as f64 / 29. * 9.], 512);
        std::thread::sleep(Duration::from_millis(2));
        if let Some(Ok(snapshot)) = worker.take() {
            assert!(
                snapshot.preview,
                "full refinement must wait for quiet input"
            );
            previews += 1;
            assert_eq!(
                snapshot.reformat_side, 512,
                "Moving thin planes must not drop resolution"
            );
            for (plane, axis) in [2, 1, 0].into_iter().enumerate() {
                assert_eq!(
                    snapshot.images[plane].geometry.as_ref().unwrap().position[axis],
                    snapshot.point[axis]
                );
            }
        }
    }
    assert!(previews > 0, "input must not starve completed previews");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let snapshot = wait(&worker).unwrap();
        if !snapshot.preview {
            assert_eq!(snapshot.point, [1., 2., 9.]);
            assert_eq!(snapshot.generation, 30);
            break;
        }
        assert!(Instant::now() < deadline, "latest view never refined");
    }
}

#[test]
fn moving_one_slider_preserves_full_detail_and_reuses_unchanged_planes() {
    let source = frames()
        .into_iter()
        .enumerate()
        .map(|(z, frame)| {
            let mut frame = (*frame).clone();
            frame.width = 257;
            frame.height = 129;
            frame.geometry.as_mut().unwrap().spacing = [1., 1.];
            frame.pixels = (0..129)
                .flat_map(|_| (0..257).map(move |x| (2000 * z + 100 * (x % 2)) as f32))
                .collect::<Vec<_>>()
                .into();
            Arc::new(frame)
        })
        .collect::<Vec<_>>();
    let worker = MprWorker::start(source, egui::Context::default());
    let first = wait(&worker).unwrap();
    worker.request_interactive([128., 64., 6.], 512);
    let moving = wait(&worker).unwrap();
    assert!(moving.preview);
    assert_eq!(moving.reformat_side, 512);
    assert_eq!(
        [moving.images[0].width, moving.images[0].height],
        [257, 129]
    );
    for (index, value) in moving.images[0].pixels.iter().enumerate() {
        assert_eq!(*value, 4000. + 100. * ((index % 257) % 2) as f32);
    }
    assert!(!Arc::ptr_eq(&first.images[0], &moving.images[0]));
    for index in [1, 2] {
        assert!(
            Arc::ptr_eq(&first.images[index], &moving.images[index]),
            "An unchanged plane must retain its image and GPU upload identity"
        );
    }
    let settled = wait(&worker).unwrap();
    assert!(!settled.preview);
    assert_eq!(settled.point, [128., 64., 6.]);
    for index in 0..3 {
        assert!(
            Arc::ptr_eq(&moving.images[index], &settled.images[index]),
            "Settling a thin view must not replace its pixels with a different resolution"
        );
    }
}

#[test]
fn thin_plane_cache_obeys_requested_sampling_changes() {
    let worker = MprWorker::start(frames(), egui::Context::default());
    let first = wait(&worker).unwrap();
    worker.request(first.point, 128);
    let reduced = wait(&worker).unwrap();
    assert_eq!(reduced.reformat_side, 128);
    for index in 0..3 {
        assert!(!Arc::ptr_eq(&first.images[index], &reduced.images[index]));
    }
    worker.request(first.point, 512);
    let restored = wait(&worker).unwrap();
    assert_eq!(restored.reformat_side, 512);
    for index in 0..3 {
        assert!(!Arc::ptr_eq(
            &reduced.images[index],
            &restored.images[index]
        ));
    }
}

#[test]
fn completed_preview_can_be_drawn_while_newer_input_is_queued_but_stale_full_results_are_discarded()
{
    let worker = MprWorker::start(frames(), egui::Context::default());
    let mut snapshot = wait(&worker).unwrap();
    worker.shared.generation.store(2, Ordering::Relaxed);
    snapshot.preview = true;
    lock(&worker.shared.work).result = Some(Ok(snapshot.clone()));
    assert!(worker.take().unwrap().unwrap().preview);
    snapshot.preview = false;
    lock(&worker.shared.work).result = Some(Ok(snapshot));
    assert!(worker.take().is_none());
}

#[test]
fn slab_worker_publishes_coherent_bounded_preview_then_exact_requested_sampling() {
    use orv_core::slab::*;
    let worker = MprWorker::start(frames(), egui::Context::default());
    wait(&worker).unwrap();
    let slab = Slab {
        projection: SlabProjection::Minimum,
        thickness_mm: 4.,
        intervals: 40,
    };
    worker.request_slab([1., 2., 4.5], 512, Some(slab));
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut preview = false;
    loop {
        let snapshot = wait(&worker).unwrap();
        assert_eq!(snapshot.point, [1., 2., 4.5]);
        for image in &snapshot.images {
            assert_eq!(
                image.slab,
                Some(if snapshot.preview {
                    slab.preview()
                } else {
                    slab
                })
            );
            assert!(
                image.width <= snapshot.reformat_side && image.height <= snapshot.reformat_side
            );
        }
        if snapshot.preview {
            preview = true;
        } else {
            assert!(preview);
            break;
        }
        assert!(Instant::now() < deadline);
    }
    assert!(
        worker.take().is_none(),
        "Idle worker must not keep producing frames"
    );
}
