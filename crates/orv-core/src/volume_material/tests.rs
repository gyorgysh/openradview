// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use crate::{
    WindowLevel,
    study::{Frame, Geometry},
};
use std::sync::Arc;

fn point(value: f32, luminance: f32, extinction_per_mm: f32) -> MaterialPoint {
    MaterialPoint {
        value,
        luminance,
        extinction_per_mm,
    }
}
fn close(a: f64, b: f64) {
    assert!((a - b).abs() <= 1e-10, "{a} != {b}");
}

#[test]
fn piecewise_material_interpolates_physical_extinction_and_linear_light() {
    let material = VolumeMaterial::new(&[
        point(-100., 0., 0.),
        point(0., 0.25, 0.5),
        point(200., 1., 2.),
    ])
    .unwrap();
    for (value, light, extinction) in [
        (-1000., 0., 0.),
        (-100., 0., 0.),
        (-50., 0.125, 0.25),
        (0., 0.25, 0.5),
        (50., 0.4375, 0.875),
        (200., 1., 2.),
        (1000., 1., 2.),
    ] {
        let sample = material.sample(value).unwrap();
        close(sample.luminance, light);
        close(sample.extinction_per_mm, extinction);
    }
    for missing in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        assert!(material.sample(missing).is_none());
    }
}

#[test]
fn material_admission_refuses_ambiguous_or_invalid_parameters() {
    for points in [
        vec![],
        vec![point(0., 0., 0.)],
        vec![point(1., 0., 0.), point(0., 1., 1.)],
        vec![point(0., 0., 0.), point(0., 1., 1.)],
        vec![point(-0., 0., 0.), point(0., 1., 1.)],
        vec![point(f32::NAN, 0., 0.), point(1., 1., 1.)],
        vec![point(0., 0., 0.), point(f32::INFINITY, 1., 1.)],
        vec![point(0., -0.1, 0.), point(1., 1., 1.)],
        vec![point(0., f32::NAN, 0.), point(1., 1., 1.)],
        vec![point(0., 0., -0.1), point(1., 1., 1.)],
        vec![point(0., 0., 0.), point(1., 1., 11.)],
        vec![point(0., 0., 0.), point(1., 1., f32::INFINITY)],
        (0..65).map(|i| point(i as f32, 0., 0.)).collect(),
    ] {
        assert!(VolumeMaterial::new(&points).is_err(), "{points:?}");
    }
    let points: Vec<_> = (0..64).map(|i| point(i as f32, 1., 10.)).collect();
    assert_eq!(VolumeMaterial::new(&points).unwrap().points(), points);
}

#[test]
fn extreme_and_close_knots_do_not_overflow_or_collapse() {
    let m = VolumeMaterial::new(&[point(-1e20, 0., 0.), point(1e20, 1., 10.)]).unwrap();
    let middle = m.sample(0.).unwrap();
    close(middle.luminance, 0.5);
    close(middle.extinction_per_mm, 5.);
    let low = f32::from_bits(1);
    let high = f32::from_bits(3);
    let m = VolumeMaterial::new(&[point(low, 0., 0.), point(high, 1., 2.)]).unwrap();
    let middle = m.sample(f32::from_bits(2)).unwrap();
    close(middle.luminance, 0.5);
    close(middle.extinction_per_mm, 1.);
}

#[test]
fn transparent_intervals_include_internal_peaks_endpoint_clamping_and_adjacent_floats() {
    let material = VolumeMaterial::new(&[
        point(-100., 0., 0.),
        point(-50., 1., 0.),
        point(0., 0., 1.),
        point(50., 1., 0.),
        point(100., 0., 0.),
    ])
    .unwrap();
    for (lo, hi) in [(-1e20, -50.), (50., 1e20), (-50., -50.), (50., 50.)] {
        assert!(material.transparent_between(lo, hi), "{lo}..{hi}");
    }
    for (lo, hi) in [
        (-100., 100.),
        (-50., 50.),
        (-50., (-50.0f32).next_up()),
        (50.0f32.next_down(), 50.),
        (-1., 1.),
        (0., 0.),
        (1., -1.),
        (f32::NAN, 0.),
        (0., f32::INFINITY),
    ] {
        assert!(!material.transparent_between(lo, hi), "{lo}..{hi}");
    }
    let tiny = VolumeMaterial::new(&[point(0., 1., 0.), point(1., 0., f32::from_bits(1))]).unwrap();
    assert!(tiny.transparent_between(-1., 0.));
    assert!(!tiny.transparent_between(0., f32::from_bits(1)));
    // Luminance alone contributes no radiance without extinction.
    let zero = VolumeMaterial::new(&[point(-1e20, 0., 0.), point(1e20, 1., 0.)]).unwrap();
    assert!(zero.transparent_between(-f32::MAX, f32::MAX));
    let opaque = VolumeMaterial::new(&[point(0., 0., 1.), point(1., 0., 0.)]).unwrap();
    assert!(!opaque.transparent_between(-100., -50.));
    assert!(opaque.transparent_between(1., 100.));
}

