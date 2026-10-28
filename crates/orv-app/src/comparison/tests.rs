// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use std::time::{Duration, Instant};

fn study() -> Arc<Study> {
    let mut study = orv_core::study::demo_study();
    study.series[0].frames = crate::mpr_worker::tests::frames();
    let mut second = study.series[0].clone();
    second.number = 2;
    second.frames.reverse();
    study.series.push(second);
    Arc::new(study)
}
fn wait(session: &mut ComparisonSession) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        for pane in &mut session.panes {
            pane.poll();
        }
        if session
            .panes
            .iter()
            .all(|pane| pane.image.is_some() && pane.geometry_task.is_none())
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "Comparison workers did not finish"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}
#[test]
fn hover_preserves_active_tools_and_link_feedback_does_not_shift_the_canvas() {
    let ctx = egui::Context::default();
    let mut activated = false;
    let mut draw = |events| {
        let mut output = ctx.run_ui(
            egui::RawInput {
                events,
                ..Default::default()
            },
            |ui| {
                let (_, response) =
                    ui.allocate_exact_size(egui::vec2(200., 200.), egui::Sense::click_and_drag());
                activated |= activates_pane(ui, &response);
            },
        );
        output.textures_delta.clear();
    };
    let pos = egui::pos2(100., 100.);
    draw(vec![]);
    draw(vec![egui::Event::PointerMoved(pos)]);
    assert!(!activated);
    for pressed in [true, false] {
        let mut output = ctx.run_ui(
            egui::RawInput {
                events: vec![egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            },
            |ui| {
                let (_, response) =
                    ui.allocate_exact_size(egui::vec2(200., 200.), egui::Sense::click_and_drag());
                activated |= activates_pane(ui, &response);
            },
        );
        output.textures_delta.clear();
    }
    assert!(activated);
    let mut session = ComparisonSession::start(study(), 0, &ctx).unwrap();
    wait(&mut session);
    for width in [300., 900.] {
        let mut before = None;
        for notice in [
            None,
            Some("The source image center is outside the other series. Its image is unchanged."),
        ] {
            session.notice = notice;
            let mut cursor = None;
            let mut output = ctx.run_ui(Default::default(), |ui| {
                ui.set_width(width);
                session.link_status(ui);
                cursor = Some(ui.cursor());
            });
            output.textures_delta.clear();
            if let Some(before) = before {
                assert_eq!(cursor, before);
            }
            before = Some(cursor);
        }
    }
}
#[test]
fn independent_panes_share_source_pixels_link_physically_and_export_only_the_active_image() {
    let ctx = egui::Context::default();
    let study = study();
    let mut session = ComparisonSession::start(study.clone(), 0, &ctx).unwrap();
    wait(&mut session);
    assert!(session.compatible());
    assert!(Arc::ptr_eq(
        &session.panes[0].image.as_ref().unwrap().1,
        &study.series[0].frames[2]
    ));
    assert!(Arc::ptr_eq(
        &session.panes[1].image.as_ref().unwrap().1,
        &study.series[1].frames[2]
    ));
    session.panes[0].view.slice = 0;
    assert_eq!(
        session.panes[1].view.slice, 2,
        "Navigation starts independent"
    );
    session.linked = true;
    session.synchronize(0);
    assert_eq!(
        session.panes[1].view.slice, 3,
        "Reverse source order must use physical position"
    );
    session.panes[1].worker.request(3);
    let deadline = Instant::now() + Duration::from_secs(5);
    while session.panes[1].image.as_ref().unwrap().0 != 3 {
        session.panes[1].poll();
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
    session.active = 1;
    session.panes[1].last_view = Some(session.panes[1].view.clone());
    session.panes[1].picture_size = Some([400, 240]);
    let (index, spec, pixels, size) = session.picture().unwrap();
    assert_eq!((index, spec.view.slice, size), (1, 3, [400, 240]));
    let crate::export::Pixels::Image(image) = pixels else {
        panic!("Comparison exports one source image");
    };
    assert!(Arc::ptr_eq(&image, &study.series[1].frames[3]));
    session.panes[1].view.invert = true;
    assert!(
        session.picture().is_none(),
        "Export waits for the changed display to be painted"
    );
}
#[test]
fn source_replacement_cancels_old_work_and_closing_releases_all_image_references() {
    let ctx = egui::Context::default();
    let study = study();
    let weak = Arc::downgrade(&study.series[0].frames[0]);
    let mut session = ComparisonSession::start(study, 0, &ctx).unwrap();
    wait(&mut session);
    session.linked = true;
    assert!(session.replace_active(1, &ctx));
    assert!(!session.linked);
    assert!(session.panes[0].image.is_none() && session.panes[0].geometry.is_none());
    wait(&mut session);
    assert_eq!(session.panes[0].series, 1);
    assert!(Arc::ptr_eq(
        &session.panes[0].image.as_ref().unwrap().1,
        &session.panes[1].image.as_ref().unwrap().1
    ));
    drop(session);
    let deadline = Instant::now() + Duration::from_secs(5);
    while weak.upgrade().is_some() {
        assert!(
            Instant::now() < deadline,
            "Closed comparison retained source images"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn single_series_can_compare_a_separate_source_without_crossing_pane_identity() {
    let ctx = egui::Context::default();
    let original = Arc::new(orv_core::study::demo_study());
    let mut other = orv_core::study::demo_study();
    other.series[0].number = 99;
    let other = Arc::new(other);
    let mut session = ComparisonSession::start(original.clone(), 0, &ctx).unwrap();
    assert!(session.open_right(other.clone(), &ctx));
    wait(&mut session);
    assert!(Arc::ptr_eq(session.active_study(), &other));
    assert_eq!(session.active_source().number, 99);
    session.panes[1].last_view = Some(session.panes[1].view.clone());
    session.panes[1].picture_size = Some([300, 200]);
    let (_, _, crate::export::Pixels::Image(image), _) = session.picture().unwrap() else {
        panic!("The active source image should be exported");
    };
    assert!(Arc::ptr_eq(
        &image,
        &other.series[0].frames[session.panes[1].view.slice as usize]
    ));
    session.active = 0;
    assert!(Arc::ptr_eq(session.active_study(), &original));
    assert_ne!(session.active_source().number, 99);
}

#[test]
fn unsaved_tracking_covers_measurements_and_views_after_saving() {
    let ctx = egui::Context::default();
    let mut session = ComparisonSession::start(study(), 0, &ctx).unwrap();
    assert!(!session.unsaved());
    session.add_test_measurement(0);
    assert!(session.unsaved());
    session.saved_marks[0] = session.panes[0]
        .annotations
        .capture()
        .unwrap()
        .into_iter()
        .map(|m| m.saved)
        .collect();
    session.saved_views = Some(session.panes.each_ref().map(|pane| pane.view.clone()));
    assert!(!session.unsaved());
    session.panes[0].annotations.clear();
    assert!(
        session.unsaved(),
        "Deleting a saved mark is an unsaved change"
    );
    session.add_test_measurement(0);
    assert!(!session.unsaved());
    session.panes[0].view.invert = true;
    assert!(session.unsaved());
    session.discard_marks();
    assert!(!session.unsaved());
    session.saved_views = Some(session.panes.each_ref().map(|pane| pane.view.clone()));
    assert!(session.open_right(study(), &ctx));
    assert!(
        session.unsaved(),
        "Replacing a saved source changes the comparison"
    );
}

#[test]
fn adopted_source_view_keeps_its_selected_frame_and_window_after_decode() {
    let ctx = egui::Context::default();
    let mut session = ComparisonSession::start(study(), 0, &ctx).unwrap();
    let view = ViewState {
        slice: 0,
        invert: true,
        window: orv_core::WindowLevel::BONE,
        ..Default::default()
    };
    session.adopt_source_view(Annotations::default(), view.clone(), Tool::Angle, false);
    wait(&mut session);
    assert_eq!(session.panes[0].view, view);
    assert_eq!(session.panes[0].image.as_ref().unwrap().0, 0);
    assert_eq!(session.panes[0].tool, Tool::Angle);
}
