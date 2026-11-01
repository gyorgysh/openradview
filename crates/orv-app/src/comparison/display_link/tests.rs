// SPDX-License-Identifier: GPL-3.0-or-later
use super::super::{ComparisonSession, tests as comparison_tests};
use super::*;
use orv_core::{
    WindowLevel,
    scalar::{ScalarData, ScalarInfo, Storage, Unit, ValueType, VoiChoices},
};
use std::time::{Duration, Instant};

fn session() -> ComparisonSession {
    let ctx = egui::Context::default();
    let mut study = Arc::try_unwrap(comparison_tests::study()).ok().unwrap();
    let scalar = ScalarData::new(
        ScalarInfo {
            value_type: ValueType::F32,
            storage: Storage::SignedInteger,
            stored_bits: 16,
            unit: Unit::Hounsfield,
            rescale: None,
            resampled: false,
        },
        VoiChoices::new(vec![], vec![]).unwrap(),
    )
    .unwrap();
    for series in &mut study.series {
        for frame in &mut series.frames {
            Arc::make_mut(frame).scalar = Some(scalar.clone());
        }
    }
    let mut session = ComparisonSession::start(Arc::new(study), 0, &ctx).unwrap();
    comparison_tests::wait(&mut session);
    session.panes[0].canvas_aspect = Some(1.);
    session.panes[1].canvas_aspect = Some(2.);
    session
}
fn views(session: &ComparisonSession) -> [ViewState; 2] {
    std::array::from_fn(|index| session.panes[index].view.clone())
}
fn update(
    session: &mut ComparisonSession,
    before: &[ViewState; 2],
    previous_active: usize,
    reconfigured: bool,
    enabled: bool,
) -> Update {
    let result = session.display_links.update(Input {
        panes: [&session.panes[0], &session.panes[1]],
        before,
        before_aspects: std::array::from_fn(|index| session.panes[index].canvas_aspect),
        active: session.active,
        previous_active,
        reconfigured,
        enabled,
    });
    if let Some((target, view)) = &result.view {
        session.panes[*target].view = view.clone();
    }
    result
}
fn finish_source(session: &mut ComparisonSession) {
    let requested = session.panes[0].view.slice as usize;
    session.panes[0].worker.request(requested);
    let deadline = Instant::now() + Duration::from_secs(5);
    while session.panes[0].image.as_ref().unwrap().0 != session.panes[0].view.slice as usize {
        session.panes[0].poll();
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
}
#[test]
fn actions_forward_once_with_physical_aspects_and_keep_target_orientation_inversion_and_slice() {
    let mut session = session();
    let before = views(&session);
    session.display_links.selection = Selection {
        pan: true,
        zoom: true,
        window: false,
    };
    session.panes[0].view.zoom = 2.;
    session.panes[0].view.pan = [0.1, -0.2];
    session.panes[1].view.invert = true;
    let target = session.panes[1].view.clone();
    let result = update(&mut session, &before, 0, false, true);
    assert_eq!(result.view.as_ref().unwrap().0, 1);
    let linked = &session.panes[1].view;
    assert!((linked.pan[0] - 0.05).abs() < 1e-6);
    assert!((linked.pan[1] + 0.2).abs() < 1e-6);
    assert_eq!(linked.zoom, 2.);
    assert_eq!(linked.slice, target.slice);
    assert_eq!(linked.window, target.window);
    assert_eq!(linked.transform, target.transform);
    assert!(linked.invert);
    let generation = session.display_links.generation;
    let before = views(&session);
    assert!(update(&mut session, &before, 0, false, true).view.is_none());
    assert_eq!(session.display_links.generation, generation);
    assert!(session.display_links.pending.is_none());
}
#[test]
fn rapid_actions_coalesce_until_the_requested_frame_without_holding_pixel_references() {
    let mut session = session();
    session.display_links.selection.pan = true;
    let before = views(&session);
    let old = session.panes[0].image.as_ref().unwrap().1.clone();
    let count = Arc::strong_count(&old);
    session.panes[0].view.slice = 0;
    session.panes[0].view.pan = [0.1, 0.];
    assert!(update(&mut session, &before, 0, false, true).view.is_none());
    assert_eq!(Arc::strong_count(&old), count);
    assert_eq!(session.display_links.display_status, Status::Pending);
    let generation = session.display_links.generation;
    let before = views(&session);
    session.panes[0].view.pan = [0.2, 0.];
    update(&mut session, &before, 0, false, true);
    assert_eq!(session.display_links.generation, generation + 1);
    assert_eq!(session.panes[1].view.pan, [0.; 2]);
    finish_source(&mut session);
    let before = views(&session);
    assert!(update(&mut session, &before, 0, false, true).view.is_some());
    assert!((session.panes[1].view.pan[0] - 0.1).abs() < 1e-6);
    assert!(session.display_links.pending.is_none());
}
#[test]
fn pending_window_and_pan_changes_merge_without_dropping_either_action_group() {
    let mut session = session();
    session.display_links.selection = Selection {
        pan: true,
        zoom: false,
        window: true,
    };
    let before = views(&session);
    session.panes[0].view.slice = 0;
    session.panes[0].view.window = WindowLevel::BONE;
    update(&mut session, &before, 0, false, true);
    assert!(session.display_links.pending.as_ref().unwrap().mask.window);
    let before = views(&session);
    session.panes[0].view.pan = [0.2, 0.];
    update(&mut session, &before, 0, false, true);
    let mask = session.display_links.pending.as_ref().unwrap().mask;
    assert!(mask.display.pan && mask.window);
    finish_source(&mut session);
    let before = views(&session);
    update(&mut session, &before, 0, false, true);
    assert_eq!(session.panes[1].view.window, WindowLevel::BONE);
    assert!((session.panes[1].view.pan[0] - 0.1).abs() < 1e-6);
    assert_eq!(session.display_links.display_status, Status::Applied);
    assert_eq!(session.display_links.window_status, Status::Applied);
}
#[test]
fn changed_ownership_target_source_viewport_settings_and_disabled_workspace_cancel_pending_actions()
{
    for change in 0..6 {
        let mut session = session();
        session.display_links.selection.pan = true;
        let before = views(&session);
        session.panes[0].view.slice = 0;
        session.panes[0].view.pan = [0.2, 0.];
        update(&mut session, &before, 0, false, true);
        let before = views(&session);
        match change {
            0 => session.active = 1,
            1 => session.panes[1].view.invert = true,
            2 => session.panes[0].source = Arc::new((*session.panes[0].source).clone()),
            3 => session.panes[1].canvas_aspect = Some(1.),
            4 => session.display_links.selection.pan = false,
            _ => {}
        }
        let expected = session.panes[1].view.clone();
        let result = update(&mut session, &before, 0, false, change != 5);
        assert!(result.view.is_none());
        assert!(session.display_links.pending.is_none(), "change {change}");
        assert_eq!(session.panes[1].view, expected);
        finish_source(&mut session);
        let before = views(&session);
        let active = session.active;
        assert!(
            update(&mut session, &before, active, false, true)
                .view
                .is_none()
        );
    }
}
#[test]
fn spatial_refusal_keeps_window_link_independent_and_scalar_refusal_keeps_physical_link_independent()
 {
    for refuse_spatial in [false, true] {
        let mut session = session();
        session.display_links.selection = Selection {
            pan: true,
            zoom: false,
            window: true,
        };
        let (_, target) = session.panes[1].image.as_mut().unwrap();
        let target = Arc::make_mut(target);
        if refuse_spatial {
            target.geometry.as_mut().unwrap().frame_of_reference =
                Some("unrelated-local-reference".into());
        } else {
            target.scalar = None;
        }
        let before = views(&session);
        session.panes[0].view.pan = [0.2, 0.];
        session.panes[0].view.window = WindowLevel::BONE;
        let result = update(&mut session, &before, 0, false, true);
        assert!(result.status_changed && result.view.is_some());
        if refuse_spatial {
            assert_eq!(session.panes[1].view.pan, before[1].pan);
            assert_eq!(session.panes[1].view.window, WindowLevel::BONE);
            assert_eq!(
                session.display_links.display_status,
                Status::Failed(LinkError::Reference)
            );
        } else {
            assert!((session.panes[1].view.pan[0] - 0.1).abs() < 1e-6);
            assert_eq!(session.panes[1].view.window, before[1].window);
            assert_eq!(
                session.display_links.window_status,
                Status::Failed(LinkError::Representation)
            );
        }
    }
}
#[test]
fn decode_failures_only_end_actions_bound_to_the_failed_requested_index() {
    let mut session = session();
    session.display_links.selection.pan = true;
    let before = views(&session);
    session.panes[0].view.slice = 0;
    session.panes[0].view.pan = [0.2, 0.];
    session.panes[0].error_index = Some(2);
    update(&mut session, &before, 0, false, true);
    assert!(session.display_links.pending.is_some());
    session.panes[0].error_index = Some(0);
    let before = views(&session);
    let result = update(&mut session, &before, 0, false, true);
    assert!(result.status_changed && result.view.is_none());
    assert!(session.display_links.pending.is_none());
    assert_eq!(
        session.display_links.display_status,
        Status::Failed(LinkError::View)
    );
}
#[test]
fn source_replacement_resets_opt_ins_and_drops_pending_actions() {
    let mut session = session();
    session.display_links.selection = Selection {
        pan: true,
        zoom: true,
        window: true,
    };
    let before = views(&session);
    session.panes[0].view.slice = 0;
    session.panes[0].view.pan = [0.2, 0.];
    update(&mut session, &before, 0, false, true);
    assert!(session.display_links.pending.is_some());
    assert!(session.replace_active(1, &egui::Context::default()));
    assert!(!session.display_links.selection.pan);
    assert!(!session.display_links.selection.zoom);
    assert!(!session.display_links.selection.window);
    assert!(session.display_links.pending.is_none());
}

#[test]
fn enabling_or_disabling_window_link_does_not_reapply_the_existing_physical_links() {
    let mut session = session();
    session.display_links.selection.pan = true;
    let before = views(&session);
    update(&mut session, &before, 0, true, true);
    session.panes[0].view.window = WindowLevel::BONE;
    session.panes[1].view.pan = [0.4, 0.1];
    session.panes[1].view.zoom = 3.;
    let before = views(&session);
    session.display_links.selection.window = true;
    update(&mut session, &before, 0, true, true);
    assert_eq!(session.panes[1].view.window, WindowLevel::BONE);
    assert_eq!(session.panes[1].view.pan, before[1].pan);
    assert_eq!(session.panes[1].view.zoom, before[1].zoom);
    let before = views(&session);
    session.display_links.selection.window = false;
    assert!(update(&mut session, &before, 0, true, true).view.is_none());
    assert_eq!(session.panes[1].view, before[1]);
    assert_eq!(session.display_links.window_status, Status::Off);
}

#[test]
fn pan_and_zoom_opt_ins_keep_the_other_physical_quantity_independent() {
    for pan_only in [false, true] {
        let mut session = session();
        session.display_links.selection = Selection {
            pan: pan_only,
            zoom: !pan_only,
            window: false,
        };
        session.panes[1].view.zoom = 3.;
        session.panes[1].view.pan = [0.2, 0.1];
        let before = views(&session);
        session.panes[0].view.zoom = 2.;
        session.panes[0].view.pan = [0.1, -0.2];
        update(&mut session, &before, 0, false, true);
        let target = &session.panes[1].view;
        if pan_only {
            assert_eq!(target.zoom, 3.);
            assert!((target.pan[0] - 0.075).abs() < 1e-6);
            assert!((target.pan[1] + 0.3).abs() < 1e-6);
        } else {
            assert_eq!(target.zoom, 2.);
            for (actual, old) in target.pan.into_iter().zip(before[1].pan) {
                assert!((actual - old * 2. / 3.).abs() < 1e-6);
            }
        }
    }
}

#[test]
fn changed_canvas_aspects_start_one_fresh_physical_action_with_the_same_anchor() {
    let mut session = session();
    session.display_links.selection = Selection {
        pan: true,
        zoom: true,
        window: true,
    };
    session.panes[0].view.pan = [0.1, -0.2];
    let before = views(&session);
    update(&mut session, &before, 0, true, true);
    let old_pan = session.panes[1].view.pan;
    let aspects = std::array::from_fn(|index| session.panes[index].canvas_aspect);
    session.panes[1].canvas_aspect = Some(4.);
    let before = views(&session);
    let result = session.display_links.update(Input {
        panes: [&session.panes[0], &session.panes[1]],
        before: &before,
        before_aspects: aspects,
        active: 0,
        previous_active: 0,
        reconfigured: false,
        enabled: true,
    });
    let (target, linked) = result.view.unwrap();
    assert_eq!(target, 1);
    assert!((linked.pan[0] - old_pan[0] / 2.).abs() < 1e-6);
    assert_eq!(linked.pan[1], old_pan[1]);
    assert_eq!(linked.window, before[1].window);
    session.panes[1].view = linked;
    let generation = session.display_links.generation;
    let before = views(&session);
    assert!(update(&mut session, &before, 0, false, true).view.is_none());
    assert_eq!(session.display_links.generation, generation);
}
