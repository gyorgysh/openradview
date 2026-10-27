// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use orv_core::scene::{Reconstruction, SceneState, VolumeMetadata, hex};
use orv_render::snapshot::{SnapshotManifest, SnapshotRenderer};
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicUsize, Ordering};

#[test]
fn production_volume_snapshot_matches_affine_rays_and_complete_provenance() {
    let volume = volume(None, false);
    let mut state = VolumeView::fit(&volume);
    state.camera.yaw_degrees = 23.;
    state.camera.pitch_degrees = -18.;
    state.camera.roll_degrees = 11.;
    state.clip = Some(PhysicalClip {
        minimum_mm: [0.4, 0.8, 1.],
        maximum_mm: [2.8, 3.6, 4.4],
    });
    let scene = SceneState {
        schema_version: 2,
        source_sha256: "0".repeat(64),
        series_index: 0,
        view: ViewState {
            window: WindowLevel {
                function: Default::default(),
                width: 512.,
                center: 256.,
            },
            ..Default::default()
        },
        crosshair_mm: None,
        reformat_side: 2,
        output_size: [13, 19],
        reconstruction: Some(Reconstruction::Volume(state.clone())),
    };
    let mut renderer = pollster::block_on(SnapshotRenderer::new()).expect("Real GPU required");
    let result = renderer
        .capture_volume(volume.clone(), &scene, || false)
        .unwrap();
    let png = image::load_from_memory(&result.png).unwrap().to_rgba8();
    assert_eq!(png.dimensions(), (13, 19));
    for y in 0..19 {
        for x in 0..13 {
            let expected = if let Some((origin, direction, [near, far])) =
                rays(&volume, &scene.view, &state, scene.output_size, x, y)
            {
                let value = |t| {
                    let p = volume
                        .world_to_voxel(std::array::from_fn(|i| origin[i] + direction[i] * t));
                    p[0] + 10. * p[1] + 100. * p[2]
                };
                f64::from(
                    scene
                        .view
                        .window
                        .normalize(value(near).max(value(far)) as f32),
                ) * 255.
            } else {
                10.
            };
            assert_gray(&png.get_pixel(x, y).0, expected);
        }
    }
    assert!(
        result.manifest.image.is_none(),
        "A projection must not invent a physical2D plane"
    );
    let metadata = result.manifest.volume.as_ref().unwrap();
    assert_eq!(metadata.source, VolumeMetadata::from_volume(&volume));
    assert_eq!(metadata.nominal_grid_step, 0.5);
    assert_eq!(metadata.maximum_intervals, 4096);
    assert_eq!(result.manifest.scene_sha256, scene.hash().unwrap());
    assert_eq!(
        result.manifest.png_sha256,
        hex(&Sha256::digest(&result.png))
    );
    assert_eq!(result.manifest.renderer_version, 8);
    let encoded = serde_json::to_string(&result.manifest).unwrap();
    assert_eq!(
        serde_json::from_str::<SnapshotManifest>(&encoded).unwrap(),
        result.manifest
    );
    assert!(
        renderer
            .capture(&volume.frames()[0], &scene, || false)
            .is_err()
    );
    let checks = AtomicUsize::new(0);
    assert!(
        renderer
            .capture_volume(volume.clone(), &scene, || checks
                .fetch_add(1, Ordering::Relaxed)
                >= 5)
            .is_err()
    );
    assert!(checks.load(Ordering::Relaxed) >= 5);
    let complete = renderer.capture_volume(volume, &scene, || false).unwrap();
    assert_eq!(
        complete.png, result.png,
        "Cancelled partial tiles must never affect a later snapshot"
    );
}

#[test]
fn oblique_snapshot_keeps_explicit_axes_and_physical_image_metadata() {
    let volume = volume(None, true);
    let plane =
        orv_core::oblique::ObliquePlane::from_angles(volume.center(), [25., 15., -12.]).unwrap();
    let frame = volume.reformat_oblique(&plane, 37, || false).unwrap();
    let scene = SceneState {
        schema_version: 2,
        source_sha256: "0".repeat(64),
        series_index: 0,
        view: ViewState {
            window: frame.default_window,
            ..Default::default()
        },
        crosshair_mm: None,
        reformat_side: 37,
        output_size: [23, 19],
        reconstruction: Some(Reconstruction::Oblique(plane.clone())),
    };
    let mut renderer = pollster::block_on(SnapshotRenderer::new()).expect("Real GPU required");
    let snapshot = renderer.capture(&frame, &scene, || false).unwrap();
    assert_eq!(snapshot.manifest.scene, scene);
    let geometry = snapshot
        .manifest
        .image
        .as_ref()
        .unwrap()
        .geometry
        .as_ref()
        .unwrap();
    assert_eq!(geometry.orientation[..3], plane.column_direction);
    assert_eq!(geometry.orientation[3..], plane.row_direction);
    assert!(snapshot.manifest.volume.is_none());
    assert!(renderer.capture_volume(volume, &scene, || false).is_err());
}

#[test]
fn slab_gpu_snapshot_preserves_extrema_before_windowing_and_has_no_measurement_plane() {
    use orv_core::slab::{Slab, SlabPlane, SlabProjection};
    let volume = volume(None, true);
    let plane =
        orv_core::oblique::ObliquePlane::from_angles(volume.center(), [21., -17., 9.]).unwrap();
    let normal = plane.normal_direction();
    // Independent affine phantom in world coordinates; source x+10y+100z.
    let slope = normal[0] + 10. * normal[1] / 1.5 + normal[2] * (50. - 0.125 - 5. / 3.);
    let mut renderer = pollster::block_on(SnapshotRenderer::new()).expect("Real GPU required");
    for projection in [SlabProjection::Maximum, SlabProjection::Minimum] {
        let slab = Slab {
            projection,
            thickness_mm: 0.5,
            intervals: 8,
        };
        let frame = volume.reformat_slab(&plane, slab, 37, || false).unwrap();
        let mut scene = SceneState {
            schema_version: 2,
            source_sha256: "a".repeat(64),
            series_index: 0,
            view: ViewState {
                window: frame.default_window,
                ..Default::default()
            },
            crosshair_mm: None,
            reformat_side: 37,
            output_size: [31, 31],
            reconstruction: Some(Reconstruction::Slab(SlabPlane {
                plane: plane.clone(),
                slab,
            })),
        };
        let result = renderer.capture(&frame, &scene, || false).unwrap();
        let png = image::load_from_memory(&result.png).unwrap().to_rgba8();
        let extreme = 166.5
            + if projection == SlabProjection::Maximum {
                slope.abs() * 0.25
            } else {
                -slope.abs() * 0.25
            };
        let expected = (1. - f64::from(scene.view.window.normalize(extreme as f32))) * 255.;
        assert_gray(&png.get_pixel(15, 15).0, expected);
        let metadata = result.manifest.image.unwrap();
        assert!(metadata.geometry.is_none());
        assert_eq!(metadata.slab.unwrap().parameters, slab);
        assert_eq!(result.manifest.scene_sha256, scene.hash().unwrap());
        assert!(renderer.capture(&frame, &scene, || true).is_err());
        if let Some(Reconstruction::Slab(state)) = &mut scene.reconstruction {
            state.plane.center_mm[2] += 0.1;
        }
        assert!(renderer.capture(&frame, &scene, || false).is_err());
        scene.reconstruction = Some(Reconstruction::Oblique(plane.clone()));
        assert!(renderer.capture(&frame, &scene, || false).is_err());
    }
}
