// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use crate::{
    color::{ColorData, ColorEncoding, ColorInfo},
    slab::{Slab, SlabProjection},
};
use std::{cell::Cell, sync::Arc};

fn image() -> Frame {
    let mut image = (*crate::study::demo_study().series[0].frames[0]).clone();
    image.width = 5;
    image.height = 5;
    image.pixels = (0..25).map(|i| i as f32).collect::<Vec<_>>().into();
    image.rgba = None;
    image.color = None;
    image.scalar = None;
    image.validity = None;
    let geometry = image.geometry.as_mut().unwrap();
    geometry.orientation = [1., 0., 0., 0., 1., 0.];
    geometry.spacing = [3., 2.];
    image
}
fn included(region: RoiShape<'_>) -> Vec<usize> {
    let region = region.prepare([5, 5], &|| false).unwrap();
    let mut points = Vec::new();
    region
        .visit(5, &|| false, |index| {
            points.push(index);
            Ok(())
        })
        .unwrap();
    points
}
#[test]
fn ellipse_pixel_centres_boundaries_and_missing_samples_have_independent_areas() {
    let mut image = image();
    let shape = RoiShape::Ellipse([[1., 1.], [3., 3.]]);
    assert_eq!(included(shape), [7, 11, 12, 13, 17]);
    let stats = roi_shape_stats(&image, shape).unwrap();
    assert_eq!(stats.samples.count, 5);
    assert_eq!(stats.samples.min, 7.);
    assert_eq!(stats.samples.max, 17.);
    assert_eq!(stats.samples.mean, 12.);
    assert!((stats.samples.standard_deviation - 10.4f64.sqrt()).abs() < 1e-12);
    assert_eq!(stats.samples.area_mm2, Some(30.));
    assert!((stats.geometric_area_mm2.unwrap() - 6. * std::f64::consts::PI).abs() < 1e-12);
    assert_eq!(
        roi_shape_stats(&image, RoiShape::Ellipse([[3., 3.], [1., 1.]])).unwrap(),
        stats
    );
    assert_eq!(
        included(RoiShape::Ellipse([[0.6, 0.6], [3.4, 3.4]])),
        [7, 11, 12, 13, 17]
    );
    assert_eq!(
        included(RoiShape::Ellipse([[0., 1.], [4., 3.]])),
        [7, 10, 11, 12, 13, 14, 17]
    );
    let mut mask = vec![1; 25];
    mask[12] = 0;
    image.validity = Some(mask.into());
    let missing = roi_shape_stats(&image, shape).unwrap();
    assert_eq!(missing.samples.count, 4);
    assert_eq!(missing.samples.mean, 12.);
    assert!((missing.samples.standard_deviation - 13f64.sqrt()).abs() < 1e-12);
    assert_eq!(missing.samples.area_mm2, Some(24.));
    assert_eq!(missing.geometric_area_mm2, stats.geometric_area_mm2);
}
#[test]
fn simple_concave_and_reversed_polygons_include_edges_and_vertices_without_filling_notches() {
    let triangle = [[0., 0.], [4., 0.], [0., 4.]];
    assert_eq!(
        included(RoiShape::Polygon(&triangle)),
        [0, 1, 2, 3, 4, 5, 6, 7, 8, 10, 11, 12, 15, 16, 20]
    );
    let stats = roi_shape_stats(&image(), RoiShape::Polygon(&triangle)).unwrap();
    assert_eq!(stats.samples.count, 15);
    assert_eq!(stats.samples.mean, 8.);
    assert!((stats.samples.standard_deviation - (98f64 / 3.).sqrt()).abs() < 1e-12);
    assert_eq!(stats.samples.area_mm2, Some(90.));
    assert_eq!(stats.geometric_area_mm2, Some(48.));
    let mut reversed = triangle;
    reversed.reverse();
    assert_eq!(
        roi_shape_stats(&image(), RoiShape::Polygon(&reversed)).unwrap(),
        stats
    );
    let concave = [[0., 0.], [4., 0.], [4., 1.], [1., 1.], [1., 4.], [0., 4.]];
    assert_eq!(
        included(RoiShape::Polygon(&concave)),
        [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 15, 16, 20, 21]
    );
    let stats = roi_shape_stats(&image(), RoiShape::Polygon(&concave)).unwrap();
    assert_eq!(stats.samples.mean, 8.625);
    assert_eq!(stats.geometric_area_mm2, Some(42.));
    let redundant = [[0., 0.], [1., 0.], [2., 0.], [2., 2.], [0., 2.]];
    assert_eq!(
        included(RoiShape::Polygon(&redundant)),
        [0, 1, 2, 5, 6, 7, 10, 11, 12]
    );
    let translated = triangle.map(|point| point.map(|v| v + 1e9));
    let region = RoiShape::Polygon(&translated)
        .prepare([1_000_000_005; 2], &|| false)
        .unwrap();
    assert_eq!(region.area_pixels, 8.);
    let mut count = 0;
    region
        .visit(1_000_000_005, &|| false, |_| {
            count += 1;
            Ok(())
        })
        .unwrap();
    assert_eq!(count, 15);
}
#[test]
fn degenerate_crossing_touching_and_unbounded_shapes_are_refused() {
    let frame = image();
    for points in [
        vec![],
        vec![[0., 0.], [1., 1.]],
        vec![[0., 0.], [1., 1.], [2., 2.]],
        vec![[0., 0.], [4., 4.], [0., 4.], [4., 0.]],
        vec![[0., 0.], [4., 0.], [4., 4.], [2., 0.], [0., 4.]],
        vec![[0., 0.], [2., 0.], [1., 0.], [1., 2.]],
        vec![[0., 0.], [0., 0.], [1., 1.]],
        vec![[0., 0.], [1., 0.], [1., 1.], [0., 0.]],
        vec![[-0.01, 0.], [2., 0.], [2., 2.]],
        vec![[0., 0.], [4.01, 0.], [2., 2.]],
        vec![[0., 0.], [1., 0.], [f64::NAN, 2.]],
        vec![[0., 0.]; MAX_POLYGON_VERTICES + 1],
        vec![[0., 0.], [4., 0.], [4., 1e-15]],
    ] {
        assert!(
            roi_shape_stats(&frame, RoiShape::Polygon(&points)).is_err(),
            "{points:?}"
        );
    }
    for corners in [
        [[1., 1.], [1., 3.]],
        [[1., 1.], [3., 1.]],
        [[0., 0.], [f64::INFINITY, 1.]],
        [[0., 0.], [4., 5.]],
    ] {
        assert!(roi_shape_stats(&frame, RoiShape::Ellipse(corners)).is_err());
    }
    assert!(
        RoiShape::Ellipse([[0., 0.], [f64::from_bits(1), 1.]])
            .prepare([5, 5], &|| false)
            .is_err()
    );
}
#[test]
fn rgb_shapes_use_the_same_mask_keep_source_precision_and_exclude_missing_alpha() {
    let mut image = image();
    image.pixels = Arc::new([]);
    let mut rgba = vec![255; 100];
    rgba[12 * 4 + 3] = 0;
    image.rgba = Some(rgba.into());
    image.color = Some(Arc::new(ColorData {
        info: ColorInfo {
            source_encoding: ColorEncoding::Rgb,
            bits: 16,
            icc_sha256: None,
        },
        rgb: (0..25)
            .flat_map(|i| [i * 1000, 100 + i, 65535 - i])
            .collect::<Vec<_>>()
            .into(),
    }));
    let shape = RoiShape::Ellipse([[1., 1.], [3., 3.]]);
    let stats = color_roi_shape_stats(&image, shape).unwrap();
    assert_eq!(stats.samples.count, 4);
    assert_eq!(stats.samples.bits, 16);
    assert_eq!(stats.samples.domain, ColorRoiDomain::BeforeIcc);
    assert_eq!(stats.samples.channels[0].mean, 12000.);
    assert_eq!(stats.samples.channels[1].mean, 112.);
    assert_eq!(stats.samples.channels[2].mean, 65523.);
    assert_eq!(stats.samples.area_mm2, Some(24.));
    assert!((stats.geometric_area_mm2.unwrap() - 6. * std::f64::consts::PI).abs() < 1e-12);
    assert!(roi_shape_stats(&image, shape).is_err());
    image.color = None;
    let presentation = color_roi_shape_stats(&image, shape).unwrap();
    assert_eq!(presentation.samples.bits, 8);
    assert_eq!(
        presentation.samples.domain,
        ColorRoiDomain::SrgbPresentation
    );
    assert_eq!(presentation.samples.channels[0].mean, 255.);
}
#[test]
fn physical_areas_use_the_plane_basis_and_remain_unknown_without_valid_numeric_geometry() {
    let mut image = image();
    let shape = RoiShape::Rectangle([[1., 1.], [3., 3.]]);
    let y = (1. - 1e-10f64).sqrt();
    image.geometry.as_mut().unwrap().orientation = [1., 0., 0., 1e-5, y, 0.];
    let stats = roi_shape_stats(&image, shape).unwrap();
    assert!((stats.geometric_area_mm2.unwrap() - 24. * y).abs() < 1e-12);
    assert!((stats.samples.area_mm2.unwrap() - 54. * y).abs() < 1e-12);
    for spacing in [[1e200, 1e200], [1e-200, 1e-200], [0., 2.]] {
        image.geometry.as_mut().unwrap().spacing = spacing;
        let stats = roi_shape_stats(&image, shape).unwrap();
        assert_eq!(stats.geometric_area_mm2, None);
        assert_eq!(stats.samples.area_mm2, None);
    }
    image.geometry = None;
    let stats = roi_shape_stats(&image, shape).unwrap();
    assert_eq!(stats.geometric_area_mm2, None);
    assert_eq!(stats.samples.area_mm2, None);
}
#[test]
fn cancellation_interrupts_polygon_validation_and_a_wide_single_row_without_partial_statistics() {
    let polygon = (0..MAX_POLYGON_VERTICES)
        .map(|i| {
            let angle = i as f64 * std::f64::consts::TAU / MAX_POLYGON_VERTICES as f64;
            [2. + angle.cos(), 2. + angle.sin()]
        })
        .collect::<Vec<_>>();
    let calls = Cell::new(0);
    assert!(matches!(
        RoiShape::Polygon(&polygon).prepare([5, 5], &|| {
            calls.set(calls.get() + 1);
            calls.get() >= 5
        }),
        Err("Cancelled")
    ));
    let wide = RoiShape::Rectangle([[0., 0.], [8191., 0.]])
        .prepare([8192, 1], &|| false)
        .unwrap();
    let sampled = Cell::new(0);
    let result = wide.visit(8192, &|| sampled.get() >= 500, |_| {
        sampled.set(sampled.get() + 1);
        Ok(())
    });
    assert!(matches!(result, Err("Cancelled")));
    assert_eq!(sampled.get(), 1024);
    let mut frame = image();
    frame.width = 8192;
    frame.height = 1;
    frame.pixels = vec![10.; 8192].into();
    calls.set(0);
    let result =
        roi_shape_stats_cancellable(&frame, RoiShape::Rectangle([[0., 0.], [8191., 0.]]), || {
            calls.set(calls.get() + 1);
            calls.get() >= 5
        });
    assert!(matches!(result, Err("Cancelled")));
    assert_eq!(calls.get(), 5);
    assert!(roi_shape_stats_cancellable(&image(), RoiShape::Polygon(&polygon), || true).is_err());
}
#[test]
fn legacy_rectangles_keep_one_cell_semantics_and_the_original_serialized_fields() {
    let mut frame = image();
    let corners = [[2., 2.], [2., 2.]];
    let old = roi_stats(&frame, corners).unwrap();
    let shape = roi_shape_stats(&frame, RoiShape::Rectangle(corners)).unwrap();
    assert_eq!(shape.samples, old);
    assert_eq!(old.count, 1);
    assert_eq!(old.mean, 12.);
    assert_eq!(old.area_mm2, Some(6.));
    assert_eq!(shape.geometric_area_mm2, Some(0.));
    assert_eq!(
        serde_json::to_value(old)
            .unwrap()
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        [
            "area_mm2",
            "count",
            "max",
            "mean",
            "min",
            "standard_deviation"
        ]
    );
    frame.validity = Some(vec![1; 24].into());
    assert!(roi_stats(&frame, corners).is_err());
    frame.validity = None;
    frame.slab = Some(Slab {
        projection: SlabProjection::Maximum,
        thickness_mm: 2.,
        intervals: 4,
    });
    assert!(roi_stats(&frame, corners).is_err());
    frame.pixels = Arc::new([]);
    frame.rgba = Some(vec![255; 100].into());
    assert!(color_roi_stats(&frame, corners).is_err());
}
