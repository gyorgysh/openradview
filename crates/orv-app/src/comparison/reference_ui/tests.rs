// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
fn frame(position: [f64; 3], orientation: [f64; 6], size: [u32; 2]) -> Arc<Frame> {
    Arc::new(Frame {
        presentation: None,
        calibration: None,
        slab: None,
        scalar: None,
        validity: None,
        color: None,
        nifti: None,
        width: size[0],
        height: size[1],
        pixels: Vec::new().into(),
        rgba: None,
        geometry: Some(orv_core::study::Geometry {
            position,
            orientation,
            spacing: [2., 1.],
            frame_of_reference: Some("generated-shared-reference".into()),
            nifti_transform: None,
        }),
        instance: 1,
        frame_index: 0,
        monochrome1: false,
        default_window: orv_core::WindowLevel::SOFT_TISSUE,
    })
}
fn pair() -> [Arc<Frame>; 2] {
    [
        frame([0.; 3], [1., 0., 0., 0., 1., 0.], [5; 2]),
        frame([1., 4., -2.], [1., 0., 0., 0., 0., 1.], [3; 2]),
    ]
}
fn points(shape: &egui::Shape, white: &mut Vec<egui::Pos2>) -> usize {
    match shape {
        egui::Shape::Vec(shapes) => shapes.iter().map(|shape| points(shape, white)).sum(),
        egui::Shape::LineSegment { points, stroke } => {
            if stroke.color == theme::PAPER {
                white.extend_from_slice(points);
            }
            1
        }
        _ => 0,
    }
}
#[test]
fn finite_lines_use_original_canvas_mapping_under_parent_clipping_and_stay_below_later_overlays() {
    let ctx = egui::Context::default();
    let images = pair();
    let view = ViewState::default();
    for partial in [false, true] {
        let mut status = Status::Off;
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1000., 500.),
                )),
                ..Default::default()
            },
            |ui| {
                if partial {
                    ui.set_clip_rect(egui::Rect::from_min_max(
                        egui::pos2(50., 0.),
                        egui::pos2(1000., 500.),
                    ));
                }
                let a = Target::reserve(
                    ui,
                    egui::Rect::from_min_size(egui::pos2(10., 20.), egui::vec2(100., 100.)),
                    images[0].clone(),
                    view.clone(),
                );
                let b = Target::reserve(
                    ui,
                    egui::Rect::from_min_size(egui::pos2(210., 20.), egui::vec2(100., 100.)),
                    images[1].clone(),
                    view.clone(),
                );
                ui.painter().rect_filled(
                    egui::Rect::from_min_size(egui::pos2(15., 30.), egui::vec2(10., 10.)),
                    0.,
                    theme::INK,
                );
                status = paint_pair(
                    [Some(&a), Some(&b)],
                    [Some((0, &images[0], &view)), Some((0, &images[1], &view))],
                );
            },
        );
        output.textures_delta.clear();
        assert_eq!(status, Status::Shown);
        let index = output
            .shapes
            .iter()
            .position(|shape| matches!(shape.shape, egui::Shape::Vec(_)))
            .unwrap();
        let overlay = output
            .shapes
            .iter()
            .position(|shape| matches!(shape.shape, egui::Shape::Rect(_)))
            .unwrap();
        assert!(
            index < overlay,
            "Late slot filling must preserve the original canvas layer order"
        );
        let mut white = Vec::new();
        points(&output.shapes[index].shape, &mut white);
        let min = white
            .iter()
            .map(|point| point.x)
            .fold(f32::INFINITY, f32::min);
        let max = white
            .iter()
            .map(|point| point.x)
            .fold(f32::NEG_INFINITY, f32::max);
        assert!(
            (min - if partial { 50. } else { 45. }).abs() < 1e-4,
            "{min}"
        );
        assert!((max - 75.).abs() < 1e-4, "{max}");
        assert!(white.iter().all(|point| (point.y - 70.).abs() < 1e-4));
    }
}
#[test]
fn pending_changed_frame_or_view_and_unrelated_reference_never_fill_a_reserved_line() {
    let ctx = egui::Context::default();
    let images = pair();
    let view = ViewState::default();
    for scenario in 0..5 {
        let mut status = Status::Off;
        let replacement = Arc::new((*images[0]).clone());
        let mut changed_view = view.clone();
        changed_view.slice = 1;
        let mut unrelated = (*images[1]).clone();
        unrelated.geometry.as_mut().unwrap().frame_of_reference = Some("other-reference".into());
        let unrelated = Arc::new(unrelated);
        let second = if scenario == 4 {
            &unrelated
        } else {
            &images[1]
        };
        let mut output = ctx.run_ui(Default::default(), |ui| {
            let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(100., 100.));
            let a = Target::reserve(ui, rect, images[0].clone(), view.clone());
            let b = Target::reserve(ui, rect, second.clone(), view.clone());
            let current = match scenario {
                0 => None,
                1 => Some((0, &replacement, &view)),
                2 => Some((0, &images[0], &changed_view)),
                3 => Some((1, &images[0], &view)),
                _ => Some((0, &images[0], &view)),
            };
            status = paint_pair([Some(&a), Some(&b)], [current, Some((0, second, &view))]);
        });
        output.textures_delta.clear();
        assert_eq!(
            status,
            if scenario == 4 {
                Status::Reference
            } else {
                Status::Pending
            }
        );
        let mut white = Vec::new();
        assert_eq!(
            output
                .shapes
                .iter()
                .map(|shape| points(&shape.shape, &mut white))
                .sum::<usize>(),
            0
        );
    }
}
#[test]
fn screen_clipping_bounds_dashes_even_at_extreme_zoom_and_retains_explicit_no_line_states() {
    let ctx = egui::Context::default();
    let images = pair();
    let view = ViewState {
        zoom: 50.,
        ..Default::default()
    };
    let mut status = Status::Off;
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(10000., 10000.),
            )),
            ..Default::default()
        },
        |ui| {
            let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(10000., 10000.));
            let a = Target::reserve(ui, rect, images[0].clone(), view.clone());
            let b = Target::reserve(ui, rect, images[1].clone(), view.clone());
            status = paint_pair(
                [Some(&a), Some(&b)],
                [Some((0, &images[0], &view)), Some((0, &images[1], &view))],
            );
        },
    );
    output.textures_delta.clear();
    assert_eq!(status, Status::Shown);
    let mut white = Vec::new();
    let count = output
        .shapes
        .iter()
        .map(|shape| points(&shape.shape, &mut white))
        .sum::<usize>();
    assert!(count > 0 && count <= 512);
    assert!(
        white
            .iter()
            .all(|point| point.x >= 2. && point.y >= 2. && point.x <= 9998. && point.y <= 9998.)
    );
    assert_eq!(
        clip(
            [egui::pos2(-10., -10.), egui::pos2(10., -10.)],
            egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(10., 10.))
        ),
        None
    );
    for (other, expected) in [
        (images[0].clone(), Status::Coplanar),
        (
            frame([0., 0., 2.], [1., 0., 0., 0., 1., 0.], [5; 2]),
            Status::Parallel,
        ),
        (
            frame([100., 4., -2.], [1., 0., 0., 0., 0., 1.], [3; 2]),
            Status::Outside,
        ),
    ] {
        let mut output = ctx.run_ui(Default::default(), |ui| {
            let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(100., 100.));
            let plain = ViewState::default();
            let a = Target::reserve(ui, rect, images[0].clone(), plain.clone());
            let b = Target::reserve(ui, rect, other.clone(), plain.clone());
            status = paint_pair(
                [Some(&a), Some(&b)],
                [Some((0, &images[0], &plain)), Some((0, &other, &plain))],
            );
        });
        output.textures_delta.clear();
        assert_eq!(status, expected);
    }
}
