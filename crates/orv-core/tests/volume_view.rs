// SPDX-License-Identifier: GPL-3.0-or-later
use orv_core::{
    ViewState, WindowLevel,
    oblique::ObliquePlane,
    spatial::GeometryError,
    study::{Frame, Geometry},
    volume::Volume,
    volume_view::*,
};
use std::sync::Arc;

fn volume() -> Volume {
    Volume::new(
        (0..3)
            .map(|z| {
                Arc::new(Frame {
                    slab: None,
                    scalar: None,
                    validity: None,
                    color: None,
                    nifti: None,
                    rgba: None,
                    width: 4,
                    height: 5,
                    pixels: (0..5)
                        .flat_map(|y| (0..4).map(move |x| (x + 10 * y + 100 * z) as f32))
                        .collect::<Vec<_>>()
                        .into(),
                    geometry: Some(Geometry {
                        nifti_transform: None,
                        frame_of_reference: Some("synthetic".into()),
                        position: [
                            10.0 + 0.5 * z as f64,
                            20.0 + z as f64,
                            30.0 + 2.0 * z as f64,
                        ],
                        orientation: [0., 1., 0., -1., 0., 0.],
                        spacing: [2., 0.5],
                    }),
                    instance: z,
                    frame_index: 0,
                    monochrome1: false,
                    default_window: WindowLevel::BONE,
                })
            })
            .collect(),
    )
    .unwrap()
}

#[test]
fn oblique_resampling_preserves_sheared_physical_coordinates_and_explicit_missing_data() {
    let volume = volume();
    let plane = ObliquePlane::from_angles(volume.center(), [17., -29., 38.]).unwrap();
    let frame = volume.reformat_oblique(&plane, 128, || false).unwrap();
    let geometry = frame.geometry.as_ref().unwrap();
    let mut finite = 0;
    let mut missing = 0;
    for y in 0..frame.height {
        for x in 0..frame.width {
            let p = geometry.pixel_to_world([x as f64, y as f64]).unwrap();
            let value = frame.pixels[(y * frame.width + x) as usize];
            if value.is_finite() {
                // Independent inverse for this rotated/sheared analytic phantom.
                let z = (p[2] - 30.0) / 2.0;
                let y = (10.0 + 0.5 * z - p[0]) / 2.0;
                let x = (p[1] - 20.0 - z) / 0.5;
                assert!((f64::from(value) - (x + 10.0 * y + 100.0 * z)).abs() < 1e-3);
                finite += 1;
            } else {
                missing += 1;
            }
        }
    }
    assert!(finite > 0 && missing > 0);
    let center_pixel = geometry.world_to_pixel(plane.center_mm).unwrap();
    let restored = geometry.pixel_to_world(center_pixel).unwrap();
    for (a, b) in restored.into_iter().zip(plane.center_mm) {
        assert!((a - b).abs() < 1e-9);
    }
    assert_eq!(
        volume.reformat_oblique(&plane, 128, || true).unwrap_err(),
        GeometryError::Cancelled
    );
    let mut invalid = plane;
    invalid.row_direction = invalid.column_direction;
    assert_eq!(
        volume
            .reformat_oblique(&invalid, 128, || false)
            .unwrap_err(),
        GeometryError::Orientation
    );
}

#[test]
fn orthographic_rays_use_physical_mm_and_clip_the_affine_domain() {
    let volume = volume();
    let mut camera = Camera::fit(&volume);
    let display = ViewState::default();
    for (yaw, pitch, expected) in [
        (0., 0., [0., 1., 0.]),
        (90., 0., [-1., 0., 0.]),
        (0., 90., [0., 0., 1.]),
    ] {
        camera.yaw_degrees = yaw;
        camera.pitch_degrees = pitch;
        let frame = camera.frame(&display, 1.0).unwrap();
        let (origin, direction) = frame.ray([0.5, 0.5]);
        assert_eq!(origin, volume.center());
        for (a, b) in direction.into_iter().zip(expected) {
            assert!((a - b).abs() < 1e-9);
        }
        let [near, far] = volume.ray_interval(origin, direction).unwrap();
        assert!(near < 0. && far > 0.);
        for t in [near, (near + far) * 0.5, far] {
            let p = std::array::from_fn(|i| origin[i] + direction[i] * t);
            assert!(volume.sample(p).is_some());
        }
        let before = std::array::from_fn(|i| origin[i] + direction[i] * (near - 0.1));
        assert!(volume.sample(before).is_none());
    }
    assert!(volume.ray_interval([1000.; 3], [0., 1., 0.]).is_none());
    assert!(volume.ray_interval(volume.center(), [0., 2., 0.]).is_none());
}

