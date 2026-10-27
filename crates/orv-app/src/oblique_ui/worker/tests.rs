// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use orv_core::{WindowLevel, study::Geometry};
use std::time::{Duration, Instant};

fn frames() -> Vec<Arc<Frame>> {
    (0..8)
        .map(|z| {
            Arc::new(Frame {
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
            assert_eq!(snapshot.image.slab, Some(slab.preview()));
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
