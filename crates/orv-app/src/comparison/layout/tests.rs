// SPDX-License-Identifier: GPL-3.0-or-later
use super::super::tests as comparison_tests;
use super::*;
use std::time::{Duration, Instant};
fn four_study() -> Arc<Study> {
    let mut study = Arc::try_unwrap(comparison_tests::study()).ok().unwrap();
    for index in 0..2 {
        let mut source = study.series[index].clone();
        source.number = index as i32 + 3;
        study.series.push(source);
    }
    Arc::new(study)
}
fn four() -> ComparisonSession {
    let ctx = egui::Context::default();
    let mut session = ComparisonSession::start(four_study(), 0, &ctx).unwrap();
    comparison_tests::wait(&mut session);
    session.add_extra(&ctx).unwrap();
    comparison_tests::wait(&mut session);
    session
}
#[test]
fn four_panes_choose_distinct_available_series_share_pixels_and_stop_at_four_workers() {
    let mut session = four();
    assert_eq!(
        session
            .panes
            .iter()
            .map(|pane| pane.series)
            .collect::<Vec<_>>(),
        vec![0, 1, 2, 3]
    );
    for pane in &session.panes {
        let (index, image) = pane.image.as_ref().unwrap();
        assert!(Arc::ptr_eq(
            image,
            &pane.study.series[pane.series].frames[*index]
        ));
        assert!(pane.geometry_task.is_none());
    }
    let sources = session
        .panes
        .iter()
        .map(|pane| Arc::downgrade(&pane.source))
        .collect::<Vec<_>>();
    session.add_extra(&egui::Context::default()).unwrap();
    assert_eq!(session.panes.len(), 4);
    for (before, after) in sources.iter().zip(&session.panes) {
        assert!(before.ptr_eq(&Arc::downgrade(&after.source)));
    }
}
#[test]
fn added_panes_keep_each_loaded_study_identity_and_active_export_ownership() {
    let ctx = egui::Context::default();
    let mut session = ComparisonSession::start(comparison_tests::study(), 0, &ctx).unwrap();
    let external = comparison_tests::study();
    assert!(session.open_right(external.clone(), &ctx));
    session.add_extra(&ctx).unwrap();
    comparison_tests::wait(&mut session);
    assert!(Arc::ptr_eq(
        &session.panes[2].study,
        &session.panes[0].study
    ));
    assert!(Arc::ptr_eq(&session.panes[3].study, &external));
    session.active = 3;
    assert!(Arc::ptr_eq(session.active_study(), &external));
    session.panes[3].last_view = Some(session.panes[3].view.clone());
    session.panes[3].picture_size = Some([320, 200]);
    let (series, _, crate::export::Pixels::Image(image), _) = session.picture().unwrap() else {
        panic!("Image expected")
    };
    assert_eq!(series, session.panes[3].series);
    assert!(Arc::ptr_eq(
        &image,
        &session.panes[3].image.as_ref().unwrap().1
    ));
}
#[test]
fn closing_extra_panes_drops_their_sources_workers_and_marks_and_resets_pairing() {
    let mut session = four();
    let weak = [
        Arc::downgrade(&session.panes[2].source),
        Arc::downgrade(&session.panes[3].source),
    ];
    session.add_test_measurement(2);
    assert!(session.unsaved());
    assert!(session.set_pair([2, 3]));
    session.active = 3;
    session.close_extra();
    assert_eq!(session.panes.len(), 2);
    assert_eq!(session.active, 1);
    assert_eq!(session.pair, [0, 1]);
    assert!(session.clear_extra_slots);
    assert!(!session.unsaved());
    let deadline = Instant::now() + Duration::from_secs(5);
    while weak.iter().any(|source| source.upgrade().is_some()) {
        assert!(
            Instant::now() < deadline,
            "Closed pane worker retained its source"
        );
        std::thread::yield_now();
    }
}
#[test]
fn extra_source_changes_preserve_two_pane_saved_baselines_and_four_pane_files_are_refused() {
    let mut session = four();
    let views = std::array::from_fn(|index| session.panes[index].view.clone());
    session.saved_views = Some(views);
    session.active = 2;
    session.add_test_measurement(2);
    assert!(session.active_unsaved());
    assert!(session.replace_active(3, &egui::Context::default()));
    assert!(session.saved_views.is_some());
    assert!(!session.pane_unsaved(0) && !session.pane_unsaved(1));
    session.begin_save(&egui::Context::default());
    assert!(session.file_task.is_none());
    assert!(session.file_notice.unwrap().contains("two panes"));
    session.begin_open(&egui::Context::default());
    assert!(session.file_task.is_none());
    let source = session.panes[1].source.clone();
    assert!(!session.open_right(comparison_tests::study(), &egui::Context::default()));
    assert!(Arc::ptr_eq(&source, &session.panes[1].source));
}
#[test]
fn selecting_any_valid_pair_links_only_its_physical_slices_and_refuses_invalid_pairs() {
    let mut session = four();
    assert!(!session.set_pair([0, 4]));
    assert!(!session.set_pair([2, 2]));
    assert!(session.set_pair([2, 3]));
    assert!(session.compatible());
    let first = [session.panes[0].view.clone(), session.panes[1].view.clone()];
    session.panes[2].view.slice = 0;
    session.synchronize(2);
    assert_eq!(session.panes[3].view.slice, 3);
    assert_eq!(session.panes[0].view, first[0]);
    assert_eq!(session.panes[1].view, first[1]);
    session.synchronize(0);
    assert_eq!(session.panes[3].view.slice, 3);
}
#[test]
fn four_grid_uses_two_rows_without_overlap_and_two_grid_stacks_only_when_tall() {
    for (count, size, expected) in [
        (4, egui::vec2(600., 300.), [2, 2]),
        (4, egui::vec2(1000., 600.), [2, 2]),
        (2, egui::vec2(600., 300.), [2, 1]),
        (2, egui::vec2(400., 700.), [1, 2]),
        (2, egui::vec2(1000., 600.), [2, 1]),
    ] {
        let gap = egui::vec2(8., 12.);
        let layout = grid(count, size, gap);
        assert_eq!([layout.columns, layout.rows], expected);
        let mut boxes = Vec::new();
        for row in 0..layout.rows {
            for column in 0..layout.columns {
                let origin = egui::vec2(
                    column as f32 * (layout.pane.x + gap.x),
                    row as f32 * (layout.pane.y + gap.y),
                );
                let rect = egui::Rect::from_min_size(egui::Pos2::ZERO + origin, layout.pane);
                assert!(egui::Rect::from_min_size(egui::Pos2::ZERO, size).contains_rect(rect));
                assert!(
                    boxes
                        .iter()
                        .all(|other: &egui::Rect| !rect.intersects(*other))
                );
                boxes.push(rect);
            }
        }
        assert_eq!(boxes.len(), count);
    }
}
#[test]
fn four_pane_controls_fit_a_compact_content_area_and_idle_without_repainting() {
    let mut session = four();
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    crate::theme::install(&ctx);
    let frame = eframe::Frame::_new_kittest();
    let mut last = None;
    let mut delay = Duration::ZERO;
    for pass in 0..6 {
        let mut output = ctx.run_ui(
            egui::RawInput {
                time: Some(pass as f64),
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(600., 400.),
                )),
                ..Default::default()
            },
            |ui| {
                session.ui(ui, &frame, true);
            },
        );
        delay = output.viewport_output[&egui::ViewportId::ROOT].repaint_delay;
        output.textures_delta.clear();
        last = output.platform_output.accesskit_update.take();
    }
    let tree = last.unwrap();
    let sliders = tree
        .nodes
        .iter()
        .filter(|(_, node)| node.role() == egui::accesskit::Role::Slider)
        .map(|(_, node)| node.bounds().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(sliders.len(), 4);
    for bounds in sliders {
        assert!(
            bounds.x0 >= 0. && bounds.y0 >= 0. && bounds.x1 <= 600. && bounds.y1 <= 400.,
            "Slider outside compact view: {bounds:?}"
        );
    }
    for pane in &session.panes {
        let size = pane.picture_size.unwrap();
        assert!(
            size[0] >= 200 && size[1] >= 35,
            "Canvas too small: {size:?}"
        );
    }
    assert!(delay > Duration::from_secs(1), "Idle repaint: {delay:?}");
}

