// SPDX-License-Identifier: GPL-3.0-or-later
use orv_core::{
    ViewState, WindowLevel,
    oblique::ObliquePlane,
    scene::{ImageMetadata, Reconstruction, SceneState},
    slab::*,
    study::{Frame, Geometry},
    volume::Volume,
};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
fn phantom(missing: bool) -> Volume {
    Volume::new(
        (0..9)
            .map(|z| {
                let geometry = Geometry {
                    nifti_transform: None,
                    frame_of_reference: Some("generated-slab".into()),
                    position: [0.2 * f64::from(z), 0.1 * f64::from(z), f64::from(z) * 2.],
                    orientation: [1., 0., 0., 0., 1., 0.],
                    spacing: [1.5, 0.5],
                };
                let pixels = (0..9)
                    .flat_map(|y| {
                        (0..9).map(move |x| {
                            let p = [
                                0.5 * f64::from(x) + 0.2 * f64::from(z),
                                1.5 * f64::from(y) + 0.1 * f64::from(z),
                                2. * f64::from(z),
                            ];
                            (p[0] + 2. * p[1] - 3. * p[2] - 100.) as f32
                        })
                    })
                    .collect::<Vec<_>>();
                Arc::new(Frame {
                    slab: None,
                    scalar: None,
                    validity: missing.then(|| vec![0; 81].into()),
                    color: None,
                    nifti: None,
                    rgba: None,
                    width: 9,
                    height: 9,
                    pixels: pixels.into(),
                    geometry: Some(geometry),
                    instance: z,
                    frame_index: 0,
                    monochrome1: false,
                    default_window: WindowLevel::SOFT_TISSUE,
                })
            })
            .collect(),
    )
    .unwrap()
}
#[test]
fn slab_extrema_match_independent_negative_affine_phantom_with_shear_and_rotated_normals() {
    let volume = phantom(false);
    for angles in [[0.; 3], [23., -19., 11.]] {
        let plane = ObliquePlane::from_angles(volume.center(), angles).unwrap();
        for projection in [SlabProjection::Maximum, SlabProjection::Minimum] {
            let slab = Slab {
                projection,
                thickness_mm: 6.,
                intervals: 24,
            };
            let frame = volume.reformat_slab(&plane, slab, 41, || false).unwrap();
            let g = frame.geometry.as_ref().unwrap();
            let n = plane.normal_direction();
            let mut present = 0;
            for y in 0..frame.height {
                for x in 0..frame.width {
                    let center = g.pixel_to_world([f64::from(x), f64::from(y)]).unwrap();
                    let expected = (0..=24)
                        .filter_map(|i| {
                            let d = -3. + f64::from(i) * 0.25;
                            let p: [f64; 3] = std::array::from_fn(|a| center[a] + n[a] * d);
                            let z = p[2] / 2.;
                            let sx = (p[0] - 0.2 * z) / 0.5;
                            let sy = (p[1] - 0.1 * z) / 1.5;
                            ([sx, sy, z].iter().all(|v| *v >= 0. && *v <= 8.))
                                .then_some(p[0] + 2. * p[1] - 3. * p[2] - 100.)
                        })
                        .reduce(|a, b| {
                            if projection == SlabProjection::Maximum {
                                a.max(b)
                            } else {
                                a.min(b)
                            }
                        });
                    let actual = frame.pixels[(y * frame.width + x) as usize];
                    if let Some(expected) = expected {
                        present += 1;
                        assert!(
                            (f64::from(actual) - expected).abs() < 3e-5,
                            "{actual} != {expected}"
                        );
                    } else {
                        assert!(!actual.is_finite());
                    }
                }
            }
            assert!(present > 20);
            let metadata = ImageMetadata::from_frame(&frame);
            assert!(metadata.geometry.is_none());
            assert_eq!(metadata.slab.unwrap().parameters, slab);
            assert!(orv_core::measurements::roi_stats(&frame, [[0., 0.], [1., 1.]]).is_err());
            assert!(Volume::new(vec![frame.clone(), frame]).is_err());
        }
    }
}
#[test]
fn slab_missing_samples_never_become_zero_and_cancellation_is_bounded() {
    let volume = phantom(true);
    let plane = ObliquePlane::from_angles(volume.center(), [0.; 3]).unwrap();
    let slab = Slab {
        projection: SlabProjection::Maximum,
        thickness_mm: 2000.,
        intervals: 256,
    };
    let frame = volume.reformat_slab(&plane, slab, 128, || false).unwrap();
    assert!(frame.pixels.iter().all(|v| v.is_nan()));
    let checks = AtomicUsize::new(0);
    assert!(
        volume
            .reformat_slab(&plane, slab, 2048, || checks
                .fetch_add(1, Ordering::Relaxed)
                > 3)
            .is_err()
    );
    assert_eq!(checks.load(Ordering::Relaxed), 5);
    for intervals in [0, 1, 3, 255, 257, u16::MAX] {
        assert!(Slab { intervals, ..slab }.validate().is_err());
    }
    for thickness_mm in [f64::NAN, 0., -1., 2000.01] {
        assert!(
            Slab {
                thickness_mm,
                ..slab
            }
            .validate()
            .is_err()
        );
    }
}
#[test]
fn slab_scene_hash_binds_sampling_parameters_and_refuses_ambiguous_planes() {
    let volume = phantom(false);
    let plane = ObliquePlane::from_angles(volume.center(), [13., 7., -5.]).unwrap();
    let slab = Slab::for_volume(&volume, 10., SlabProjection::Minimum);
    let scene = SceneState {
        schema_version: 2,
        source_sha256: "a".repeat(64),
        series_index: 0,
        view: ViewState::default(),
        crosshair_mm: None,
        reformat_side: 128,
        output_size: [64, 64],
        reconstruction: Some(Reconstruction::Slab(SlabPlane { plane, slab })),
    };
    let hash = scene.hash().unwrap();
    let encoded = serde_json::to_string(&scene).unwrap();
    assert_eq!(scene, serde_json::from_str(&encoded).unwrap());
    for case in 0..4 {
        let mut changed = scene.clone();
        let Some(Reconstruction::Slab(state)) = &mut changed.reconstruction else {
            unreachable!()
        };
        match case {
            0 => state.slab.thickness_mm += 1.,
            1 => state.slab.projection = SlabProjection::Maximum,
            2 => state.slab.intervals = 2,
            _ => state.plane.center_mm[0] += 0.1,
        }
        assert_ne!(changed.hash().unwrap(), hash);
    }
    let mut invalid = scene;
    invalid.crosshair_mm = Some(volume.center());
    assert!(invalid.validate().is_err());
}

