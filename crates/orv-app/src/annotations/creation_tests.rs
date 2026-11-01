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
fn draft(image: &Frame, tool: Tool, points: Vec<[f64; 2]>) -> Annotations {
    let mut annotations = Annotations::default();
    annotations.bind_image(image, &ViewState::default());
    annotations.active_tool = Some(tool);
    annotations.pending = points;
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
#[test]
fn a_region_remains_private_until_its_matching_canvas_accepts_the_validated_candidate() {
    let ctx = egui::Context::default();
    let image = image();
    let mut annotations = draft(&image, Tool::EllipseRoi, vec![[1., 1.], [3., 3.]]);
    annotations.complete_draft(&image, &ViewState::default(), Tool::EllipseRoi, &ctx);
    assert!(annotations.marks.is_empty());
    assert!(annotations.capture().is_err());
    wait(&mut annotations);
    assert!(annotations.marks.is_empty());
    assert!(annotations.candidate.as_ref().unwrap().result.is_some());
    annotations.finish_candidate();
    assert_eq!(annotations.marks.len(), 1);
    assert!(annotations.pending.is_empty());
    assert!(annotations.marks[0].label.starts_with("Ellipse ROI · n=5"));
    assert!(annotations.marks[0].label.contains("30.00 mm² sampled"));
    assert!(annotations.marks[0].label.contains("18.85 mm² shape"));
    assert_eq!(Arc::strong_count(&image), 1);
}
#[test]
fn invalid_polygon_returns_to_the_draft_and_can_be_repaired_without_publishing_a_bad_mark() {
    let ctx = egui::Context::default();
    let image = image();
    let crossing = vec![[0., 0.], [4., 4.], [0., 4.], [4., 0.]];
    let mut annotations = draft(&image, Tool::PolygonRoi, crossing.clone());
    annotations.complete_draft(&image, &ViewState::default(), Tool::PolygonRoi, &ctx);
    wait(&mut annotations);
    annotations.finish_candidate();
    assert!(annotations.marks.is_empty());
    assert_eq!(annotations.pending, crossing);
    assert!(annotations.draft_error.as_ref().unwrap().contains("cross"));
    annotations.pending = vec![[0., 0.], [4., 0.], [0., 4.]];
    annotations.complete_draft(&image, &ViewState::default(), Tool::PolygonRoi, &ctx);
    wait(&mut annotations);
    annotations.finish_candidate();
    assert_eq!(annotations.marks.len(), 1);
    assert_eq!(annotations.marks[0].id, 1);
    assert!(annotations.marks[0].label.starts_with("Polygon ROI · n=15"));
    assert!(annotations.draft_error.is_none());
}
#[test]
fn changing_image_context_rejects_even_an_already_completed_candidate() {
    let ctx = egui::Context::default();
    let image = image();
    let mut annotations = draft(&image, Tool::EllipseRoi, vec![[1., 1.], [3., 3.]]);
    annotations.complete_draft(&image, &ViewState::default(), Tool::EllipseRoi, &ctx);
    wait(&mut annotations);
    annotations.bind_image(
        &image,
        &ViewState {
            slice: 1,
            ..Default::default()
        },
    );
    annotations.finish_candidate();
    assert!(annotations.marks.is_empty());
    assert!(annotations.pending.is_empty());
    assert!(annotations.candidate.is_none());
    assert!(!annotations.calculation.is_busy());
    assert_eq!(Arc::strong_count(&image), 1);
}
#[test]
fn collection_point_budget_is_checked_before_starting_another_region_job() {
    let ctx = egui::Context::default();
    let image = image();
    let mut annotations = draft(&image, Tool::Roi, vec![[1., 1.], [3., 3.]]);
    let points = (0..256)
        .map(|i| {
            let a = i as f64 * std::f64::consts::TAU / 256.;
            [2. + a.cos(), 2. + a.sin()]
        })
        .collect::<Vec<_>>();
    annotations.marks = (0..16)
        .map(|id| Mark {
            id,
            revision: id,
            image: annotations.image.clone().unwrap(),
            points: points.clone(),
            tool: Tool::PolygonRoi,
            label: "Generated region".into(),
        })
        .collect();
    annotations.complete_draft(&image, &ViewState::default(), Tool::Roi, &ctx);
    assert_eq!(annotations.marks.len(), 16);
    assert!(annotations.candidate.is_none());
    assert!(!annotations.calculation.is_busy());
    assert!(
        annotations
            .draft_error
            .as_ref()
            .unwrap()
            .contains("allowance")
    );
}
fn key(key: egui::Key) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }
}
#[test]
fn polygon_enter_belongs_to_the_canvas_and_escape_cancels_pending_validation() {
    let ctx = egui::Context::default();
    let image = image();
    let mut annotations = draft(&image, Tool::PolygonRoi, vec![[0., 0.], [4., 0.], [0., 4.]]);
    let mut input = String::new();
    let mut output = ctx.run_ui(
        egui::RawInput {
            events: vec![key(egui::Key::Enter)],
            ..Default::default()
        },
        |ui| {
            ui.text_edit_singleline(&mut input).request_focus();
            let (_, response) =
                ui.allocate_exact_size(egui::vec2(400., 400.), egui::Sense::click_and_drag());
            annotations.handle_creation(
                ui,
                &response,
                &image,
                &ViewState::default(),
                Tool::PolygonRoi,
            );
        },
    );
    output.textures_delta.clear();
    assert_eq!(annotations.pending.len(), 3);
    assert!(annotations.candidate.is_none());
    for action in [egui::Key::Enter, egui::Key::Escape] {
        let mut output = ctx.run_ui(
            egui::RawInput {
                events: vec![key(action)],
                ..Default::default()
            },
            |ui| {
                let (_, response) =
                    ui.allocate_exact_size(egui::vec2(400., 400.), egui::Sense::click_and_drag());
                response.request_focus();
                annotations.handle_creation(
                    ui,
                    &response,
                    &image,
                    &ViewState::default(),
                    Tool::PolygonRoi,
                );
            },
        );
        output.textures_delta.clear();
        if action == egui::Key::Enter {
            assert!(annotations.candidate.is_some());
        }
    }
    assert!(annotations.candidate.is_none());
    assert!(annotations.pending.is_empty());
    assert!(annotations.marks.is_empty());
}
#[test]
fn a_ready_candidate_behind_a_disabled_canvas_does_not_repaint_continuously() {
    let ctx = egui::Context::default();
    let image = image();
    let mut annotations = draft(&image, Tool::EllipseRoi, vec![[1., 1.], [3., 3.]]);
    annotations.complete_draft(&image, &ViewState::default(), Tool::EllipseRoi, &ctx);
    wait(&mut annotations);
    let mut delay = Duration::ZERO;
    for _ in 0..5 {
        let mut output = ctx.run_ui(Default::default(), |ui| {
            ui.disable();
            annotations.status_overlay(
                ui,
                Rect::from_min_size(Pos2::ZERO, egui::vec2(500., 500.)),
                Tool::EllipseRoi,
            );
        });
        delay = output.viewport_output[&egui::ViewportId::ROOT].repaint_delay;
        output.textures_delta.clear();
    }
    assert!(annotations.candidate.is_some());
    assert!(delay > Duration::from_secs(1));
}
#[test]
fn point_picking_and_file_restore_share_exact_finite_sampled_image_bounds() {
    let mut image = (*image()).clone();
    for point in [[0., 0.], [4., 4.], [0.25, 3.75]] {
        assert!(sampled_point(&image, point));
    }
    for point in [
        [-0.01, 0.],
        [0., -0.01],
        [4.01, 4.],
        [4., 4.01],
        [f64::NAN, 1.],
        [1., f64::INFINITY],
    ] {
        assert!(!sampled_point(&image, point));
    }
    let mut mask = vec![1; 25];
    mask[0] = 0;
    image.validity = Some(mask.into());
    assert!(!sampled_point(&image, [0., 0.]));
}
#[test]
fn double_click_fit_cannot_remap_a_polygon_click_but_remains_available_for_pan() {
    for tool in [Tool::Pan, Tool::PolygonRoi] {
        let ctx = egui::Context::default();
        let mut view = ViewState {
            zoom: 2.,
            pan: [0.1, 0.2],
            ..Default::default()
        };
        let mut scroll = 0.;
        let mut double = false;
        let position = egui::pos2(200., 200.);
        let events = [None, Some(true), Some(false), Some(true), Some(false)];
        for (frame, pressed) in events.into_iter().enumerate() {
            let mut input = vec![egui::Event::PointerMoved(position)];
            if let Some(pressed) = pressed {
                input.push(egui::Event::PointerButton {
                    pos: position,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                });
            }
            let mut output = ctx.run_ui(
                egui::RawInput {
                    time: Some(frame as f64 * 0.05),
                    events: input,
                    ..Default::default()
                },
                |ui| {
                    let (_, response) = ui
                        .allocate_exact_size(egui::vec2(400., 400.), egui::Sense::click_and_drag());
                    double |= response.double_clicked();
                    crate::viewport::interact(ui, &response, &mut view, tool, 5, &mut scroll);
                },
            );
            output.textures_delta.clear();
        }
        assert!(double);
        if tool == Tool::Pan {
            assert_eq!(view.zoom, 1.);
            assert_eq!(view.pan, [0., 0.]);
        } else {
            assert_eq!(view.zoom, 2.);
            assert_eq!(view.pan, [0.1, 0.2]);
        }
    }
}
#[test]
fn a_view_mode_change_discards_a_completed_private_candidate_and_releases_canvas_focus() {
    let ctx = egui::Context::default();
    let mut study = orv_core::study::demo_study();
    study.series[0].frames = crate::mpr_worker::tests::frames();
    let image = study.series[0].frames[0].clone();
    let mut annotations = draft(&image, Tool::EllipseRoi, vec![[0., 0.], [2., 2.]]);
    let id = egui::Id::new("test-draft-canvas");
    annotations.canvas_id = Some(id);
    ctx.memory_mut(|memory| memory.request_focus(id));
    annotations.complete_draft(&image, &ViewState::default(), Tool::EllipseRoi, &ctx);
    wait(&mut annotations);
    let mut workspace = crate::workspace::Workspace::default();
    workspace.study.replace(Arc::new(study));
    std::mem::swap(&mut workspace.annotations, &mut annotations);
    workspace.set_mode(crate::workspace::ViewMode::Mpr, &ctx);
    assert!(workspace.annotations.candidate.is_none());
    assert!(workspace.annotations.marks.is_empty());
    assert!(!ctx.memory(|memory| memory.has_focus(id)));
}

