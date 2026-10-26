// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use crate::color::{ColorData, ColorEncoding, ColorInfo};
use std::{cell::Cell, sync::Arc};

fn image(width: u32, values: &[f32]) -> Frame {
    Frame {
        validity: None,
        width,
        height: 1,
        pixels: values.into(),
        rgba: None,
        color: None,
        nifti: None,
        geometry: None,
        instance: 0,
        frame_index: 0,
        monochrome1: false,
        default_window: crate::WindowLevel::SOFT_TISSUE,
    }
}
fn geometry() -> Geometry {
    Geometry {
        nifti_transform: None,
        frame_of_reference: None,
        position: [0.; 3],
        orientation: [1., 0., 0., 0., 1., 0.],
        spacing: [3., 0.7],
    }
}
#[test]
fn source_histogram_counts_all_valid_values_with_explicit_endpoints() {
    let frame = image(7, &[-2., -2., 0., 1., 2., f32::NAN, f32::INFINITY]);
    let result = histogram(&frame, 64, || false).unwrap();
    assert_eq!((result.count, result.missing), (5, 2));
    let channel = &result.channels[0];
    assert_eq!((channel.min, channel.max), (-2., 2.));
    assert_eq!(channel.bins[0], 2);
    assert_eq!(channel.bins[32], 1);
    assert_eq!(channel.bins[48], 1);
    assert_eq!(channel.bins[63], 1);
    assert_eq!(channel.bins.iter().sum::<u64>(), 5);
    let constant = histogram(&image(2, &[17., 17.]), 256, || false).unwrap();
    assert_eq!(constant.channels[0].bins[0], 2);
    assert_eq!(constant.channels[0].min, constant.channels[0].max);
    assert!(histogram(&image(1, &[f32::NAN]), 64, || false).is_err());
}
#[test]
fn histogram_and_probe_preserve_sixteen_bit_codes_and_exclude_missing_color() {
    let mut frame = image(3, &[]);
    frame.rgba = Some(Arc::from([0, 0, 0, 255, 255, 255, 255, 255, 0, 0, 0, 0]));
    frame.color = Some(Arc::new(ColorData {
        info: ColorInfo {
            source_encoding: ColorEncoding::Rgb,
            bits: 16,
            icc_sha256: Some("a".repeat(64)),
        },
        rgb: Arc::from([0, 1000, 3000, 65535, 1001, 3000, 123, 456, 789]),
    }));
    let result = histogram(&frame, 256, || false).unwrap();
    assert_eq!((result.count, result.missing), (2, 1));
    assert_eq!(
        (result.channels[0].min, result.channels[0].max),
        (0., 65535.)
    );
    assert_eq!(
        (result.channels[1].min, result.channels[1].max),
        (1000., 1001.)
    );
    assert_eq!(result.channels[2].bins[0], 2);
    assert!(
        result
            .channels
            .iter()
            .all(|h| h.bins.iter().sum::<u64>() == 2)
    );
    assert_eq!(
        sample(&frame, [1., 0.]),
        Some(Sample::RgbCodes {
            values: [65535, 1001, 3000],
            bits: 16
        })
    );
    assert_eq!(sample(&frame, [2., 0.]), None);
    frame.color = None;
    assert_eq!(
        sample(&frame, [1., 0.]),
        Some(Sample::SrgbDisplay([255; 3]))
    );
    assert_eq!(
        histogram(&frame, 64, || false).unwrap().channels[0].max,
        255.
    );
}
#[test]
fn inspection_rejects_invalid_buffers_and_cancels_during_both_passes() {
    let frame = image(8192, &vec![1.; 8192]);
    let calls = Cell::new(0);
    assert_eq!(
        histogram(&frame, 64, || {
            calls.set(calls.get() + 1);
            calls.get() > 2
        }),
        Err("Cancelled")
    );
    assert!(histogram(&frame, 63, || false).is_err());
    assert!(histogram(&frame, 4097, || false).is_err());
    assert!(histogram(&image(2, &[1.]), 64, || false).is_err());
    let mut malformed = image(u32::MAX, &[]);
    assert!(histogram(&malformed, 64, || false).is_err());
    malformed.width = 1;
    malformed.rgba = Some(Arc::from([0, 0, 0, 128]));
    assert!(histogram(&malformed, 64, || false).is_err());
    assert_eq!(sample(&image(2, &[3., 4.]), [f64::NAN, 0.]), None);
    assert_eq!(sample(&image(2, &[3., 4.]), [2., 0.]), None);
}
#[test]
fn edge_labels_follow_axis_signs_and_oblique_dominance_not_anatomy_appearance() {
    let mut g = geometry();
    assert_eq!(edge_directions(&g).unwrap(), ["R", "L", "A", "P"]);
    g.orientation = [0., -1., 0., 0., 0., -1.];
    assert_eq!(edge_directions(&g).unwrap(), ["P", "A", "S", "I"]);
    let angle = 30f64.to_radians();
    g.orientation = [angle.cos(), angle.sin(), 0., -angle.sin(), angle.cos(), 0.];
    assert_eq!(edge_directions(&g).unwrap(), ["RA", "LP", "AL", "PR"]);
    g.spacing[0] = 0.;
    assert!(edge_directions(&g).is_none());
    g = geometry();
    g.orientation[0] = f64::NAN;
    assert!(edge_directions(&g).is_none());
}
#[test]
fn coordinate_reference_keeps_scanner_and_template_context_distinct() {
    let mut g = geometry();
    assert_eq!(coordinate_reference(&g), "Scanner LPS · mm");
    g.nifti_transform = Some(crate::study::NiftiTransform {
        method: crate::study::NiftiMethod::Sform,
        code: 4,
    });
    assert_eq!(coordinate_reference(&g), "MNI152 LPS · mm");
}
