// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use std::time::{Duration, Instant};
fn image() -> Arc<Frame> {
    let mut image = (*orv_core::study::demo_study().series[0].frames[0]).clone();
    image.width = 5;
    image.height = 5;
    image.pixels = (0..25).map(|i| i as f32).collect::<Vec<_>>().into();
    image.geometry.as_mut().unwrap().spacing = [3., 2.];
    Arc::new(image)
}
fn annotations(image: &Frame, tool: Tool, points: Vec<[f64; 2]>) -> Annotations {
    let mut annotations = Annotations::default();
    annotations.ensure_admitted().unwrap();
    annotations.bind_image(image, &ViewState::default());
    annotations.active_tool = Some(Tool::EditMarks);
    annotations.marks.push(Mark {
        id: 0,
        revision: 0,
        image: annotations.image.clone().unwrap(),
        label: report(image, &points, tool, || false).unwrap(),
        points,
        tool,
    });
    annotations.next_id = 1;
    annotations.next_revision = 1;
    annotations.selected = Some(0);
    annotations
}
fn wait(annotations: &mut Annotations) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while annotations.calculation.is_busy() {
        annotations.poll();
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
}
fn triangle(image: &Frame) -> Annotations {
    annotations(image, Tool::PolygonRoi, vec![[0., 0.], [4., 0.], [0., 4.]])
}
#[test]
fn accepted_geometry_reports_and_history_move_together_without_retaining_images() {
    let ctx = egui::Context::default();
    let image = image();
    let mut annotations = triangle(&image);
    let old = annotations.marks[0].points.clone();
    let original_label = annotations.marks[0].label.clone();
    assert!(original_label.contains("n=15 · mean 8.00"));
    let changed = vec![[0., 0.], [2., 0.], [0., 4.]];
    annotations
        .propose_edit(0, 0, changed.clone(), &image, &ctx)
        .unwrap();
    wait(&mut annotations);
    assert_eq!(annotations.marks[0].points, old);
    assert!(!annotations.history.can_undo());
    assert!(annotations.capture().is_err());
    annotations.finish_candidate();
    assert_eq!(annotations.marks[0].points, changed);
    assert!(annotations.marks[0].label.contains("n=9 · mean 7.78"));
    assert!(annotations.marks[0].label.contains("24.00 mm² shape"));
    let accepted_revision = annotations.marks[0].revision;
    annotations.bind_image(
        &image,
        &ViewState {
            slice: 1,
            ..Default::default()
        },
    );
    annotations.undo_edit(false);
    assert_eq!(annotations.visible_marks().count(), 0);
    assert_eq!(annotations.marks[0].points, old);
    assert_eq!(annotations.marks[0].label, original_label);
    assert!(annotations.marks[0].revision > accepted_revision);
    annotations.undo_edit(true);
    assert_eq!(annotations.marks[0].points, changed);
    assert!(annotations.marks[0].label.contains("n=9 · mean 7.78"));
    assert_eq!(Arc::strong_count(&image), 1);
}
#[test]
fn invalid_cancelled_context_changed_and_stale_base_edits_keep_the_original_mark() {
    let ctx = egui::Context::default();
    let image = image();
    let old = vec![[0., 0.], [4., 0.], [4., 4.], [0., 4.]];
    let mut annotations = annotations(&image, Tool::PolygonRoi, old.clone());
    annotations
        .propose_edit(
            0,
            0,
            vec![[0., 0.], [4., 4.], [4., 0.], [0., 4.]],
            &image,
            &ctx,
        )
        .unwrap();
    wait(&mut annotations);
    annotations.finish_candidate();
    assert_eq!(annotations.marks[0].points, old);
    assert!(!annotations.history.can_undo());
    assert!(
        annotations
            .editor
            .error
            .as_ref()
            .unwrap()
            .contains("Original geometry kept")
    );
    for change in 0..3 {
        let mut points = old.clone();
        points[0] = [0.5, 0.5];
        annotations
            .propose_edit(0, 0, points, &image, &ctx)
            .unwrap();
        wait(&mut annotations);
        match change {
            0 => annotations.cancel_interaction(&ctx),
            1 => annotations.bind_image(
                &image,
                &ViewState {
                    slice: 1,
                    ..Default::default()
                },
            ),
            _ => annotations.marks[0].revision = 99,
        }
        annotations.finish_candidate();
        assert_eq!(annotations.marks[0].points, old);
        assert!(!annotations.history.can_undo());
        annotations.bind_image(&image, &ViewState::default());
    }
    assert_eq!(Arc::strong_count(&image), 1);
}
#[test]
fn numeric_apply_uses_one_based_coordinates_and_rejects_an_invalid_anchor() {
    let ctx = egui::Context::default();
    let image = image();
    let mut annotations = triangle(&image);
    for (column, accepted) in [(3., true), (0., false)] {
        annotations.select_vertex(0, 1);
        annotations.editor.numeric.as_mut().unwrap().coordinates = [column, 1.];
        annotations.editor.numeric.as_mut().unwrap().dirty = true;
        annotations.editor.apply = true;
        assert!(annotations.capture().is_err());
        let mut output = ctx.run_ui(Default::default(), |ui| {
            let (_, response) =
                ui.allocate_exact_size(egui::vec2(400., 400.), egui::Sense::click_and_drag());
            annotations.handle_editor(ui, &response, &image, &ViewState::default());
        });
        output.textures_delta.clear();
        wait(&mut annotations);
        annotations.finish_candidate();
        assert_eq!(annotations.marks[0].points[1], [2., 0.]);
        if !accepted {
            assert!(
                annotations
                    .editor
                    .error
                    .as_ref()
                    .unwrap()
                    .contains("valid image sample")
            );
        }
    }
}
#[test]
fn edited_legacy_and_new_kinds_capture_only_coordinates_and_restore_fresh_reports() {
    let ctx = egui::Context::default();
    let image = image();
    for (tool, points, changed) in [
        (
            Tool::Distance,
            vec![[1., 1.], [3., 3.]],
            vec![[0.5, 1.], [3., 3.]],
        ),
        (
            Tool::EllipseRoi,
            vec![[1., 1.], [3., 3.]],
            vec![[0.5, 1.], [3., 3.]],
        ),
        (
            Tool::PolygonRoi,
            vec![[0., 0.], [4., 0.], [0., 4.]],
            vec![[0., 0.], [2., 0.], [0., 4.]],
        ),
    ] {
        let mut annotations = annotations(&image, tool, points);
        annotations
            .propose_edit(0, 0, changed.clone(), &image, &ctx)
            .unwrap();
        wait(&mut annotations);
        annotations.finish_candidate();
        let captured = annotations.capture().unwrap();
        assert_eq!(captured[0].saved.points, changed);
        assert_eq!(captured[0].saved.tool.extended(), tool.roi());
        let json = serde_json::to_string(&captured[0].saved).unwrap();
        assert!(!json.contains("report") && !json.contains("mean") && !json.contains("revision"));
        let saved: SavedMark = serde_json::from_str(&json).unwrap();
        let mut series = orv_core::study::demo_study().series.remove(0);
        series.frames = vec![image.clone()];
        let restored = Annotations::restore(&[saved], &series, || false).unwrap();
        assert_eq!(restored.marks[0].points, changed);
        assert_eq!(restored.marks[0].label, annotations.marks[0].label);
        assert!(!restored.history.can_undo());
    }
}
#[test]
fn handle_hit_radius_is_logical_and_transforms_return_the_same_source_vertex() {
    let image = image();
    let annotations = annotations(&image, Tool::Distance, vec![[2., 2.], [4., 4.]]);
    let rect = Rect::from_min_size(egui::pos2(40., 60.), egui::vec2(800., 600.));
    for quarter_turns in 0..4 {
        for flip_horizontal in [false, true] {
            for flip_vertical in [false, true] {
                let view = ViewState {
                    zoom: 1.4,
                    pan: [0.1, -0.1],
                    transform: orv_core::DisplayTransform {
                        quarter_turns,
                        flip_horizontal,
                        flip_vertical,
                    },
                    ..Default::default()
                };
                let point = screen_at(rect, &image, &view, [2., 2.]).unwrap();
                assert_eq!(
                    annotations.pick_handle(rect, &image, &view, point + egui::vec2(9., 0.)),
                    Some((0, 0))
                );
                assert_eq!(
                    annotations.pick_handle(rect, &image, &view, point + egui::vec2(11., 0.)),
                    None
                );
                let pixel = pixel_at(rect, &image, &view, point).unwrap();
                assert!((pixel[0] - 2.).abs() < 1e-5 && (pixel[1] - 2.).abs() < 1e-5);
            }
        }
    }
}
fn editor_frame(
    ctx: &egui::Context,
    annotations: &mut Annotations,
    image: &Arc<Frame>,
    events: Vec<egui::Event>,
) -> Rect {
    let mut rect = Rect::NOTHING;
    let mut output = ctx.run_ui(
        egui::RawInput {
            events,
            ..Default::default()
        },
        |ui| {
            let (_, response) =
                ui.allocate_exact_size(egui::vec2(400., 400.), egui::Sense::click_and_drag());
            rect = response.rect;
            annotations.show(ui, &response, image, &ViewState::default(), Tool::EditMarks);
        },
    );
    output.textures_delta.clear();
    rect
}
#[test]
fn real_pointer_drag_commits_once_on_release_and_escape_restores_without_history() {
    for cancel in [false, true] {
        let ctx = egui::Context::default();
        let image = image();
        let mut annotations = annotations(&image, Tool::EllipseRoi, vec![[1., 1.], [3., 3.]]);
        let original = annotations.marks[0].points.clone();
        let rect = editor_frame(&ctx, &mut annotations, &image, vec![]);
        let start = screen_at(rect, &image, &ViewState::default(), [1., 1.]).unwrap();
        let target = screen_at(rect, &image, &ViewState::default(), [0.5, 1.]).unwrap();
        editor_frame(
            &ctx,
            &mut annotations,
            &image,
            vec![
                egui::Event::PointerMoved(start),
                egui::Event::PointerButton {
                    pos: start,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        editor_frame(
            &ctx,
            &mut annotations,
            &image,
            vec![egui::Event::PointerMoved(target)],
        );
        assert!(
            annotations.editor.gesture.is_some(),
            "The test must actually start a vertex gesture"
        );
        assert_eq!(annotations.marks[0].points, original);
        assert!(
            !annotations.calculation.is_busy(),
            "Dragging does not scan pixels"
        );
        if cancel {
            editor_frame(
                &ctx,
                &mut annotations,
                &image,
                vec![egui::Event::Key {
                    key: egui::Key::Escape,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
            );
        }
        editor_frame(
            &ctx,
            &mut annotations,
            &image,
            vec![egui::Event::PointerButton {
                pos: target,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        wait(&mut annotations);
        annotations.finish_candidate();
        if cancel {
            assert_eq!(annotations.marks[0].points, original);
            assert!(!annotations.history.can_undo());
        } else {
            assert!((annotations.marks[0].points[0][0] - 0.5).abs() < 1e-5);
            assert!(annotations.history.can_undo());
            annotations.undo_edit(false);
            assert_eq!(annotations.marks[0].points, original);
            assert!(
                !annotations.history.can_undo(),
                "One gesture contributes one edit"
            );
        }
        assert!(!annotations.editor.unfinished());
        assert!(annotations.candidate.is_none());
        assert_eq!(Arc::strong_count(&image), 1);
    }
}
#[test]
fn a_drag_delivered_as_one_input_batch_still_edits_its_original_handle() {
    let ctx = egui::Context::default();
    let image = image();
    let mut annotations = annotations(&image, Tool::EllipseRoi, vec![[1., 1.], [3., 3.]]);
    let rect = editor_frame(&ctx, &mut annotations, &image, vec![]);
    let start = screen_at(rect, &image, &ViewState::default(), [1., 1.]).unwrap();
    let target = screen_at(rect, &image, &ViewState::default(), [0.5, 1.]).unwrap();
    editor_frame(
        &ctx,
        &mut annotations,
        &image,
        vec![
            egui::Event::PointerMoved(start),
            egui::Event::PointerButton {
                pos: start,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::PointerMoved(target),
            egui::Event::PointerButton {
                pos: target,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
        ],
    );
    wait(&mut annotations);
    annotations.finish_candidate();
    assert!((annotations.marks[0].points[0][0] - 0.5).abs() < 1e-5);
    assert!(annotations.history.can_undo());
}
#[test]
fn clicking_near_a_handle_does_not_edit_and_a_drag_keeps_the_grab_offset_or_rolls_back() {
    for movement in [0, 1, 2] {
        let ctx = egui::Context::default();
        let image = image();
        let mut annotations = annotations(&image, Tool::EllipseRoi, vec![[1., 1.], [3., 3.]]);
        let original = annotations.marks[0].points.clone();
        let rect = editor_frame(&ctx, &mut annotations, &image, vec![]);
        let start =
            screen_at(rect, &image, &ViewState::default(), [1., 1.]).unwrap() + egui::vec2(9., 0.);
        let target = match movement {
            0 => start,
            1 => {
                screen_at(rect, &image, &ViewState::default(), [0.5, 1.]).unwrap()
                    + egui::vec2(9., 0.)
            }
            _ => rect.right_bottom(),
        };
        editor_frame(
            &ctx,
            &mut annotations,
            &image,
            vec![
                egui::Event::PointerMoved(start),
                egui::Event::PointerButton {
                    pos: start,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::PointerMoved(target),
                egui::Event::PointerButton {
                    pos: target,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        wait(&mut annotations);
        annotations.finish_candidate();
        if movement == 1 {
            assert!((annotations.marks[0].points[0][0] - 0.5).abs() < 1e-5);
            assert!(annotations.history.can_undo());
        } else {
            assert_eq!(annotations.marks[0].points, original);
            assert!(!annotations.history.can_undo());
        }
    }
}