#[test]
fn physical_clipping_intersects_ordered_lps_bounds_and_preserves_legacy_views() {
    let volume = volume();
    let origin = volume.center();
    let clip = PhysicalClip {
        minimum_mm: [origin[0] - 1., origin[1] - 0.25, origin[2] - 1.],
        maximum_mm: [origin[0] + 1., origin[1] + 0.75, origin[2] + 1.],
    };
    assert_eq!(
        volume.clipped_ray_interval(origin, [0., 1., 0.], Some(&clip)),
        Some([-0.25, 0.75])
    );
    assert_eq!(clip.interval(origin, [0., -1., 0.]), Some([-0.75, 0.25]));
    assert!(clip.interval([1000.; 3], [0., 1., 0.]).is_none());
    assert!(clip.interval(origin, [0., 2., 0.]).is_none());
    assert_eq!(
        volume.clipped_ray_interval(origin, [0., 1., 0.], None),
        volume.ray_interval(origin, [0., 1., 0.])
    );
    let legacy = VolumeView::fit(&volume);
    let encoded = serde_json::to_string(&legacy).unwrap();
    assert!(!encoded.contains("clip"));
    assert_eq!(
        serde_json::from_str::<VolumeView>(&encoded).unwrap(),
        legacy
    );
    for bad in [
        PhysicalClip {
            minimum_mm: [0.; 3],
            maximum_mm: [0.; 3],
        },
        PhysicalClip {
            minimum_mm: [f64::NAN; 3],
            maximum_mm: [1.; 3],
        },
        PhysicalClip {
            minimum_mm: [-1e8; 3],
            maximum_mm: [1.; 3],
        },
    ] {
        assert!(bad.validate().is_err());
    }
}

#[test]
fn transfer_and_camera_state_roundtrip_and_reject_invalid_ranges() {
    let volume = volume();
    let mut view = VolumeView::fit(&volume);
    assert_eq!(
        serde_json::from_str::<VolumeView>(&serde_json::to_string(&view).unwrap()).unwrap(),
        view
    );
    for preset in [
        TransferPreset::SoftTissue,
        TransferPreset::Bone,
        TransferPreset::Lung,
    ] {
        let (window, transfer) = preset.values();
        assert!(window.is_valid());
        transfer.validate().unwrap();
        assert_eq!(transfer.weight(transfer.low), 0.);
        assert_eq!(
            transfer.weight(transfer.high),
            if transfer.shape == TransferShape::Ramp {
                1.
            } else {
                0.
            }
        );
    }
    view.camera.vertical_span_mm = f64::NAN;
    assert!(view.validate().is_err());
    view.camera = Camera::fit(&volume);
    view.transfer.opacity_per_mm = -1.;
    assert!(view.validate().is_err());
    let display = ViewState {
        zoom: 2.,
        pan: [0.25, -0.5],
        ..Default::default()
    };
    let base = Camera::fit(&volume);
    let frame = base.frame(&display, 2.).unwrap();
    let (target, _) = frame.ray([0.75, 0.0]);
    for (a, b) in target.into_iter().zip(volume.center()) {
        assert!((a - b).abs() < 1e-9);
    }
}

#[test]
fn fit_covers_affine_corners_in_narrow_and_wide_viewports_without_resetting_orbit() {
    let volume = volume();
    for aspect in [0.4, 2.5] {
        let mut camera = Camera::fit(&volume);
        camera.yaw_degrees = 31.;
        camera.pitch_degrees = -22.;
        camera.roll_degrees = 17.;
        camera.fit_to_volume(&volume, aspect).unwrap();
        assert_eq!(camera.yaw_degrees, 31.);
        let frame = camera.frame(&ViewState::default(), aspect).unwrap();
        for p in volume.corners() {
            let d: [f64; 3] = std::array::from_fn(|i| p[i] - frame.origin_mm[i]);
            let project = |axis: [f64; 3]| {
                d.iter().zip(axis).map(|(a, b)| a * b).sum::<f64>()
                    / axis.iter().map(|v| v * v).sum::<f64>()
            };
            assert!(project(frame.right_mm).abs() <= 0.5);
            assert!(project(frame.down_mm).abs() <= 0.5);
        }
    }
}
