// SPDX-License-Identifier: GPL-3.0-or-later
use orv_core::{ViewState, scene::SceneState, study::demo_study};
use orv_mcp::{catalog::PreparedSeries, consent::*};
use std::sync::Arc;

fn prepared() -> (Arc<PreparedSeries>, SceneState) {
    let mut study = demo_study();
    let mut series = study.series.remove(0);
    series.frames.truncate(3);
    let source = Arc::new(PreparedSeries::new(2, Arc::new(series), || false).unwrap());
    let scene = SceneState {
        schema_version: 1,
        source_sha256: source.description().source_sha256.clone(),
        series_index: 2,
        view: ViewState::default(),
        reconstruction: None,
        crosshair_mm: None,
        reformat_side: 512,
        output_size: [64, 64],
    };
    (source, scene)
}
fn consent(data: DataScope) -> Consent {
    Consent {
        recipient: "User-configured local agent".into(),
        data,
        tools: ToolScopes {
            view_control: true,
            snapshots: true,
            metadata: true,
            measurements: true,
        },
        preview_reviewed: true,
        burned_in_risk_accepted: true,
    }
}

#[test]
fn explicit_review_is_required_and_does_not_expand_with_view_control() {
    let (source, scene) = prepared();
    let mut unreviewed = consent(DataScope::ReviewedScenes);
    unreviewed.preview_reviewed = false;
    assert!(Session::grant(source.clone(), unreviewed, scene.clone()).is_err());
    let mut no_risk_ack = consent(DataScope::ReviewedScenes);
    no_risk_ack.burned_in_risk_accepted = false;
    assert!(Session::grant(source.clone(), no_risk_ack, scene.clone()).is_err());
    let session =
        Session::grant(source, consent(DataScope::ReviewedScenes), scene.clone()).unwrap();
    assert!(session.authorize(&scene, Operation::Snapshot).is_ok());
    let mut other = scene.clone();
    other.view.slice = 1;
    session.request_view(other.clone()).unwrap();
    session.publish_view(scene.clone()).unwrap();
    assert_eq!(
        session.current().unwrap(),
        other,
        "A stale UI publication cannot replace an unconsumed agent request"
    );
    assert!(
        session.approved_current().unwrap().is_none(),
        "Unreviewed current context is private too"
    );
    for op in [
        Operation::Snapshot,
        Operation::Metadata,
        Operation::Measurement,
    ] {
        assert!(session.authorize(&other, op).is_err());
    }
    assert_eq!(session.take_requested_view().unwrap(), Some(other.clone()));
    assert!(session.take_requested_view().unwrap().is_none());
    session.approve_preview(&other).unwrap();
    assert_eq!(session.approved_current().unwrap(), Some(other.clone()));
    assert!(session.authorize(&other, Operation::Snapshot).is_ok());
    other.view.invert = true;
    assert!(
        session.authorize(&other, Operation::Snapshot).is_err(),
        "Even display settings must be reviewed in exact-view mode"
    );
    other.view.invert = false;
    for change in 0..3 {
        let mut transformed = other.clone();
        match change {
            0 => transformed.view.transform.rotate_clockwise(),
            1 => transformed.view.transform.flip_horizontal = true,
            _ => transformed.view.transform.flip_vertical = true,
        }
        assert!(
            session
                .authorize(&transformed, Operation::Snapshot)
                .is_err(),
            "Rotation/flips cannot reuse approval for another exact view"
        );
    }
}

#[test]
fn series_scope_stays_within_approved_source_and_tool_scopes() {
    let (source, mut scene) = prepared();
    let mut c = consent(DataScope::SelectedSeries);
    c.tools.measurements = false;
    let session = Session::grant(source, c, scene.clone()).unwrap();
    scene.view.slice = 2;
    assert!(session.authorize(&scene, Operation::Snapshot).is_ok());
    assert!(session.authorize(&scene, Operation::Measurement).is_err());
    scene.view.slice = 3;
    assert!(session.authorize(&scene, Operation::Snapshot).is_err());
    scene.view.slice = 0;
    scene.series_index = 0;
    assert!(session.authorize(&scene, Operation::SetView).is_err());
    scene.series_index = 2;
    scene.source_sha256 = "0".repeat(64);
    assert!(session.authorize(&scene, Operation::Metadata).is_err());
}

#[test]
fn revoke_invalidates_clones_cancels_work_and_releases_images() {
    let (source, scene) = prepared();
    let weak = Arc::downgrade(&source);
    let session =
        Session::grant(source, consent(DataScope::SelectedSeries), scene.clone()).unwrap();
    let clone = session.clone();
    let lease = session.authorize(&scene, Operation::Snapshot).unwrap();
    let thread_session = clone.clone();
    let thread_scene = scene.clone();
    let worker = std::thread::spawn(move || {
        while !thread_session.is_revoked() {
            std::thread::yield_now();
        }
        assert!(
            thread_session
                .authorize(&thread_scene, Operation::Snapshot)
                .is_err()
        );
        assert!(
            lease
                .resolve(&thread_scene, || thread_session.is_revoked())
                .is_err()
        );
    });
    session.revoke();
    worker.join().unwrap();
    assert!(clone.is_revoked());
    assert!(clone.cancellation().is_cancelled());
    assert!(clone.current().is_err());
    assert!(clone.approve_preview(&scene).is_err());
    assert!(
        weak.upgrade().is_none(),
        "Revoked session handles must not retain the source"
    );
}

#[test]
fn secure_tokens_are_unique_and_compare_exactly() {
    let a = BearerToken::generate().unwrap();
    let b = BearerToken::generate().unwrap();
    assert_eq!(a.expose_for_configuration().len(), 64);
    assert_ne!(a.expose_for_configuration(), b.expose_for_configuration());
    assert!(a.matches(a.expose_for_configuration()));
    assert!(!a.matches(b.expose_for_configuration()));
    assert!(!a.matches(""));
}
