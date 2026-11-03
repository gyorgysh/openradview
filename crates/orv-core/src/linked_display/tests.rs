// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use crate::scalar::{Rescale, ScalarData, ScalarInfo, Storage, ValueType, VoiChoices, VoiLut};
use std::sync::Arc;
fn image(size: [u32; 2], spacing: [f64; 2], position: V3) -> Frame {
    Frame {
        presentation: None,
        slab: None,
        scalar: None,
        validity: None,
        color: None,
        nifti: None,
        width: size[0],
        height: size[1],
        pixels: Vec::new().into(),
        rgba: None,
        geometry: Some(Geometry {
            position,
            orientation: [1., 0., 0., 0., 1., 0.],
            spacing,
            frame_of_reference: Some("generated-local-reference".into()),
            nifti_transform: None,
        }),
        instance: 1,
        frame_index: 0,
        monochrome1: false,
        default_window: WindowLevel::SOFT_TISSUE,
    }
}
fn pair() -> (Frame, Frame, ViewState) {
    (
        image([10, 10], [2., 1.], [0.; 3]),
        image([20, 10], [1., 0.5], [-0.25, -0.5, 4.]),
        ViewState {
            zoom: 2.,
            pan: [0.1, -0.2],
            slice: 12,
            ..Default::default()
        },
    )
}
fn close(a: f64, b: f64) {
    assert!((a - b).abs() < 1e-5, "{a} != {b}");
}
fn centre(frame: &Frame, view: &ViewState, aspect: f64) -> V3 {
    let pixel = mapping(frame, view, aspect)
        .unwrap()
        .view_to_pixel_unbounded([0.5; 2])
        .unwrap();
    frame
        .geometry
        .as_ref()
        .unwrap()
        .pixel_to_world(pixel)
        .unwrap()
}
#[test]
fn physical_anchor_and_field_of_view_follow_spacing_without_copying_view_pixels_or_other_fields() {
    let (source, target, view) = pair();
    let action = PhysicalDisplay::capture(&source, &view, 1.).unwrap();
    close(action.vertical_fov_mm(), 10.);
    for (a, b) in centre(&source, &view, 1.).into_iter().zip([3.5, 11., 0.]) {
        close(a, b);
    }
    let target_view = ViewState {
        slice: 7,
        window: WindowLevel::BONE,
        invert: true,
        ..Default::default()
    };
    let result = action
        .apply(
            &target,
            &target_view,
            1.,
            DisplayLinks {
                pan: true,
                zoom: true,
            },
        )
        .unwrap();
    close(f64::from(result.zoom), 1.);
    close(f64::from(result.pan[0]), 0.1);
    close(f64::from(result.pan[1]), -0.7);
    assert_eq!(result.slice, 7);
    assert_eq!(result.window, target_view.window);
    assert!(result.invert);
    assert_eq!(result.transform, target_view.transform);
    for (a, b) in centre(&target, &result, 1.).into_iter().zip([3.5, 11., 4.]) {
        close(a, b);
    }
    close(
        PhysicalDisplay::capture(&target, &result, 1.)
            .unwrap()
            .vertical_fov_mm(),
        10.,
    );
    assert!(source.pixels.is_empty() && target.pixels.is_empty());
}
#[test]
fn pan_and_zoom_switches_are_independent_and_zoom_preserves_the_targets_own_centre() {
    let (source, target, view) = pair();
    let action = PhysicalDisplay::capture(&source, &view, 1.).unwrap();
    let original = ViewState {
        zoom: 2.,
        pan: [0.2, 0.1],
        ..Default::default()
    };
    let result = action
        .apply(
            &target,
            &original,
            1.,
            DisplayLinks {
                pan: true,
                zoom: false,
            },
        )
        .unwrap();
    assert_eq!(result.zoom, 2.);
    close(f64::from(result.pan[0]), 0.2);
    close(f64::from(result.pan[1]), -1.4);
    let before = centre(&target, &original, 1.);
    let result = action
        .apply(
            &target,
            &original,
            1.,
            DisplayLinks {
                pan: false,
                zoom: true,
            },
        )
        .unwrap();
    assert_eq!(result.zoom, 1.);
    assert_ne!(
        result.pan, original.pan,
        "Normalized pan must change to preserve a physical centre at new zoom"
    );
    for (a, b) in centre(&target, &result, 1.).into_iter().zip(before) {
        close(a, b);
    }
    assert_eq!(
        action
            .apply(&target, &original, 1., DisplayLinks::default())
            .unwrap(),
        original
    );
}
#[test]
fn transformed_asymmetric_images_and_different_viewport_aspects_keep_the_same_physical_anchor() {
    let (source, _, view) = pair();
    let target = image([40, 10], [1., 0.5], [-0.25, -0.5, 4.]);
    let action = PhysicalDisplay::capture(&source, &view, 1.).unwrap();
    for aspect in [0.5, 1., 2.] {
        for quarter_turns in 0..4 {
            for flip_horizontal in [false, true] {
                for flip_vertical in [false, true] {
                    let view = ViewState {
                        transform: crate::DisplayTransform {
                            quarter_turns,
                            flip_horizontal,
                            flip_vertical,
                        },
                        ..Default::default()
                    };
                    let result = action
                        .apply(
                            &target,
                            &view,
                            aspect,
                            DisplayLinks {
                                pan: true,
                                zoom: true,
                            },
                        )
                        .unwrap();
                    assert_eq!(result.transform, view.transform);
                    close(
                        PhysicalDisplay::capture(&target, &result, aspect)
                            .unwrap()
                            .vertical_fov_mm(),
                        10.,
                    );
                    for (a, b) in centre(&target, &result, aspect)
                        .into_iter()
                        .zip([3.5, 11., 4.])
                    {
                        close(a, b);
                    }
                }
            }
        }
    }
}
#[test]
fn spatial_refusal_and_out_of_range_candidates_leave_original_views_untouched() {
    let (source, mut target, view) = pair();
    let action = PhysicalDisplay::capture(&source, &view, 1.).unwrap();
    let original = ViewState::default();
    let links = DisplayLinks {
        pan: true,
        zoom: true,
    };
    target.geometry.as_mut().unwrap().frame_of_reference = Some("unrelated".into());
    assert_eq!(
        action.apply(&target, &original, 1., links),
        Err(LinkError::Reference)
    );
    let (_, mut target, _) = pair();
    target.geometry.as_mut().unwrap().orientation = [1., 0., 0., 0., 0., 1.];
    assert_eq!(
        action.apply(&target, &original, 1., links),
        Err(LinkError::Orientation)
    );
    let (_, mut target, _) = pair();
    target.geometry.as_mut().unwrap().nifti_transform = Some(crate::study::NiftiTransform {
        method: crate::study::NiftiMethod::Sform,
        code: 4,
    });
    assert_eq!(
        action.apply(&target, &original, 1., links),
        Err(LinkError::Reference)
    );
    let (_, mut target, _) = pair();
    target.geometry.as_mut().unwrap().position[0] = 1e6;
    assert_eq!(
        action.apply(&target, &original, 1., links),
        Err(LinkError::Limits)
    );
    let tiny = image([10, 10], [0.0001; 2], [0.; 3]);
    let big = image([10, 10], [100.; 2], [0.; 3]);
    let action = PhysicalDisplay::capture(
        &tiny,
        &ViewState {
            zoom: 50.,
            ..Default::default()
        },
        1.,
    )
    .unwrap();
    assert_eq!(
        action.apply(&big, &original, 1., links),
        Err(LinkError::Limits)
    );
    assert_eq!(original, ViewState::default());
}
#[test]
fn large_common_translations_keep_small_anchor_offsets_and_unbounded_anchors_never_become_picks() {
    let (mut source, mut target, view) = pair();
    for axis in 0..3 {
        let delta = [1e12, -2e12, 3e12][axis];
        source.geometry.as_mut().unwrap().position[axis] += delta;
        target.geometry.as_mut().unwrap().position[axis] += delta;
    }
    let action = PhysicalDisplay::capture(&source, &view, 1.).unwrap();
    let result = action
        .apply(
            &target,
            &ViewState::default(),
            1.,
            DisplayLinks {
                pan: true,
                zoom: true,
            },
        )
        .unwrap();
    close(f64::from(result.pan[0]), 0.1);
    close(f64::from(result.pan[1]), -0.7);
    let map = mapping(&target, &result, 1.).unwrap();
    assert!(
        map.view_to_pixel([0.5; 2]).is_none(),
        "A centre outside the image is not a valid probe"
    );
    let pixel = map.view_to_pixel_unbounded([0.5; 2]).unwrap();
    close(pixel[0], 7.5);
    close(pixel[1], 11.5);
    assert!(map.view_to_pixel_unbounded([f64::INFINITY, 0.5]).is_none());
}
fn info(unit: Unit) -> ScalarInfo {
    ScalarInfo {
        modality_lut: None,
        real_world: None,
        value_type: ValueType::F32,
        storage: Storage::SignedInteger,
        stored_bits: 16,
        unit,
        rescale: Some(Rescale {
            slope: 1.,
            intercept: -1024.,
        }),
        resampled: false,
    }
}
fn scalar(info: ScalarInfo, lut: Option<Vec<u16>>) -> Arc<ScalarData> {
    let luts = lut
        .map(|values| vec![VoiLut::new(0, 16, values.into()).unwrap()])
        .unwrap_or_default();
    ScalarData::new(info, VoiChoices::new(vec![], luts).unwrap()).unwrap()
}
#[test]
fn window_link_requires_known_units_exact_mapping_and_table_identity_instead_of_a_shared_index() {
    let (mut source, mut target, _) = pair();
    let mut description = info(Unit::Hounsfield);
    source.scalar = Some(scalar(description, None));
    target.scalar = source.scalar.clone();
    assert_eq!(
        scalar_window(&source, &target, WindowLevel::BONE),
        Ok(WindowLevel::BONE)
    );
    for change in 0..5 {
        let mut changed = description;
        match change {
            0 => changed.unit = Unit::Counts,
            1 => changed.storage = Storage::UnsignedInteger,
            2 => changed.stored_bits = 12,
            3 => {
                changed.rescale = Some(Rescale {
                    slope: 2.,
                    intercept: -1024.,
                })
            }
            _ => {
                changed.resampled = true;
                changed.rescale = None;
            }
        }
        target.scalar = Some(scalar(changed, None));
        assert_eq!(
            scalar_window(&source, &target, WindowLevel::BONE),
            Err(LinkError::Representation)
        );
    }
    description.unit = Unit::Unknown;
    source.scalar = Some(scalar(description, None));
    target.scalar = source.scalar.clone();
    assert_eq!(
        scalar_window(&source, &target, WindowLevel::BONE),
        Err(LinkError::Representation)
    );
    description.unit = Unit::Hounsfield;
    source.scalar = Some(scalar(description, Some(vec![0, 100, 65535])));
    target.scalar = Some(scalar(description, Some(vec![0, 200, 65535])));
    assert_eq!(
        scalar_window(&source, &target, WindowLevel::lookup(0)),
        Err(LinkError::Representation)
    );
    target.scalar = source.scalar.clone();
    assert_eq!(
        scalar_window(&source, &target, WindowLevel::lookup(0)),
        Ok(WindowLevel::lookup(0))
    );
    assert_eq!(
        scalar_window(&source, &target, WindowLevel::lookup(1)),
        Err(LinkError::Representation)
    );
    target.rgba = Some(Vec::new().into());
    assert_eq!(
        scalar_window(&source, &target, WindowLevel::BONE),
        Err(LinkError::Representation)
    );
    target.rgba = None;
    target.scalar = None;
    assert_eq!(
        scalar_window(&source, &target, WindowLevel::BONE),
        Err(LinkError::Representation)
    );
}
#[test]
fn malformed_unknown_projected_and_unrepresentable_views_cannot_capture_an_action() {
    let (mut source, _, view) = pair();
    assert!(matches!(
        PhysicalDisplay::capture(&source, &view, 0.),
        Err(LinkError::View)
    ));
    assert!(matches!(
        PhysicalDisplay::capture(
            &source,
            &ViewState {
                zoom: f32::NAN,
                ..view.clone()
            },
            1.
        ),
        Err(LinkError::View)
    ));
    source.geometry.as_mut().unwrap().frame_of_reference = None;
    assert!(matches!(
        PhysicalDisplay::capture(&source, &view, 1.),
        Err(LinkError::Reference)
    ));
    let (mut source, _, view) = pair();
    source.slab = Some(crate::slab::Slab {
        projection: crate::slab::SlabProjection::Maximum,
        thickness_mm: 2.,
        intervals: 2,
    });
    assert!(matches!(
        PhysicalDisplay::capture(&source, &view, 1.),
        Err(LinkError::Geometry)
    ));
    source.slab = None;
    source.geometry.as_mut().unwrap().spacing = [f64::MAX; 2];
    assert!(PhysicalDisplay::capture(&source, &view, 1.).is_err());
}