#[test]
fn typed_points_work_behind_their_menu_and_validate_the_same_bounds_and_region() {
    let ctx = egui::Context::default();
    let image = image();
    let view = ViewState::default();
    let mut annotations = draft(&image, Tool::EllipseRoi, Vec::new());
    let popup = egui::Id::new("typed-measurement-menu");
    egui::Popup::open_id(&ctx, popup);
    for pixel in [[-1., 0.], [1., 1.], [3., 3.]] {
        annotations.point_input.request = Some(pixel);
        assert!(annotations.capture().is_err());
        assert!(annotations.outlines(&image, &view).is_err());
        assert!(annotations.source_outlines().is_err());
        let mut output = ctx.run_ui(Default::default(), |ui| {
            let (_, response) =
                ui.allocate_exact_size(egui::vec2(400., 400.), egui::Sense::click_and_drag());
            annotations.show(ui, &response, &image, &view, Tool::EllipseRoi);
        });
        output.textures_delta.clear();
        assert!(annotations.point_input.request.is_none());
        if pixel[0] < 0. {
            assert!(annotations.pending.is_empty());
            assert!(
                annotations
                    .draft_error
                    .as_ref()
                    .unwrap()
                    .contains("valid image sample")
            );
        }
    }
    assert!(annotations.marks.is_empty());
    assert!(annotations.candidate.is_some());
    wait(&mut annotations);
    annotations.finish_candidate();
    assert_eq!(annotations.marks.len(), 1);
    assert_eq!(annotations.marks[0].points, vec![[1., 1.], [3., 3.]]);
    assert!(annotations.marks[0].label.starts_with("Ellipse ROI · n=5"));
    assert!(annotations.marks[0].label.contains("18.85 mm² shape"));
    assert_eq!(Arc::strong_count(&image), 1);
    annotations.point_input.request = Some([2., 2.]);
    annotations.bind_image(&image, &ViewState { slice: 1, ..view });
    assert!(
        annotations.point_input.request.is_none(),
        "A changed source clears queued input"
    );
    egui::Popup::close_all(&ctx);
}

