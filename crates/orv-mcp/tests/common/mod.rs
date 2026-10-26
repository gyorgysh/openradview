// SPDX-License-Identifier: GPL-3.0-or-later
use orv_core::{ViewState, scene::SceneState, study::demo_study};
use orv_mcp::{
    catalog::PreparedSeries,
    consent::{Consent, DataScope, Session, ToolScopes},
};
use std::sync::Arc;

pub fn session() -> (Session, SceneState) {
    let mut study = demo_study();
    let mut series = study.series.remove(0);
    series.frames.truncate(3);
    let source = Arc::new(PreparedSeries::new(0, Arc::new(series), || false).unwrap());
    let scene = SceneState {
        schema_version: 1,
        source_sha256: source.description().source_sha256.clone(),
        series_index: 0,
        view: ViewState::default(),
        reconstruction: None,
        crosshair_mm: None,
        reformat_side: 128,
        output_size: [64, 64],
    };
    let consent = Consent {
        recipient: "Synthetic test agent".into(),
        data: DataScope::ReviewedScenes,
        tools: ToolScopes {
            view_control: true,
            snapshots: true,
            metadata: true,
            measurements: true,
        },
        preview_reviewed: true,
        burned_in_risk_accepted: true,
    };
    (
        Session::grant(source, consent, scene.clone()).unwrap(),
        scene,
    )
}