#[test]
fn imperfect_supplied_axes_keep_the_exact_physical_centre_and_axis_extent() {
    let (source, mut target, view) = pair();
    target.geometry.as_mut().unwrap().orientation = [1.00002, 0., 0., 0.00003, 0.99998, 0.];
    let action = PhysicalDisplay::capture(&source, &view, 1.).unwrap();
    let result = action
        .apply(
            &target,
            &ViewState::default(),
            1.,
            DisplayLinks {
                pan: true,
                zoom: true,
            },
        )
        .unwrap();
    for (a, b) in centre(&target, &result, 1.).into_iter().zip([3.5, 11., 4.]) {
        close(a, b);
    }
    close(
        f64::from(result.zoom),
        (0.00003f64.powi(2) + 0.99998f64.powi(2)).sqrt(),
    );
    let choices = VoiChoices::new(vec![], vec![]).unwrap();
    let mut source = source;
    source.scalar = Some(
        ScalarData::with_unit_identity(info(Unit::Hounsfield), choices.clone(), Some([1; 32]))
            .unwrap(),
    );
    target.scalar = Some(
        ScalarData::with_unit_identity(info(Unit::Hounsfield), choices, Some([2; 32])).unwrap(),
    );
    assert_eq!(
        scalar_window(&source, &target, WindowLevel::BONE),
        Err(LinkError::Representation)
    );
}
