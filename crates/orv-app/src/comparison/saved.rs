// SPDX-License-Identifier: GPL-3.0-or-later
//! Explicit, path-free comparison files; all source reads and I/O run on a worker.
use crate::annotations::{Annotations, CapturedMark, SavedMark};
use eframe::egui;
use orv_core::{
    Plane, ViewState,
    scene::{ImageMetadata, frame_pixel_hash, fresh_series_fingerprint, is_sha256},
    study::{Frame, Series},
};
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::{Read, Write},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
};

const MAX_FILE_BYTES: u64 = 256 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ComparisonFile {
    version: u32,
    active: u8,
    panes: [SavedPane; 2],
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedPane {
    source_sha256: String,
    view: ViewState,
    marks: Vec<SavedMark>,
}
pub(super) struct Capture {
    pub source: Arc<Series>,
    pub image: Arc<Frame>,
    pub view: ViewState,
    pub marks: Vec<CapturedMark>,
}
pub(super) struct RestoredPane {
    pub view: ViewState,
    pub marks: Vec<SavedMark>,
    pub annotations: Annotations,
}
pub(super) enum Outcome {
    Saved {
        marks: [Vec<SavedMark>; 2],
        views: [ViewState; 2],
    },
    Restored {
        panes: Box<[RestoredPane; 2]>,
        active: usize,
    },
}
pub(super) enum Action {
    Save([Capture; 2], usize),
    Open([Arc<Series>; 2]),
}
pub(super) struct Task {
    receiver: mpsc::Receiver<Result<Outcome, &'static str>>,
    cancel: Arc<AtomicBool>,
    pub cancelling: bool,
}
impl Task {
    pub fn start(action: Action, ctx: egui::Context) -> Self {
        let (sender, receiver) = mpsc::sync_channel(1);
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let fallback = sender.clone();
        let repaint = ctx.clone();
        if std::thread::Builder::new()
            .name("orv-comparison-file".into())
            .spawn(move || {
                let cancelled = || flag.load(Ordering::Acquire);
                let result = match action {
                    Action::Save(captures, active) => {
                        let path = rfd::FileDialog::new()
                            .set_title("Save comparison locally")
                            .set_file_name("OpenRadView-comparison.orv.json")
                            .save_file();
                        match path {
                            Some(path) if !cancelled() => save(&path, captures, active, &cancelled),
                            _ => Err("Comparison save cancelled."),
                        }
                    }
                    Action::Open(sources) => {
                        let path = rfd::FileDialog::new()
                            .set_title("Open saved comparison")
                            .add_filter("OpenRadView comparison", &["json"])
                            .pick_file();
                        match path {
                            Some(path) if !cancelled() => open(&path, &sources, &cancelled),
                            _ => Err("Comparison opening cancelled."),
                        }
                    }
                };
                let _ = sender.send(result);
                ctx.request_repaint();
            })
            .is_err()
        {
            let _ = fallback.send(Err("Could not start the comparison worker."));
            repaint.request_repaint();
        }
        Self {
            receiver,
            cancel,
            cancelling: false,
        }
    }
    pub fn cancel(&mut self) {
        self.cancelling = true;
        self.cancel.store(true, Ordering::Release);
    }
    pub fn poll(&self) -> Option<Result<Outcome, &'static str>> {
        match self.receiver.try_recv() {
            Ok(result) => Some(result),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => Some(Err("The comparison worker stopped.")),
        }
    }
}
impl Drop for Task {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}

