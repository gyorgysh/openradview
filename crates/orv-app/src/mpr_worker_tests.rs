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
pub(super) fn wait(worker: &MprWorker) -> Result<MprSnapshot, String> {
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
fn backend_mode_changes_discard_old_previews_even_when_their_generation_is_current() {
    let mut producer = MprWorker::start(frames(), egui::Context::default());
    let mut snapshot = wait(&producer).unwrap();
    let handle = producer.worker.take().unwrap();
    drop(producer);
    handle.join().unwrap();
    let worker = MprWorker {
        shared: Arc::new(Shared::default()),
        worker: None,
    };
    worker.set_canonical(true);
    snapshot.preview = true;
    snapshot.generation = worker.generation();
    lock(&worker.shared.work).result = Some(Ok(snapshot.clone()));
    assert!(worker.take().is_none());
    snapshot.canonical_requested = true;
    lock(&worker.shared.work).result = Some(Ok(snapshot));
    assert!(worker.take().unwrap().unwrap().preview);
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
    worker.set_suspended(true);
    assert!(worker.take().is_none());
    worker.set_suspended(false);
    assert!(matches!(wait(&worker),Err(message) if message.contains("at least two")));
    worker.request([0.; 3], 64);
    assert!(matches!(worker.take(), Some(Err(message)) if message.contains("at least two")));
}

#[test]
fn superseded_preview_errors_do_not_hide_newer_requests() {
    let worker = MprWorker {
        shared: Arc::new(Shared::default()),
        worker: None,
    };
    let ctx = egui::Context::default();
    worker.request_interactive([0.; 3], 512);
    let old = lock(&worker.shared.work).latest.clone().unwrap();
    worker.request_interactive([1.; 3], 512);
    publish(&worker.shared, &old, true, Err(GeometryError::Limit), &ctx);
    assert!(worker.take().is_none());

    let current = lock(&worker.shared.work).latest.clone().unwrap();
    publish(
        &worker.shared,
        &current,
        true,
        Err(GeometryError::Limit),
        &ctx,
    );
    assert!(matches!(worker.take(), Some(Err(_))));
    publish(
        &worker.shared,
        &current,
        true,
        Err(GeometryError::Limit),
        &ctx,
    );
    worker.request_interactive([2.; 3], 512);
    assert!(worker.take().is_none());
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
fn slab_plane_geometry_is_stable_when_the_point_moves_in_plane() {
    use orv_core::{
        oblique::ObliquePlane,
        slab::{Slab, SlabProjection},
    };

    let source = frames()
        .into_iter()
        .enumerate()
        .map(|(z, frame)| {
            let mut frame = (*frame).clone();
            frame.geometry.as_mut().unwrap().position = [
                -394.2698709253274 + z as f64 * 0.13,
                69.71319873438182 + z as f64 * 0.07,
                -77.777 + z as f64 * 3.,
            ];
            frame.width = 257;
            frame.pixels = vec![z as f32; 257 * 4].into();
            frame.geometry.as_mut().unwrap().spacing = [0.88, 1.812433865858239];
            Arc::new(frame)
        })
        .collect::<Vec<_>>();
    let worker = MprWorker::start(source, egui::Context::default());
    let snapshot = wait(&worker).unwrap();
    let slab = Slab {
        projection: SlabProjection::Maximum,
        thickness_mm: 4.,
        intervals: 32,
    };
    for (index, plane) in PLANES.into_iter().enumerate() {
        let axis = [2, 1, 0][index];
        let point = snapshot.volume.center();
        let initial = snapshot
            .volume
            .oblique_grid(
                &ObliquePlane::anatomical(plane, point).unwrap(),
                512,
                Some(slab),
            )
            .unwrap();
        for n in 0..50 {
            let mut moved = point;
            let [min, max] = snapshot.volume.bounds();
            for i in 0..3 {
                if i != axis {
                    moved[i] = min[i] + (max[i] - min[i]) * (n as f64 + 1.) / 51.;
                }
            }
            let grid = snapshot
                .volume
                .oblique_grid(
                    &ObliquePlane::anatomical(plane, moved).unwrap(),
                    512,
                    Some(slab),
                )
                .unwrap();
            assert_eq!(initial.dimensions(), grid.dimensions(), "{plane:?} at {n}");
            assert_eq!(initial.geometry(), grid.geometry(), "{plane:?} at {n}");
        }
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
fn stale_preview_cannot_replace_a_different_projection_or_resolution() {
    use orv_core::slab::{Slab, SlabProjection};
    let mut producer = MprWorker::start(frames(), egui::Context::default());
    let mut snapshot = wait(&producer).unwrap();
    let handle = producer.worker.take().unwrap();
    drop(producer);
    handle.join().unwrap();
    let worker = MprWorker {
        shared: Arc::new(Shared::default()),
        worker: None,
    };
    snapshot.preview = true;
    let slab = Slab {
        projection: SlabProjection::Maximum,
        thickness_mm: 4.,
        intervals: 40,
    };
    for (side, projection) in [(512, Some(slab)), (256, None)] {
        worker.request_slab(snapshot.point, side, projection);
        lock(&worker.shared.work).result = Some(Ok(snapshot.clone()));
        assert!(worker.take().is_none(), "incompatible preview was accepted");
    }
    worker.request_slab([2., 3., 4.], 512, None);
    lock(&worker.shared.work).result = Some(Ok(snapshot.clone()));
    assert!(worker.take().unwrap().unwrap().preview);

    for image in &mut snapshot.images {
        Arc::make_mut(image).slab = Some(slab);
    }
    for projection in [
        None,
        Some(Slab {
            thickness_mm: 8.,
            ..slab
        }),
    ] {
        worker.request_slab(snapshot.point, 512, projection);
        lock(&worker.shared.work).result = Some(Ok(snapshot.clone()));
        assert!(worker.take().is_none(), "old slab preview was accepted");
    }
    worker.request_slab([2., 3., 4.], 512, Some(slab));
    lock(&worker.shared.work).result = Some(Ok(snapshot));
    assert!(worker.take().unwrap().unwrap().preview);
}

fn sampling_round_trip_rejects_old_preview(publish_before_change: bool) {
    use orv_core::slab::{Slab, SlabProjection};

    let mut producer = MprWorker::start(frames(), egui::Context::default());
    let mut snapshot = wait(&producer).unwrap();
    let handle = producer.worker.take().unwrap();
    drop(producer);
    handle.join().unwrap();
    let worker = MprWorker {
        shared: Arc::new(Shared::default()),
        worker: None,
    };
    let ctx = egui::Context::default();
    worker.request_slab(snapshot.point, 512, None);
    let old = lock(&worker.shared.work).latest.clone().unwrap();
    snapshot.preview = true;
    snapshot.generation = old.generation;
    if publish_before_change {
        publish(&worker.shared, &old, true, Ok(snapshot.clone()), &ctx);
    }
    let slab = Slab {
        projection: SlabProjection::Maximum,
        thickness_mm: 4.,
        intervals: 32,
    };
    worker.request_slab(snapshot.point, 512, Some(slab));
    worker.request_slab([1., 2., 6.], 512, None);
    if !publish_before_change {
        publish(&worker.shared, &old, true, Ok(snapshot.clone()), &ctx);
    }
    assert!(
        worker.take().is_none(),
        "a preview from an earlier sampling epoch survived the round trip"
    );
    let current = lock(&worker.shared.work).latest.clone().unwrap();
    snapshot.generation = current.generation;
    worker.request_slab([1., 2., 7.], 512, None);
    publish(&worker.shared, &current, true, Ok(snapshot), &ctx);
    assert!(
        worker.take().unwrap().unwrap().preview,
        "position-only movement must not starve a compatible preview"
    );
}

#[test]
fn sampling_round_trip_discards_an_already_queued_preview() {
    sampling_round_trip_rejects_old_preview(true);
}

#[test]
fn sampling_round_trip_rejects_a_late_preview() {
    sampling_round_trip_rejects_old_preview(false);
}

#[test]
fn preview_sampling_changes_cancel_work_without_starving_navigation() {
    use orv_core::slab::{Slab, SlabProjection};
    use std::cell::Cell;

    let mut producer = MprWorker::start(frames(), egui::Context::default());
    let volume = wait(&producer).unwrap().volume;
    let handle = producer.worker.take().unwrap();
    drop(producer);
    handle.join().unwrap();
    let worker = MprWorker {
        shared: Arc::new(Shared::default()),
        worker: None,
    };
    let slab = Slab {
        projection: SlabProjection::Maximum,
        thickness_mm: 4.,
        intervals: 40,
    };
    for change_sampling in [false, true] {
        worker.request_slab([1., 2., 3.], 512, Some(slab));
        let request = lock(&worker.shared.work).latest.clone().unwrap();
        let calls = Cell::new(0);
        let result = render(
            &volume,
            &request,
            true,
            &mut PlaneCache::default(),
            &mut Gpu::default(),
            || {
                calls.set(calls.get() + 1);
                if calls.get() == 12 {
                    if change_sampling {
                        worker.request_slab([1., 2., 6.], 512, None);
                        // Returning to the previous settings cannot revive old work.
                        worker.request_slab([1., 2., 6.], 512, Some(slab));
                    } else {
                        worker.request_slab([1., 2., 6.], 512, Some(slab));
                    }
                }
                worker.shared.preview_cancelled(&request)
            },
        );
        assert!(calls.get() >= 12);
        if change_sampling {
            assert!(matches!(result, Err(GeometryError::Cancelled)));
        } else {
            let snapshot = result.unwrap();
            assert_eq!(snapshot.point, [1., 2., 3.]);
            assert!(snapshot.images.iter().all(|image| image.slab == Some(slab)));
        }
    }
}

#[test]
fn slab_worker_keeps_every_requested_sample_and_resolution_during_navigation() {
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
            assert_eq!(image.slab, Some(slab));
            assert_eq!(snapshot.reformat_side, 512);
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
