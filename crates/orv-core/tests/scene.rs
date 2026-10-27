// SPDX-License-Identifier: GPL-3.0-or-later
use orv_core::{
    Plane, ViewState,
    scene::{ImageMetadata, SceneState, series_fingerprint},
    study::demo_study,
};
use std::sync::Arc;

fn scene(source: String) -> SceneState {
    SceneState {
        schema_version: 1,
        source_sha256: source,
        series_index: 0,
        view: ViewState::default(),
        reconstruction: None,
        crosshair_mm: None,
        reformat_side: 512,
        output_size: [512, 512],
    }
}

#[test]
fn scene_hash_roundtrip_includes_physical_position_source_and_output() {
    let mut s = scene("0".repeat(64));
    s.view.plane = Plane::Coronal;
    s.crosshair_mm = Some([25.0, 30.0, 40.0]);
    let encoded = serde_json::to_string(&s).unwrap();
    let decoded: SceneState = serde_json::from_str(&encoded).unwrap();
    assert_eq!(s.hash().unwrap(), decoded.hash().unwrap());
    let hash = s.hash().unwrap();
    for change in 0..4 {
        let mut other = s.clone();
        match change {
            0 => other.crosshair_mm.as_mut().unwrap()[1] += 0.5,
            1 => other.source_sha256.replace_range(..1, "1"),
            2 => other.output_size[0] = 256,
            _ => other.view.window.center += 1.0,
        }
        assert_ne!(hash, other.hash().unwrap());
    }
    s.view.slice = 3;
    assert!(
        s.validate().is_err(),
        "MPR position must never come from an ambiguous UI index"
    );
    s.view.slice = 0;
    s.crosshair_mm.as_mut().unwrap()[0] = f64::NAN;
    assert!(s.validate().is_err());
    assert!(
        serde_json::from_str::<SceneState>(
            &encoded.replace("\"schema_version\":1", "\"unknown\":1,\"schema_version\":1")
        )
        .is_err()
    );
}

#[test]
fn fingerprint_covers_pixels_geometry_and_reference_equality_without_identifiers() {
    let mut study = demo_study();
    let series = &mut study.series[0];
    let hash = series_fingerprint(series, || false).unwrap();
    for frame in &mut series.frames {
        let frame = Arc::make_mut(frame);
        frame.geometry.as_mut().unwrap().frame_of_reference =
            Some("private replacement UID".into());
        frame.instance += 100;
    }
    assert_eq!(hash, series_fingerprint(series, || false).unwrap());
    let metadata = serde_json::to_string(&ImageMetadata::from_frame(&series.frames[0])).unwrap();
    assert!(
        !metadata.contains("private")
            && !metadata.contains("reference")
            && !metadata.contains("instance")
    );
    Arc::make_mut(&mut Arc::make_mut(&mut series.frames[0]).pixels)[0] += 1.0;
    assert_ne!(hash, series_fingerprint(series, || false).unwrap());
    Arc::make_mut(&mut Arc::make_mut(&mut series.frames[0]).pixels)[0] -= 1.0;
    Arc::make_mut(&mut series.frames[0])
        .geometry
        .as_mut()
        .unwrap()
        .position[0] += 0.5;
    assert_ne!(hash, series_fingerprint(series, || false).unwrap());
    Arc::make_mut(&mut series.frames[0])
        .geometry
        .as_mut()
        .unwrap()
        .position[0] -= 0.5;
    Arc::make_mut(&mut series.frames[1])
        .geometry
        .as_mut()
        .unwrap()
        .frame_of_reference = Some("different local reference".into());
    assert_ne!(hash, series_fingerprint(series, || false).unwrap());
    assert!(series_fingerprint(series, || true).is_err());
}

#[test]
fn snapshot_limits_reject_hostile_or_inconsistent_settings() {
    let s = scene("0".repeat(64));
    for change in 0..7 {
        let mut invalid = s.clone();
        match change {
            0 => invalid.output_size = [u32::MAX, u32::MAX],
            1 => invalid.output_size[0] = 0,
            2 => invalid.reformat_side = 1,
            3 => invalid.source_sha256 = "invalid".into(),
            4 => invalid.crosshair_mm = Some([0.0; 3]),
            5 => invalid.view.window.center = f32::MAX,
            _ => invalid.view.window.width = f32::MAX,
        }
        assert!(invalid.validate().is_err());
    }
}