#[test]
fn homogeneous_slab_attenuation_is_independent_of_partitioning() {
    let material = MaterialSample {
        luminance: 0.75,
        extinction_per_mm: 0.125,
    };
    let expected = 1. - (-1.0f64).exp(); // 0.125 / mm through 8 mm.
    for intervals in [1, 2, 7, 257, 4096] {
        let alpha = material.opacity(8. / f64::from(intervals)).unwrap();
        let mut transmission = 1.;
        for _ in 0..intervals {
            transmission *= 1. - alpha;
        }
        close(1. - transmission, expected);
    }
    assert_eq!(material.opacity(0.), Some(0.));
    assert_eq!(material.opacity(f64::MAX), Some(1.));
    assert!(material.opacity(-1.).is_none());
    assert!(material.opacity(f64::NAN).is_none());
    let transparent = MaterialSample {
        extinction_per_mm: 0.,
        ..material
    };
    assert_eq!(transparent.opacity(f64::MAX), Some(0.));
    let tiny = material.opacity(1e-100).unwrap();
    assert!(tiny > 0. && (tiny / 1.25e-101 - 1.).abs() < 1e-10);
}

fn field(flipped: bool, missing: Option<[usize; 3]>, constant: bool) -> Volume {
    let sign = if flipped { -1. } else { 1. };
    let mut frames: Vec<_> = (0..5)
        .map(|z| {
            let mut pixels = Vec::new();
            let mut valid = Vec::new();
            for y in 0..5 {
                for x in 0..5 {
                    // Independently defined physical affine and field:
                    // Bx=(0,2*s,0), By=(0,0,3), Bz=(4*s,.5,-.25).
                    let wx = 128. + 4. * sign * z as f64;
                    let wy = 256. + 2. * sign * x as f64 + 0.5 * z as f64;
                    let wz = 512. + 3. * y as f64 - 0.25 * z as f64;
                    pixels.push(if constant {
                        7.
                    } else {
                        (2. * wx - 3. * wy + 4. * wz + 10.) as f32
                    });
                    valid.push(u8::from(missing != Some([x, y, z])));
                }
            }
            Arc::new(Frame {
                width: 5,
                height: 5,
                pixels: pixels.into(),
                geometry: Some(Geometry {
                    position: [
                        128. + 4. * sign * z as f64,
                        256. + 0.5 * z as f64,
                        512. - 0.25 * z as f64,
                    ],
                    orientation: [0., sign, 0., 0., 0., 1.],
                    spacing: [3., 2.],
                    nifti_transform: None,
                    frame_of_reference: Some("generated gradient".into()),
                }),
                validity: missing.map(|_| valid.into()),
                presentation: None,
                calibration: None,
                projection: None,
                sop: None,
                slab: None,
                scalar: None,
                color: None,
                nifti: None,
                rgba: None,
                instance: z as i32,
                frame_index: 0,
                monochrome1: false,
                default_window: WindowLevel::BONE,
            })
        })
        .collect();
    frames.reverse();
    Volume::new(frames).unwrap()
}

#[test]
fn physical_gradient_recovers_known_field_through_rotation_anisotropy_and_shear() {
    for flipped in [false, true] {
        let volume = field(flipped, None, false);
        for voxel in [[0., 0., 0.], [0.25, 1.5, 2.], [2., 2., 2.], [4., 4., 4.]] {
            let gradient = physical_gradient(&volume, volume.voxel_to_world(voxel)).unwrap();
            for (value, expected) in gradient.into_iter().zip([2., -3., 4.]) {
                close(value, expected);
            }
        }
    }
}

#[test]
fn unknown_stencils_and_outside_points_do_not_invent_surface_normals() {
    let volume = field(false, Some([3, 2, 2]), false);
    assert!(physical_gradient(&volume, volume.voxel_to_world([2., 2., 2.])).is_none());
    assert!(physical_gradient(&volume, volume.voxel_to_world([3., 2., 2.])).is_none());
    assert!(physical_gradient(&volume, volume.voxel_to_world([-1., 2., 2.])).is_none());
    assert!(physical_gradient(&volume, [f64::NAN, 0., 0.]).is_none());
    assert!(physical_gradient(&volume, volume.voxel_to_world([0., 0., 0.])).is_some());
    let constant = field(false, None, true);
    let gradient = physical_gradient(&constant, constant.center()).unwrap();
    assert_eq!(gradient, [0.; 3]);
    assert!(gradient_normal(gradient).is_none());
}

#[test]
fn normal_is_scale_independent_including_subnormal_and_large_gradients() {
    let expected = [2. / 3., -1. / 3., 2. / 3.];
    for scale in [1., 1e300, 1e-300, f64::from_bits(4)] {
        let normal = gradient_normal([2. * scale, -scale, 2. * scale]).unwrap();
        for (value, expected) in normal.into_iter().zip(expected) {
            close(value, expected);
        }
    }
    assert!(gradient_normal([0.; 3]).is_none());
    assert!(gradient_normal([f64::NAN, 0., 0.]).is_none());
    assert!(gradient_normal([f64::INFINITY, 0., 0.]).is_none());
}