#[test]
fn typed_region_completion_restores_add_focus_without_stealing_another_controls_focus() {
    fn render(
        ctx: &egui::Context,
        annotations: &mut Annotations,
        events: Vec<egui::Event>,
    ) -> egui::accesskit::TreeUpdate {
        let mut output = ctx.run_ui(
            egui::RawInput {
                events,
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(800., 700.))),
                ..Default::default()
            },
            |ui| {
                annotations.point_controls(ui);
                let _ = ui.button("Other control");
            },
        );
        output.textures_delta.clear();
        output.platform_output.accesskit_update.take().unwrap()
    }
    fn focus(target_node: egui::accesskit::NodeId) -> egui::Event {
        egui::Event::AccessKitActionRequest(egui::accesskit::ActionRequest {
            action: egui::accesskit::Action::Focus,
            target_tree: egui::accesskit::TreeId::ROOT,
            target_node,
            data: None,
        })
    }
    for move_focus in [false, true] {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let image = image();
        let mut annotations = draft(&image, Tool::EllipseRoi, vec![[1., 1.]]);
        annotations.point_input.coordinates = [4., 4.];
        let tree = render(&ctx, &mut annotations, vec![]);
        let node = |name: &str| {
            tree.nodes
                .iter()
                .find_map(|(id, node)| {
                    (node.role() == egui::accesskit::Role::Button && node.label() == Some(name))
                        .then_some(*id)
                })
                .unwrap()
        };
        let add = node("Add point to measurement");
        let other = node("Other control");
        render(&ctx, &mut annotations, vec![focus(add)]);
        render(&ctx, &mut annotations, vec![key(egui::Key::Enter)]);
        let pixel = annotations.point_input.request.take().unwrap();
        annotations.add_point(pixel, &image, &ViewState::default(), Tool::EllipseRoi, &ctx);
        assert!(annotations.candidate.is_some());
        let disabled = render(&ctx, &mut annotations, vec![]);
        assert_ne!(disabled.focus, add);
        if move_focus {
            render(&ctx, &mut annotations, vec![focus(other)]);
        }
        wait(&mut annotations);
        annotations.finish_candidate();
        let completed = render(&ctx, &mut annotations, vec![]);
        assert_eq!(annotations.marks.len(), 1);
        assert_eq!(completed.focus, if move_focus { other } else { add });
        assert!(annotations.point_input.focus_after_calculation.is_none());
    }
}
