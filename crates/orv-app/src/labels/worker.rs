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
pub struct Task {
    receiver: mpsc::Receiver<Result<Output, String>>,
    cancel: Arc<AtomicBool>,
}
impl Task {
    pub fn start(op: Operation, ctx: egui::Context) -> Self {
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
        Self { receiver, cancel }
    }
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
    pub fn poll(&self) -> Option<Result<Output, String>> {
        match self.receiver.try_recv() {
            Ok(result) => Some(if self.cancel.load(Ordering::Acquire) {
                Ok(Output::Cancelled)
            } else {
                result
            }),
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
            let file=std::fs::OpenOptions::new().write(true).create_new(true).open(&path).map_err(|_|"Cannot create label file. Choose a new filename; existing files are never overwritten.")?;
            let result = current
                .write_nifti(&file, cancelled)
                .and_then(|()| file.sync_all());
            drop(file);
            if result.is_err() {
                let _ = std::fs::remove_file(&path);
                return Err("Label export did not finish; the partial file was removed".into());
            }
            Output::Saved
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

fn label_dimensions(series: &Series, cancelled: &dyn Fn() -> bool) -> Result<[usize; 3], String> {
    let info = orv_core::inspection::series_info(series, cancelled).map_err(|e| e.to_string())?;
    let grid = info.geometry.map_err(|e| e.to_string())?;
    Labels::check_dimensions(grid.dimensions)?;
    Ok(grid.dimensions)
}

#[cfg(test)]
mod tests {
    use super::*;
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
        };
        task.cancel();
        assert!(matches!(task.poll(), Some(Ok(Output::Cancelled))));
    }
}
