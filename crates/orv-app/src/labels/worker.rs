// SPDX-License-Identifier: GPL-3.0-or-later
use eframe::egui;
use orv_core::{
    labels::{Labels, PhysicalBrush, SeedPoint, SeededRequest, SelectionRequest},
    study::Series,
    volume::Volume,
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::{io, path::Path};

pub enum Operation {
    Start(Arc<Series>),
    StartVolume(Arc<Volume>),
    Import(Arc<Labels>),
    Export(Arc<Labels>),
    Paint(Arc<Labels>, usize, Vec<[f64; 2]>, u32, u8),
    Threshold(Arc<Labels>, Option<usize>, [f32; 2], u8),
    Preview(Arc<Labels>, SelectionRequest, u64),
    SeededPreview(
        Arc<Labels>,
        SeededRequest,
        Vec<SeedPoint>,
        orv_core::resources::Reservation,
        u64,
        u64,
    ),
    Physical(
        Arc<Labels>,
        Box<PhysicalBrush>,
        super::physical::InputPath,
        u64,
    ),
}
pub enum Output {
    Start(Arc<Labels>),
    Change(Arc<Labels>),
    Preview(Box<super::regions::Preview>),
    SeededPreview(Box<super::seeded::Preview>),
    Physical(Box<super::physical::Preview>),
    Saved,
    Cancelled,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum TaskKind {
    RegionPreview,
    Export,
    Other,
}
pub struct Task {
    receiver: mpsc::Receiver<Result<Output, String>>,
    cancel: Arc<AtomicBool>,
    kind: TaskKind,
}
impl Task {
    pub fn start(op: Operation, ctx: egui::Context) -> Self {
        let kind = match &op {
            Operation::Preview(..) | Operation::SeededPreview(..) => TaskKind::RegionPreview,
            Operation::Export(..) => TaskKind::Export,
            _ => TaskKind::Other,
        };
        let (sender, receiver) = mpsc::sync_channel(1);
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let fallback = sender.clone();
        let repaint = ctx.clone();
        let spawn = std::thread::Builder::new()
            .name("orv-label-edit".into())
            .spawn(move || {
                let result = run(op, &flag);
                let _ = sender.send(result);
                ctx.request_repaint();
            });
        if spawn.is_err() {
            let _ = fallback.send(Err("Could not start the label worker. Try again.".into()));
            repaint.request_repaint();
        }
        Self {
            receiver,
            cancel,
            kind,
        }
    }
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
    pub(super) fn cancel_region_preview(&self) {
        if self.kind == TaskKind::RegionPreview {
            self.cancel();
        }
    }
    pub fn poll(&self) -> Option<Result<Output, String>> {
        match self.receiver.try_recv() {
            Ok(result) => Some(
                if self.kind != TaskKind::Export && self.cancel.load(Ordering::Acquire) {
                    Ok(Output::Cancelled)
                } else {
                    result
                },
            ),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => Some(Err("Label worker stopped".into())),
        }
    }
}
impl Drop for Task {
    fn drop(&mut self) {
        self.cancel();
    }
}
pub(super) fn run(op: Operation, flag: &AtomicBool) -> Result<Output, String> {
    let cancelled = || flag.load(Ordering::Relaxed);
    orv_core::resources::refresh_memory_policy();
    let result = match op {
        Operation::Start(series) => {
            label_dimensions(&series, &cancelled)?;
            let volume = series.volume(cancelled)?;
            Output::Start(Arc::new(Labels::empty(volume)?))
        }
        Operation::StartVolume(volume) => Output::Start(Arc::new(Labels::empty(volume)?)),
        Operation::Import(current) => {
            let Some(path) = rfd::FileDialog::new()
                .set_title("Load matching NIfTI label map (integer 0–255)")
                .add_filter("NIfTI", &["nii", "gz", "hdr", "img"])
                .pick_file()
            else {
                return Ok(Output::Cancelled);
            };
            let study = orv_core::index::index_paths(&[path], cancelled, |_| {})
                .map_err(|e| e.to_string())?;
            if study.series.len() != 1 || study.series[0].modality != "NIFTI" {
                return Err(
                    "Choose one scalar NIfTI label volume without time or component axes".into(),
                );
            }
            let source = current.source_arc();
            if label_dimensions(&study.series[0], &cancelled)? != source.dimensions() {
                return Err(
                    "Label dimensions must match the source; resampling is not performed".into(),
                );
            }
            let volume = study.series[0].volume(cancelled)?;
            Output::Change(Arc::new(Labels::import(source, &volume, cancelled)?))
        }
        Operation::Export(current) => {
            let Some(path) = rfd::FileDialog::new()
                .set_title("Export labels only — NIfTI-2")
                .set_file_name("labels.nii")
                .add_filter("NIfTI", &["nii"])
                .save_file()
            else {
                return Ok(Output::Cancelled);
            };
            // Once the file has been written, a late cancellation must not
            // hide the fact that it exists at the chosen destination.
            return export_labels(&current, &path, cancelled);
        }
        Operation::Paint(current, z, points, radius, label) => {
            let mut edited = current.try_clone(cancelled)?;
            edited.paint(z, &points, radius, label, cancelled)?;
            Output::Change(Arc::new(edited))
        }
        Operation::Threshold(current, slice, range, label) => {
            let mut edited = current.try_clone(cancelled)?;
            edited.threshold(slice, range, label, cancelled)?;
            Output::Change(Arc::new(edited))
        }
        Operation::Preview(current, request, epoch) => {
            let selection = current.select_region(request, cancelled)?;
            let summary = selection.summary();
            let labels = Arc::new(selection.apply(cancelled)?);
            Output::Preview(Box::new(super::regions::Preview {
                base: current,
                labels,
                request,
                summary,
                epoch,
            }))
        }
        Operation::SeededPreview(current, request, seeds, _reservation, version, epoch) => {
            let selection = current.seeded_region(request, &seeds, cancelled)?;
            let summary = selection.summary();
            let labels = Arc::new(selection.apply(cancelled)?);
            Output::SeededPreview(Box::new(super::seeded::Preview {
                base: current,
                labels,
                request,
                summary,
                version,
                epoch,
            }))
        }
        Operation::Physical(current, settings, path, epoch) => {
            let stroke = current.propose_physical_stroke(&settings, path.points(), cancelled)?;
            Output::Physical(Box::new(super::physical::Preview { stroke, epoch }))
        }
    };
    if cancelled() {
        Ok(Output::Cancelled)
    } else {
        Ok(result)
    }
}

fn export_labels(
    current: &Labels,
    path: &Path,
    cancelled: impl Fn() -> bool,
) -> Result<Output, String> {
    if cancelled() {
        return Ok(Output::Cancelled);
    }
    if !path
        .extension()
        .and_then(std::ffi::OsStr::to_str)
        .is_some_and(|extension| extension.eq_ignore_ascii_case("nii"))
    {
        return Err("Choose a .nii filename for the uncompressed label map".into());
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path).map_err(|_| {
        "Cannot create label file. Choose a new filename; existing files are never overwritten."
    })?;
    let result = current
        .write_nifti(&file, &cancelled)
        .and_then(|()| file.sync_all());
    drop(file);
    if let Err(error) = result {
        if let Err(cleanup) = std::fs::remove_file(path)
            && cleanup.kind() != io::ErrorKind::NotFound
        {
            return Err("Label export did not finish. The partial file could not be removed; check the destination.".into());
        }
        return if error.kind() == io::ErrorKind::Interrupted && cancelled() {
            Ok(Output::Cancelled)
        } else {
            Err("Label export did not finish; the partial file was removed".into())
        };
    }
    Ok(Output::Saved)
}

fn label_dimensions(series: &Series, cancelled: &dyn Fn() -> bool) -> Result<[usize; 3], String> {
    let info = orv_core::inspection::series_info(series, cancelled).map_err(|e| e.to_string())?;
    let grid = info.geometry.map_err(|e| e.to_string())?;
    Labels::check_dimensions(grid.dimensions)?;
    Ok(grid.dimensions)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn queued_connected_preview() -> (crate::labels::LabelUi, Arc<Labels>, Arc<AtomicBool>) {
        use orv_core::labels::{LabelScope, RegionOperation, SelectionRequest};

        let base = crate::labels::tests::labels();
        let mut state = crate::labels::LabelUi {
            current: Some(base.clone()),
            ..Default::default()
        };
        state.regions.pick([2, 2, 1]);
        state.regions.sync(&base, 1, Some(1));
        let request = SelectionRequest {
            scope: LabelScope::Slice(1),
            label: 1,
            operation: RegionOperation::Grow {
                seed: [2, 2, 1],
                range: [0., 100.],
            },
        };
        let result = run(
            Operation::Preview(base.clone(), request, state.regions.epoch),
            &AtomicBool::new(false),
        );
        assert!(
            matches!(&result, Ok(Output::Preview(preview)) if state.regions.accept(preview, &base))
        );
        let (sender, receiver) = mpsc::sync_channel(1);
        sender.send(result).unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        state.task = Some(Task {
            receiver,
            cancel: cancel.clone(),
            kind: TaskKind::RegionPreview,
        });
        (state, base, cancel)
    }

    #[test]
    fn leaving_region_tools_cancels_a_queued_preview_without_changing_labels() {
        let (mut state, base, cancel) = queued_connected_preview();
        state.stop_painting();
        assert!(
            cancel.load(Ordering::Acquire),
            "obsolete region work must stop"
        );
        state.poll();
        assert!(!state.busy() && !state.dirty);
        assert!(Arc::ptr_eq(state.current.as_ref().unwrap(), &base));
        assert!(!state.region_preview_visible());
    }

    #[test]
    fn changing_the_source_slice_cancels_only_after_inputs_change() {
        let (mut state, base, cancel) = queued_connected_preview();
        state.sync_regions(&base, 1, Some(1));
        assert!(!cancel.load(Ordering::Acquire));
        state.sync_regions(&base, 1, Some(2));
        assert!(cancel.load(Ordering::Acquire));
        state.poll();
        assert!(!state.busy() && !state.dirty);
        assert!(Arc::ptr_eq(state.current.as_ref().unwrap(), &base));
        assert!(!state.region_preview_visible());
    }

    #[test]
    fn leaving_region_tools_keeps_requested_paint_and_export_work() {
        for kind in [TaskKind::Other, TaskKind::Export] {
            let base = crate::labels::tests::labels();
            let mut state = crate::labels::LabelUi {
                current: Some(base.clone()),
                ..Default::default()
            };
            state.regions.sync(&base, 1, Some(1));
            let (sender, receiver) = mpsc::sync_channel(1);
            let (result, expected) = if kind == TaskKind::Export {
                (Output::Saved, base.clone())
            } else {
                let mut edited = base.try_clone(|| false).unwrap();
                edited.paint(1, &[[3., 3.]], 1, 7, || false).unwrap();
                let edited = Arc::new(edited);
                (Output::Change(edited.clone()), edited)
            };
            sender.send(Ok(result)).unwrap();
            let cancel = Arc::new(AtomicBool::new(false));
            state.task = Some(Task {
                receiver,
                cancel: cancel.clone(),
                kind,
            });
            state.sync_regions(&base, 1, Some(2));
            state.stop_painting();
            assert!(!cancel.load(Ordering::Acquire));
            state.poll();
            assert!(!state.busy());
            assert!(Arc::ptr_eq(state.current.as_ref().unwrap(), &expected));
        }
    }
    #[test]
    fn cancellation_after_completion_suppresses_a_queued_private_edit() {
        let (sender, receiver) = mpsc::sync_channel(1);
        sender
            .send(Ok(Output::Change(crate::labels::tests::labels())))
            .ok()
            .unwrap();
        let task = Task {
            receiver,
            cancel: Arc::new(AtomicBool::new(false)),
            kind: TaskKind::Other,
        };
        task.cancel();
        assert!(matches!(task.poll(), Some(Ok(Output::Cancelled))));
    }

    #[test]
    fn label_export_reports_committed_results_after_late_cancellation() {
        let (sender, receiver) = mpsc::sync_channel(1);
        sender.send(Ok(Output::Saved)).ok().unwrap();
        let task = Task {
            receiver,
            cancel: Arc::new(AtomicBool::new(true)),
            kind: TaskKind::Export,
        };
        assert!(matches!(task.poll(), Some(Ok(Output::Saved))));

        let (sender, receiver) = mpsc::sync_channel(1);
        sender.send(Err("cleanup failed".into())).unwrap();
        let task = Task {
            receiver,
            cancel: Arc::new(AtomicBool::new(true)),
            kind: TaskKind::Export,
        };
        assert!(matches!(task.poll(), Some(Err(error)) if error == "cleanup failed"));
    }

    #[test]
    fn completed_export_marks_the_saved_revision_after_late_cancel() {
        let original = crate::labels::tests::labels();
        let mut state = crate::labels::LabelUi {
            current: Some(original.clone()),
            ..Default::default()
        };
        state.mark_saved();
        let mut edited = original.try_clone(|| false).unwrap();
        edited.paint(1, &[[3., 3.]], 1, 7, || false).unwrap();
        let edited = Arc::new(edited);
        state.commit_change(edited.clone());
        assert!(state.dirty);

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("labels.nii");
        let result = export_labels(&edited, &path, || false);
        let (sender, receiver) = mpsc::sync_channel(1);
        sender.send(result).ok().unwrap();
        state.task = Some(Task {
            receiver,
            cancel: Arc::new(AtomicBool::new(true)),
            kind: TaskKind::Export,
        });
        state.poll();
        assert!(!state.dirty);
        assert!(state.message.contains("Label map exported"));
        let bytes = std::fs::read(path).unwrap();
        assert_eq!(
            &bytes[544..],
            edited.values().iter().copied().collect::<Vec<_>>()
        );

        state.undo_edit();
        assert!(state.dirty);
        state.redo_edit();
        assert!(!state.dirty);
    }

    #[test]
    fn label_export_is_private_exclusive_and_cleans_up_cancelled_output() {
        let labels = crate::labels::tests::labels();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("labels.nii");
        let calls = std::cell::Cell::new(0);
        let cancelled = || {
            let n = calls.get();
            calls.set(n + 1);
            n >= 2
        };
        assert!(matches!(
            export_labels(&labels, &path, cancelled),
            Ok(Output::Cancelled)
        ));
        assert!(!path.exists());
        assert!(matches!(
            export_labels(&labels, &path, || false),
            Ok(Output::Saved)
        ));
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[4..12], b"n+2\0\r\n\x1a\n");
        assert!(export_labels(&labels, &path, || false).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o077,
                0
            );
        }
    }

    #[test]
    fn label_export_rejects_names_that_do_not_describe_its_uncompressed_nifti() {
        let mut labels = crate::labels::tests::labels().try_clone(|| false).unwrap();
        labels.paint(1, &[[3., 3.]], 1, 7, || false).unwrap();
        let dir = tempfile::tempdir().unwrap();
        for name in ["labels.nii.gz", "labels.img", "labels", "labels.nii.txt"] {
            let path = dir.path().join(name);
            assert!(export_labels(&labels, &path, || false).is_err(), "{name}");
            assert!(!path.exists(), "{name}");
        }
        let path = dir.path().join("labels.NII");
        assert!(matches!(
            export_labels(&labels, &path, || false),
            Ok(Output::Saved)
        ));
        let study = orv_core::index::index_paths(&[path], || false, |_| {}).unwrap();
        assert_eq!(study.series.len(), 1);
        let volume = study.series[0].volume(|| false).unwrap();
        let imported = Labels::import(labels.source_arc(), &volume, || false).unwrap();
        assert!(imported.values().iter().eq(labels.values().iter()));
    }
}
