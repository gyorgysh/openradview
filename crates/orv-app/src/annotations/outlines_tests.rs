// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
fn image() -> Arc<Frame> {
    let mut frame = (*orv_core::study::demo_study().series[0].frames[0]).clone();
    frame.width = 5;
    frame.height = 5;
    frame.pixels = vec![10.; 25].into();
    Arc::new(frame)
}
fn annotations(frame: &Frame) -> Annotations {
    let mut annotations = Annotations::default();
    annotations.ensure_admitted().unwrap();
    annotations.bind_image(frame, &ViewState::default());
    annotations.marks.push(Mark {
        id: 0,
        revision: 0,
        image: annotations.image.clone().unwrap(),
        points: vec![[1., 1.], [3., 3.]],
        tool: Tool::EllipseRoi,
        label: "private status must not be captured".into(),
    });
    annotations
}
#[test]
fn captured_outlines_are_immutable_exact_plane_records_without_source_pixels_or_status_text() {
    let frame = image();
    let mut annotations = annotations(&frame);
    let mut other = annotations.marks[0].image.clone();
    other.frame = Some(1);
    annotations.marks.push(Mark {
        id: 1,
        revision: 1,
        image: other,
        points: vec![[0., 0.], [2., 2.]],
        tool: Tool::Distance,
        label: "other frame".into(),
    });
    let captured = annotations.outlines(&frame, &ViewState::default()).unwrap();
    assert_eq!(captured.marks.len(), 1);
    assert_eq!(captured.marks[0].number, 1);
    annotations.marks[0].points[0] = [0., 0.];
    assert_eq!(captured.marks[0].points, [[1., 1.], [3., 3.]]);
    captured
        .validate(&frame, &ViewState::default(), &|| false)
        .unwrap();
    assert!(
        captured
            .validate(
                &frame,
                &ViewState {
                    slice: 1,
                    ..Default::default()
                },
                &|| false
            )
            .is_err()
    );
    let json = serde_json::to_value(captured.marks()).unwrap();
    assert_eq!(
        json,
        serde_json::json!([{"number":1,"kind":"ellipse_roi","points":[[1.,1.],[3.,3.]]}])
    );
    assert_eq!(Arc::strong_count(&frame), 1);
}
#[test]
fn outline_pressure_keeps_current_work_and_admission_lasts_until_all_owners_drop() {
    let frame = image();
    let annotations = annotations(&frame);
    let budget = MemoryBudget::new(OUTLINE_BYTES - 1);
    assert!(
        annotations
            .outlines_with_budget(&frame, &ViewState::default(), &budget)
            .is_err()
    );
    assert_eq!(budget.used(), 0);
    assert_eq!(annotations.marks[0].points, [[1., 1.], [3., 3.]]);
    budget.set_limit(OUTLINE_BYTES);
    let captured = Arc::new(
        annotations
            .outlines_with_budget(&frame, &ViewState::default(), &budget)
            .unwrap(),
    );
    let worker = captured.clone();
    drop(captured);
    assert_eq!(budget.used(), OUTLINE_BYTES);
    drop(worker);
    assert_eq!(budget.used(), 0);
    let empty = Annotations::default()
        .outlines_with_budget(&frame, &ViewState::default(), &MemoryBudget::new(0))
        .unwrap();
    assert!(empty.marks().is_empty());
    assert!(
        empty
            .validate(&frame, &ViewState::default(), &|| true)
            .is_err()
    );
}
#[test]
fn drafts_and_ready_private_candidates_cannot_be_exported_as_completed_marks() {
    let frame = image();
    let mut annotations = annotations(&frame);
    annotations.pending = vec![[1., 1.]];
    assert!(annotations.outlines(&frame, &ViewState::default()).is_err());
    annotations.pending.clear();
    annotations.candidate = Some(Candidate {
        id: 4,
        revision: 4,
        image: annotations.image.clone().unwrap(),
        points: vec![[1., 1.], [4., 4.]],
        tool: Tool::EllipseRoi,
        result: Some(Ok(stats_worker::Report {
            geometry_valid: true,
            text: "ready".into(),
        })),
        ready_repainted: false,
        base_revision: None,
    });
    assert!(annotations.outlines(&frame, &ViewState::default()).is_err());
    annotations.cancel_draft();
    assert_eq!(
        annotations
            .outlines(&frame, &ViewState::default())
            .unwrap()
            .marks
            .len(),
        1
    );
}
#[test]
fn worker_validation_rejects_changed_reference_missing_anchors_and_crossing_geometry() {
    let frame = image();
    let mut annotations = annotations(&frame);
    let captured = annotations.outlines(&frame, &ViewState::default()).unwrap();
    let mut changed = (*frame).clone();
    changed.geometry.as_mut().unwrap().frame_of_reference =
        Some("other generated reference".into());
    assert!(
        captured
            .validate(&changed, &ViewState::default(), &|| false)
            .is_err()
    );
    changed = (*frame).clone();
    let mut mask = vec![1; 25];
    mask[6] = 0;
    changed.validity = Some(mask.into());
    assert!(
        captured
            .validate(&changed, &ViewState::default(), &|| false)
            .is_err()
    );
    annotations.marks[0].tool = Tool::PolygonRoi;
    annotations.marks[0].points = vec![[0., 0.], [4., 4.], [4., 0.], [0., 4.]];
    let captured = annotations.outlines(&frame, &ViewState::default()).unwrap();
    assert!(
        captured
            .validate(&frame, &ViewState::default(), &|| false)
            .unwrap_err()
            .contains("cross")
    );
}
#[test]
fn illustration_paint_contains_numbered_outlines_without_selection_handles_or_status() {
    let ctx = egui::Context::default();
    theme::install(&ctx);
    let frame = image();
    let captured = annotations(&frame)
        .outlines(&frame, &ViewState::default())
        .unwrap();
    let mut output = ctx.run_ui(Default::default(), |ui| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(400., 400.), egui::Sense::hover());
        captured.paint(&ui.painter_at(rect), rect, &frame, &ViewState::default());
    });
    let text: Vec<_> = output
        .shapes
        .iter()
        .filter_map(|shape| {
            if let egui::Shape::Text(text) = &shape.shape {
                Some(text.galley.job.text.as_str())
            } else {
                None
            }
        })
        .collect();
    assert_eq!(text, ["#1"]);
    assert!(
        !output
            .shapes
            .iter()
            .any(|s| matches!(s.shape, egui::Shape::Circle(_)))
    );
    output.textures_delta.clear();
}

#[test]
fn projected_ellipse_limit_is_checked_before_tessellation() {
    let image = image();
    let mut annotations = annotations(&image);
    annotations.marks[0].points = vec![[0., 0.], [4., 4.]];
    let mut captured = annotations.outlines(&image, &ViewState::default()).unwrap();
    for number in 2..=128 {
        captured.marks.push(Outline {
            number,
            kind: SavedTool::EllipseRoi,
            points: vec![[0., 0.], [4., 4.]],
        });
    }
    let rect = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1024., 1024.));
    captured
        .check_drawing_size(rect, &image, &ViewState::default())
        .unwrap();
    assert!(
        captured
            .check_drawing_size(
                rect,
                &image,
                &ViewState {
                    zoom: 50.,
                    ..Default::default()
                }
            )
            .unwrap_err()
            .contains("Zoom out")
    );
}
