// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use orv_core::{ViewState, scene::SceneState};
use orv_render::snapshot::{Snapshot, SnapshotManifest};

fn snapshot() -> Snapshot {
    Snapshot {
        png: vec![1, 2, 3],
        manifest: SnapshotManifest {
            renderer_version: 1,
            scene: SceneState {
                schema_version: 1,
                source_sha256: "a".repeat(64),
                series_index: 0,
                view: ViewState::default(),
                crosshair_mm: None,
                reformat_side: 512,
                output_size: [32, 16],
                reconstruction: None,
            },
            scene_sha256: "b".repeat(64),
            png_sha256: "c".repeat(64),
            image: None,
            volume: None,
            includes_ui_overlays: false,
        },
    }
}

#[test]
fn export_never_overwrites_and_removes_partial_outputs_on_cancel() {
    let parent = tempfile::tempdir().unwrap();
    let picture = snapshot();
    let first = worker::write_picture(parent.path(), &picture, || false).unwrap();
    let second = worker::write_picture(parent.path(), &picture, || false).unwrap();
    assert_ne!(first, second);
    assert_eq!(std::fs::read(first.join("image.png")).unwrap(), picture.png);
    let restored: SnapshotManifest =
        serde_json::from_slice(&std::fs::read(first.join("view.json")).unwrap()).unwrap();
    assert_eq!(restored, picture.manifest);
    assert!(!restored.includes_ui_overlays);
    let calls = std::cell::Cell::new(0);
    assert!(
        worker::write_picture(parent.path(), &picture, || {
            let n = calls.get();
            calls.set(n + 1);
            n >= 2
        })
        .is_err()
    );
    assert_eq!(std::fs::read_dir(parent.path()).unwrap().count(), 2);
    assert_eq!(std::fs::read(first.join("image.png")).unwrap(), picture.png);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&first).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(first.join("image.png"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}

#[test]
fn reformat_verification_accepts_reordered_source_and_rejects_stale_pixels() {
    let mut series = orv_core::study::demo_study().series.remove(0);
    series.frames = crate::mpr_worker::tests::frames();
    let volume = Volume::new(series.frames.clone()).unwrap();
    series.frames.reverse();
    worker::verify_volume(&series, &volume, || false).unwrap();
    let mut changed = (*series.frames[0]).clone();
    changed.pixels = vec![99.; 16].into();
    series.frames[0] = Arc::new(changed);
    assert!(worker::verify_volume(&series, &volume, || false).is_err());
    assert!(worker::verify_volume(&series, &volume, || true).is_err());
}

#[cfg(feature = "gpu-tests")]
#[test]
fn local_export_uses_real_gpu_display_parameters_and_preserves_provenance() {
    use eframe::egui_wgpu::wgpu;
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: orv_render::configured_backends().expect("Native backend selection"),
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapter = pollster::block_on(instance.request_adapter(&Default::default()))
        .expect("Real GPU required");
    let (device, queue) =
        pollster::block_on(adapter.request_device(&orv_render::device_descriptor(&adapter)))
            .unwrap();
    let mut study = orv_core::study::demo_study();
    let mut series = study.series.remove(0);
    series.frames = crate::mpr_worker::tests::frames();
    let series = Arc::new(series);
    let mut scene = snapshot().manifest.scene;
    scene.output_size = [64, 32];
    scene.view.slice = 1;
    scene.view.zoom = 1.4;
    scene.view.pan = [0.1, -0.2];
    scene.view.invert = true;
    let input = Input {
        series: series.clone(),
        scene: scene.clone(),
        pixels: Pixels::Image(series.frames[1].clone()),
        device: device.clone(),
        queue: queue.clone(),
    };
    let saved = worker::render(input, || false).unwrap();
    assert_eq!(saved.manifest.scene.view, scene.view);
    assert_eq!(saved.manifest.scene.output_size, [64, 32]);
    assert_eq!(
        saved.manifest.scene.source_sha256,
        orv_core::scene::series_fingerprint(&series, || false).unwrap()
    );
    let image = image::load_from_memory(&saved.png).unwrap();
    assert_eq!((image.width(), image.height()), (64, 32));
    assert_eq!(
        saved
            .manifest
            .image
            .as_ref()
            .unwrap()
            .geometry
            .as_ref()
            .unwrap()
            .position_mm,
        [0., 0., 3.]
    );
    let text = serde_json::to_string(&saved.manifest).unwrap();
    assert!(!text.contains("synthetic-worker")); // reference identifier stays local
    assert!(!saved.manifest.includes_ui_overlays);
    let volume = Arc::new(Volume::new(series.frames.clone()).unwrap());
    let mut derived_scene = scene.clone();
    derived_scene.view.slice = 0;
    derived_scene.view.plane = orv_core::Plane::Coronal;
    derived_scene.crosshair_mm = Some(volume.center());
    derived_scene.reformat_side = 64;
    let reformatted = volume
        .reformat(orv_core::Plane::Coronal, volume.center(), 64, || false)
        .unwrap();
    let input = Input {
        series: series.clone(),
        scene: derived_scene.clone(),
        pixels: Pixels::Reformat(reformatted, volume.clone()),
        device: device.clone(),
        queue: queue.clone(),
    };
    let mpr = worker::render(input, || false).unwrap();
    assert_eq!(mpr.manifest.scene.crosshair_mm, Some(volume.center()));
    let plane =
        orv_core::oblique::ObliquePlane::from_angles(volume.center(), [12., 8., 5.]).unwrap();
    derived_scene.schema_version = 2;
    derived_scene.view.plane = orv_core::Plane::Acquired;
    derived_scene.crosshair_mm = None;
    derived_scene.reconstruction = Some(orv_core::scene::Reconstruction::Oblique(plane.clone()));
    let image = volume.reformat_oblique(&plane, 64, || false).unwrap();
    let input = Input {
        series: series.clone(),
        scene: derived_scene.clone(),
        pixels: Pixels::Reformat(image, volume.clone()),
        device: device.clone(),
        queue: queue.clone(),
    };
    let oblique = worker::render(input, || false).unwrap();
    assert_eq!(
        oblique.manifest.scene.reconstruction,
        derived_scene.reconstruction
    );
    let mut state = orv_core::volume_view::VolumeView::fit(&volume);
    state.quality = orv_core::volume_view::Quality::Interactive;
    state.camera.yaw_degrees = 25.;
    derived_scene.reconstruction = Some(orv_core::scene::Reconstruction::Volume(state));
    derived_scene.reformat_side = 2;
    let input = Input {
        series: series.clone(),
        scene: derived_scene.clone(),
        pixels: Pixels::Volume(volume),
        device: device.clone(),
        queue: queue.clone(),
    };
    let projection = worker::render(input, || false).unwrap();
    assert_eq!(
        projection.manifest.scene.reconstruction,
        derived_scene.reconstruction
    );
    assert!(projection.manifest.volume.is_some());
    assert!(!projection.manifest.includes_ui_overlays);
    let mut wrong = (*series.frames[1]).clone();
    wrong.pixels = vec![999.; 16].into();
    let input = Input {
        series,
        scene,
        pixels: Pixels::Image(Arc::new(wrong)),
        device,
        queue,
    };
    assert!(worker::render(input, || false).is_err());
}