#[test]
fn schema_one_json_and_hash_remain_byte_for_byte_compatible() {
    let s = scene("0".repeat(64));
    let json = concat!(
        "{\"schema_version\":1,\"source_sha256\":\"",
        "0000000000000000000000000000000000000000000000000000000000000000",
        "\",\"series_index\":0,\"view\":{\"schema_version\":1,\"plane\":\"Acquired\",\"slice\":0,",
        "\"window\":{\"width\":400.0,\"center\":40.0},\"zoom\":1.0,\"pan\":[0.0,0.0],\"invert\":false},",
        "\"crosshair_mm\":null,\"reformat_side\":512,\"output_size\":[512,512]}"
    );
    assert_eq!(serde_json::to_string(&s).unwrap(), json);
    assert_eq!(serde_json::from_str::<SceneState>(json).unwrap(), s);
    assert_eq!(
        s.hash().unwrap(),
        "7f368c2b0fe32fe5aa18c3a8fc27aca17192c3f788a376eafeea4db92db30d6b"
    );
}

#[test]
fn explicit_scene_context_is_versioned_complete_and_hash_sensitive() {
    use orv_core::{
        oblique::ObliquePlane,
        scene::{Reconstruction, VolumeMetadata},
        volume::Volume,
        volume_view::VolumeView,
    };
    let volume = Volume::new(demo_study().series[0].frames.clone()).unwrap();
    let mut s = scene("0".repeat(64));
    s.schema_version = 2;
    let plane = ObliquePlane::from_angles(volume.center(), [35., -18., 12.]).unwrap();
    s.reconstruction = Some(Reconstruction::Oblique(plane.clone()));
    s.validate().unwrap();
    let encoded = serde_json::to_string(&s).unwrap();
    assert_eq!(serde_json::from_str::<SceneState>(&encoded).unwrap(), s);
    let before = s.hash().unwrap();
    let Some(Reconstruction::Oblique(p)) = &mut s.reconstruction else {
        unreachable!()
    };
    p.center_mm[0] += 0.25;
    assert_ne!(s.hash().unwrap(), before);
    for change in 0..4 {
        let mut invalid = s.clone();
        match change {
            0 => invalid.schema_version = 1,
            1 => invalid.view.slice = 1,
            2 => invalid.view.plane = Plane::Axial,
            _ => invalid.crosshair_mm = Some([0.; 3]),
        }
        assert!(invalid.validate().is_err());
    }
    s.reformat_side = 2;
    s.reconstruction = Some(Reconstruction::Volume(VolumeView::fit(&volume)));
    let hash = s.hash().unwrap();
    let mut transformed_projection = s.clone();
    transformed_projection.view.transform.rotate_clockwise();
    assert!(transformed_projection.validate().is_err());
    for change in 0..6 {
        let mut changed = s.clone();
        let Some(Reconstruction::Volume(state)) = &mut changed.reconstruction else {
            unreachable!()
        };
        match change {
            0 => state.camera.roll_degrees += 1.,
            1 => state.projection = orv_core::volume_view::Projection::Composite,
            2 => state.transfer.opacity_per_mm += 0.1,
            3 => state.quality = orv_core::volume_view::Quality::Interactive,
            4 => state.camera.target_mm[1] += 0.25,
            _ => {
                state.clip = Some(orv_core::volume_view::PhysicalClip {
                    minimum_mm: [10.; 3],
                    maximum_mm: [50.; 3],
                })
            }
        }
        assert_ne!(changed.hash().unwrap(), hash);
    }
    s.reformat_side = 512;
    assert!(s.validate().is_err());
    let metadata = VolumeMetadata::from_volume(&volume);
    assert_eq!(metadata.dimensions, volume.dimensions());
    let json = serde_json::to_string(&metadata).unwrap();
    assert!(!json.contains("reference") && !json.contains("UID") && !json.contains("instance"));
    for axis in 0..3 {
        let p = volume.voxel_to_world(std::array::from_fn(|i| if i == axis { 1. } else { 0. }));
        for (i, value) in p.into_iter().enumerate() {
            assert!(
                (metadata.origin_mm[i] + metadata.voxel_steps_mm[axis][i] - value).abs() < 1e-9
            );
        }
    }
}
