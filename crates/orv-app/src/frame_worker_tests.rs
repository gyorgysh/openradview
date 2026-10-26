// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use crate::workspace::Workspace;
use std::time::{Duration, Instant};
#[path = "../../orv-core/tests/support/mod.rs"]
mod support;

#[test]
fn latest_request_wins_and_closing_releases_the_worker_source() {
    let mut study = orv_core::study::demo_study();
    let series = Arc::new(study.series.remove(0));
    let weak = Arc::downgrade(&series.frames[0]);
    let mut worker = FrameWorker::start(series.clone(), 0, egui::Context::default());
    for index in 1..48 {
        worker.request(index);
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    let result = loop {
        if let Some(result) = worker.take() {
            break result;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    };
    assert_eq!(result.index, 47);
    assert_eq!(result.image.unwrap().frame_index, 0);
    drop(series);
    drop(worker);
    while weak.upgrade().is_some() {
        assert!(
            Instant::now() < deadline,
            "Closed frame worker retained its source"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn indexed_navigation_retains_the_previous_complete_frame_until_the_requested_one_is_ready() {
    struct Temp(std::path::PathBuf);
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir =
        Temp(std::env::temp_dir().join(format!("orv-synthetic-ui-{}-{nonce}", std::process::id())));
    std::fs::create_dir(&dir.0).unwrap();
    for i in 0..2 {
        std::fs::write(
            dir.0.join(format!("synthetic-{i}.dcm")),
            support::fixture(i as f64 * 2., i, false, false),
        )
        .unwrap();
    }
    let study =
        orv_core::index::index_paths(std::slice::from_ref(&dir.0), || false, |_| {}).unwrap();
    let mut workspace = Workspace::default();
    workspace.study = Some(study);
    let ctx = egui::Context::default();
    let until = Instant::now() + Duration::from_secs(5);
    while workspace.current_image().is_none() {
        workspace.poll_image(&ctx);
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(workspace.current_image().unwrap().0, 0);
    workspace.view.slice = 1;
    // Requesting is independent from consuming results/painting the next frame.
    workspace.request_image(&ctx);
    assert_eq!(workspace.current_image().unwrap().0, 0);
    #[cfg(feature = "mcp")]
    {
        let (view, ready) = workspace.rendered_view().unwrap();
        assert_eq!(view.view.slice, 0);
        assert!(!ready);
    }
    while workspace.current_image().unwrap().0 != 1 {
        workspace.poll_image(&ctx);
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(2));
    }
    #[cfg(feature = "mcp")]
    {
        let (view, ready) = workspace.rendered_view().unwrap();
        assert_eq!(view.view.slice, 1);
        assert!(ready);
    }
    workspace.close();
    assert!(workspace.current_image().is_none());
}

#[cfg(feature = "mcp")]
#[test]
fn a_slider_target_is_not_published_before_that_frame_is_painted() {
    let mut workspace = Workspace::default();
    workspace.study = Some(orv_core::study::demo_study());
    workspace.view.slice = 24;
    workspace.last_image_view = Some(workspace.view.clone());
    workspace.view.slice = 25;
    let (spec, ready) = workspace.rendered_view().unwrap();
    assert_eq!(spec.view.slice, 24);
    assert!(!ready);
    workspace.last_image_view = Some(workspace.view.clone());
    let (spec, ready) = workspace.rendered_view().unwrap();
    assert_eq!(spec.view.slice, 25);
    assert!(ready);
}