fn render(
    session: &mut ComparisonSession,
    ctx: &egui::Context,
    events: Vec<egui::Event>,
) -> egui::accesskit::TreeUpdate {
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1000., 700.),
            )),
            events,
            ..Default::default()
        },
        |ui| {
            ui.add_enabled_ui(!session.busy(), |ui| {
                session.ui(ui, &eframe::Frame::_new_kittest(), true);
            });
            session.file_dialogs(ui.ctx());
        },
    );
    output.textures_delta.clear();
    output.platform_output.accesskit_update.take().unwrap()
}
fn click_node(tree: &egui::accesskit::TreeUpdate, label: &str) -> egui::Event {
    let target_node = tree
        .nodes
        .iter()
        .find_map(|(id, node)| (node.label() == Some(label)).then_some(*id))
        .unwrap();
    egui::Event::AccessKitActionRequest(egui::accesskit::ActionRequest {
        action: egui::accesskit::Action::Click,
        target_tree: egui::accesskit::TreeId::ROOT,
        target_node,
        data: None,
    })
}
#[test]
fn keeping_extra_panes_preserves_unsaved_marks_and_only_explicit_closure_discards_them() {
    let mut session = four();
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    crate::theme::install(&ctx);
    session.add_test_measurement(3);
    session.active = 3;
    session.set_pair([2, 3]);
    let source = Arc::downgrade(&session.panes[3].source);
    let tree = render(&mut session, &ctx, vec![]);
    let click = click_node(&tree, "Two panes");
    let tree = render(&mut session, &ctx, vec![click]);
    assert!(session.busy());
    assert!(session.unsaved());
    let click = click_node(&tree, "Keep four panes");
    render(&mut session, &ctx, vec![click]);
    assert!(!session.busy());
    assert_eq!(session.panes.len(), 4);
    assert_eq!(session.active, 3);
    assert_eq!(session.pair, [2, 3]);
    assert!(session.panes[3].annotations.has_marks());
    assert!(source.ptr_eq(&Arc::downgrade(&session.panes[3].source)));
    let tree = render(&mut session, &ctx, vec![]);
    let click = click_node(&tree, "Two panes");
    let tree = render(&mut session, &ctx, vec![click]);
    let click = click_node(&tree, "Close panes 3 and 4");
    render(&mut session, &ctx, vec![click]);
    assert_eq!(session.panes.len(), 2);
    assert_eq!(session.active, 1);
    assert_eq!(session.pair, [0, 1]);
    assert!(!session.busy() && !session.unsaved());
}
#[test]
fn choosing_an_extra_pair_forwards_display_only_to_that_pair_and_pair_changes_clear_opt_ins() {
    let mut session = four();
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    crate::theme::install(&ctx);
    assert!(session.set_pair([2, 3]));
    session.active = 2;
    session.panes[2].view.pan = [0.2, 0.];
    let first = [session.panes[0].view.clone(), session.panes[1].view.clone()];
    let tree = render(&mut session, &ctx, vec![]);
    let click = click_node(&tree, "Links and reference · 3 + 4");
    let tree = render(&mut session, &ctx, vec![click]);
    let click = click_node(&tree, "Link physical pan");
    render(&mut session, &ctx, vec![click]);
    assert!((session.panes[3].view.pan[0] - 0.2).abs() < 1e-6);
    assert_eq!(session.panes[0].view, first[0]);
    assert_eq!(session.panes[1].view, first[1]);
    let tree = render(&mut session, &ctx, vec![]);
    let click = click_node(&tree, "1 + 3");
    let tree = render(&mut session, &ctx, vec![click]);
    assert_eq!(session.pair, [0, 2]);
    let checked = tree
        .nodes
        .iter()
        .find(|(_, node)| node.label() == Some("Link physical pan"))
        .unwrap()
        .1
        .toggled();
    assert_eq!(checked, Some(egui::accesskit::Toggled::False));
    assert_eq!(session.panes[3].view.pan[0], 0.2);
}
#[test]
fn hidden_display_controls_keep_shortcuts_scoped_to_the_active_pane_and_text_focus() {
    let mut session = four();
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    crate::theme::install(&ctx);
    session.active = 3;
    assert!(!session.show_display);
    render(&mut session, &ctx, vec![]);
    let key = |key| egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    render(&mut session, &ctx, vec![key(egui::Key::Z)]);
    assert_eq!(session.panes[3].tool, Tool::Zoom);
    assert!(session.panes[..3].iter().all(|pane| pane.tool == Tool::Pan));
    let before = session.panes[3].view.slice;
    render(&mut session, &ctx, vec![key(egui::Key::ArrowDown)]);
    assert_eq!(session.panes[3].view.slice, before + 1);
    ctx.memory_mut(|memory| memory.request_focus(egui::Id::new("other-control")));
    // The normal input owner gate also applies when the toolbar is hidden.
    let mut output = ctx.run_ui(
        egui::RawInput {
            events: vec![key(egui::Key::P)],
            ..Default::default()
        },
        |ui| {
            let _ = ui.add(
                egui::TextEdit::singleline(&mut String::new()).id(egui::Id::new("other-control")),
            );
            session.panes[3].input(ui);
        },
    );
    output.textures_delta.clear();
    assert_eq!(session.panes[3].tool, Tool::Zoom);
}