#[test]
fn finite_padding_cannot_win_extrema_and_rgb_slabs_are_refused() {
    let original = phantom(false);
    let mut frames = original.frames().to_vec();
    let mut padded = (*frames[4]).clone();
    padded.pixels = vec![1_000_000.; 81].into();
    padded.validity = Some(vec![0; 81].into());
    frames[4] = Arc::new(padded);
    let volume = Volume::new(frames).unwrap();
    let plane = ObliquePlane::from_angles(volume.center(), [0.; 3]).unwrap();
    for projection in [SlabProjection::Maximum, SlabProjection::Minimum] {
        let slab = Slab {
            projection,
            thickness_mm: 4.,
            intervals: 2,
        };
        let frame = volume.reformat_slab(&plane, slab, 41, || false).unwrap();
        let (x, y) = (frame.width / 2, frame.height / 2);
        let p = frame
            .geometry
            .as_ref()
            .unwrap()
            .pixel_to_world([f64::from(x), f64::from(y)])
            .unwrap();
        let expected = p[0] + 2. * p[1] - 3. * p[2] - 100.
            + if projection == SlabProjection::Maximum {
                6.
            } else {
                -6.
            };
        assert!((f64::from(frame.pixels[(y * frame.width + x) as usize]) - expected).abs() < 3e-5);
    }
    let color = Volume::new(
        original
            .frames()
            .iter()
            .map(|f| {
                let mut frame = (**f).clone();
                frame.pixels = Vec::new().into();
                frame.rgba = Some(vec![255; 81 * 4].into());
                Arc::new(frame)
            })
            .collect(),
    )
    .unwrap();
    assert!(
        color
            .reformat_slab(
                &plane,
                Slab {
                    projection: SlabProjection::Maximum,
                    thickness_mm: 2.,
                    intervals: 2
                },
                32,
                || false
            )
            .is_err()
    );
    assert!(color.reformat_oblique(&plane, 32, || false).is_ok());
}