fn valid_view(view: &ViewState, series: &Series) -> Result<(), &'static str> {
    view.validate()?;
    if view.plane != Plane::Acquired
        || view.slice as usize >= series.frame_count()
        || !series.supports_window(view.slice as usize, view.window)
    {
        return Err("The saved view is not supported by this series.");
    }
    Ok(())
}
fn snapshot(capture: Capture, cancelled: &impl Fn() -> bool) -> Result<SavedPane, &'static str> {
    valid_view(&capture.view, &capture.source)?;
    let hash = fresh_series_fingerprint(&capture.source, cancelled)?;
    let actual = capture
        .source
        .read_fresh_frame(capture.view.slice as usize, cancelled)
        .map_err(|_| "The source is unavailable or has changed. Reopen it first.")?;
    if ImageMetadata::from_frame(&capture.image) != ImageMetadata::from_frame(&actual)
        || frame_pixel_hash(&capture.image, cancelled)? != frame_pixel_hash(&actual, cancelled)?
    {
        return Err("The displayed image differs from its source. Reopen it first.");
    }
    for mark in &capture.marks {
        let frame = capture
            .source
            .read_fresh_frame(mark.saved.frame as usize, cancelled)
            .map_err(|_| "A measurement image is unavailable or changed.")?;
        if !mark.matches(&frame) {
            return Err("A measurement no longer matches its source image.");
        }
    }
    Ok(SavedPane {
        source_sha256: hash,
        view: capture.view,
        marks: capture.marks.into_iter().map(|m| m.saved).collect(),
    })
}
fn save(
    path: &Path,
    captures: [Capture; 2],
    active: usize,
    cancelled: &impl Fn() -> bool,
) -> Result<Outcome, &'static str> {
    let [left, right] = captures;
    let panes = [snapshot(left, cancelled)?, snapshot(right, cancelled)?];
    let file = ComparisonFile {
        version: 1,
        active: active as u8,
        panes,
    };
    let bytes = serde_json::to_vec_pretty(&file).map_err(|_| "Could not encode the comparison.")?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err("Comparison exceeds the file size limit.");
    }
    if cancelled() {
        return Err("Comparison save cancelled.");
    }
    let parent = path.parent().ok_or("Choose a local save location.")?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)
        .map_err(|_| "Could not create the comparison file.")?;
    temp.write_all(&bytes)
        .map_err(|_| "Could not write the comparison file.")?;
    temp.as_file()
        .sync_all()
        .map_err(|_| "Could not finish the comparison file.")?;
    if cancelled() {
        return Err("Comparison save cancelled.");
    }
    temp.persist_noclobber(path).map_err(
        |_| "Could not save. Choose a new file name; existing files are not overwritten.",
    )?;
    let views = file.panes.each_ref().map(|pane| pane.view.clone());
    Ok(Outcome::Saved {
        marks: file.panes.map(|pane| pane.marks),
        views,
    })
}
fn open(
    path: &Path,
    sources: &[Arc<Series>; 2],
    cancelled: &impl Fn() -> bool,
) -> Result<Outcome, &'static str> {
    let input = File::open(path).map_err(|_| "Could not open the comparison file.")?;
    let mut bytes = Vec::new();
    input
        .take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "Could not read the comparison file.")?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err("Comparison file is too large.");
    }
    let file: ComparisonFile =
        serde_json::from_slice(&bytes).map_err(|_| "Invalid comparison file.")?;
    if file.version != 1 || file.active > 1 {
        return Err("Unsupported comparison file version.");
    }
    let mut restored = Vec::with_capacity(2);
    for (pane, source) in file.panes.into_iter().zip(sources) {
        if !is_sha256(&pane.source_sha256) || pane.marks.len() > 128 {
            return Err("Invalid comparison file.");
        }
        valid_view(&pane.view, source)?;
        if fresh_series_fingerprint(source, cancelled)? != pane.source_sha256 {
            return Err(
                "The selected series do not match this comparison. Open the original series in the same left/right order.",
            );
        }
        let annotations = Annotations::restore(&pane.marks, source, cancelled)?;
        restored.push(RestoredPane {
            view: pane.view,
            marks: pane.marks,
            annotations,
        });
    }
    if cancelled() {
        return Err("Comparison opening cancelled.");
    }
    let panes: [RestoredPane; 2] = restored
        .try_into()
        .map_err(|_| "Invalid comparison file.")?;
    Ok(Outcome::Restored {
        panes: Box::new(panes),
        active: file.active as usize,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::annotations::SavedTool;

    fn series() -> Arc<Series> {
        let mut series = orv_core::study::demo_study().series.remove(0);
        series.frames = crate::mpr_worker::tests::frames();
        Arc::new(series)
    }
    fn marks() -> Vec<SavedMark> {
        vec![SavedMark {
            frame: 0,
            tool: SavedTool::Angle,
            points: vec![[0., 0.], [1., 1.], [2., 0.]],
        }]
    }
    fn capture(series: Arc<Series>, marks: &[SavedMark]) -> Capture {
        let annotations = Annotations::restore(marks, &series, || false).unwrap();
        Capture {
            image: series.frames[0].clone(),
            source: series,
            view: ViewState::default(),
            marks: annotations.capture().unwrap(),
        }
    }
    #[test]
    fn saves_path_free_comparison_and_restores_only_matching_sources() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("comparison.json");
        let left = series();
        let mut changed = (*left).clone();
        Arc::make_mut(&mut changed.frames[0]).pixels = vec![9.; 16].into();
        let right = Arc::new(changed);
        let original = marks();
        assert!(matches!(
            save(
                &path,
                [
                    capture(left.clone(), &original),
                    capture(right.clone(), &[])
                ],
                1,
                &|| false
            ),
            Ok(Outcome::Saved { .. })
        ));
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains(folder.path().to_str().unwrap()));
        assert!(!text.contains("frame_of_reference"));
        assert!(!text.contains("pixels"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        let result = open(&path, &[left.clone(), right.clone()], &|| false).unwrap();
        let Outcome::Restored { panes, active } = result else {
            panic!("Expected restored comparison")
        };
        assert_eq!(active, 1);
        assert_eq!(panes[0].marks, original);
        assert_eq!(
            panes[0].annotations.capture().unwrap()[0].saved.points,
            original[0].points
        );
        assert!(open(&path, &[right.clone(), left.clone()], &|| false).is_err());
        assert!(open(&path, &[left.clone(), right.clone()], &|| true).is_err());
        let invalid_path = folder.path().join("invalid.json");
        let mut invalid: serde_json::Value = serde_json::from_str(&text).unwrap();
        invalid["panes"][0]["marks"][0]["points"][0][0] = serde_json::json!(999999);
        std::fs::write(&invalid_path, serde_json::to_vec(&invalid).unwrap()).unwrap();
        assert!(open(&invalid_path, &[left.clone(), right.clone()], &|| false).is_err());
        assert!(
            save(
                &path,
                [capture(left.clone(), &[]), capture(right, &[])],
                0,
                &|| false
            )
            .is_err()
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
    }
    #[test]
    fn rejects_invalid_or_oversized_files_and_cleans_up_cancelled_saves() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("comparison.json");
        let source = series();
        let sources = [source.clone(), source.clone()];
        std::fs::write(&path, vec![b' '; MAX_FILE_BYTES as usize + 1]).unwrap();
        assert!(open(&path, &sources, &|| false).is_err());
        std::fs::write(&path, b"{\"version\":1,\"paths\":[\"private\"]}").unwrap();
        assert!(open(&path, &sources, &|| false).is_err());
        let cancelled = folder.path().join("cancelled.json");
        assert!(
            save(
                &cancelled,
                [capture(source.clone(), &[]), capture(source, &[])],
                0,
                &|| true
            )
            .is_err()
        );
        assert!(!cancelled.exists());
    }
}
