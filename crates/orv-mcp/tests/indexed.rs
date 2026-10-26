// SPDX-License-Identifier: GPL-3.0-or-later
//! An external edit cannot replace previously approved lazy pixels, even when
//! the file size, identity and modification timestamp are deliberately preserved.
use orv_core::{ViewState, index::index_paths, scene::SceneState};
use orv_mcp::{
    catalog::PreparedSeries,
    consent::{Consent, DataScope, Session, ToolScopes},
    tools::{SceneRequest, ViewerTools},
};
use rmcp::handler::server::wrapper::Parameters;
use std::{
    fs::{self, File, FileTimes},
    io::{Seek, SeekFrom, Write},
    sync::Arc,
};
#[path = "../../orv-core/tests/support/mod.rs"]
mod support;

#[tokio::test]
async fn indexed_grant_rechecks_pixels_after_an_external_edit_with_preserved_timestamps() {
    struct Temp(std::path::PathBuf);
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = Temp(std::env::temp_dir().join(format!(
        "orv-synthetic-grant-{}-{nonce}.dcm",
        std::process::id()
    )));
    let bytes = support::fixture(0., 1, false, false);
    fs::write(&path.0, &bytes).unwrap();
    let original = fs::metadata(&path.0).unwrap();
    let mut study = index_paths(std::slice::from_ref(&path.0), || false, |_| {}).unwrap();
    let series = Arc::new(study.series.remove(0));
    let backing = series.source.as_ref().unwrap().clone();
    let source = Arc::new(PreparedSeries::new(0, series, || false).unwrap());
    let scene = SceneState {
        schema_version: 1,
        source_sha256: source.description().source_sha256.clone(),
        series_index: 0,
        view: ViewState::default(),
        reconstruction: None,
        crosshair_mm: None,
        reformat_side: 64,
        output_size: [64, 64],
    };
    let consent = Consent {
        recipient: "Synthetic local reader".into(),
        data: DataScope::SelectedSeries,
        tools: ToolScopes {
            view_control: true,
            snapshots: true,
            metadata: true,
            measurements: true,
        },
        preview_reviewed: true,
        burned_in_risk_accepted: true,
    };
    let session = Session::grant(source.clone(), consent, scene.clone()).unwrap();
    let tools = ViewerTools::new(session, Arc::new(|| {}));
    assert_ne!(
        tools
            .get_tags(Parameters(SceneRequest { scene: None }))
            .await
            .is_error,
        Some(true)
    );
    assert!(backing.discard_cached_frames());
    let mut file = File::options().write(true).open(&path.0).unwrap();
    file.seek(SeekFrom::Start(bytes.len() as u64 - 8)).unwrap();
    file.write_all(&77u16.to_le_bytes()).unwrap();
    file.set_times(FileTimes::new().set_modified(original.modified().unwrap()))
        .unwrap();
    drop(file);
    assert_eq!(fs::metadata(&path.0).unwrap().len(), original.len());
    assert_eq!(
        fs::metadata(&path.0).unwrap().modified().unwrap(),
        original.modified().unwrap()
    );
    assert!(source.resolve_scene(&scene, || false).is_err());
    let response = tools
        .get_tags(Parameters(SceneRequest { scene: None }))
        .await;
    assert_eq!(response.is_error, Some(true));
    assert!(
        response.structured_content.is_none(),
        "No changed source metadata/pixels may escape"
    );
    assert!(
        !serde_json::to_string(&response)
            .unwrap()
            .contains("orv-synthetic-grant")
    );
}
