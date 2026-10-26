// SPDX-License-Identifier: GPL-3.0-or-later
use eframe::egui;
use orv_core::{labels::Labels, study::Series};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};

pub enum Operation {
    Start(Arc<Series>),
    Import(Arc<Labels>),
    Export(Arc<Labels>),
    Paint(Arc<Labels>, usize, Vec<[f64; 2]>, u32, u8),
    Threshold(Arc<Labels>, Option<usize>, [f32; 2], u8),
    Overlay(Arc<Labels>, usize),
}
pub enum Output {
    Start(Arc<Labels>),
    Change(Arc<Labels>),
    Saved,
    Cancelled,
    Overlay(usize, egui::ColorImage),
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
            Ok(result) => Some(result),
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
    let result = match op {
        Operation::Start(series) => {
            let volume = series.volume(cancelled)?;
            if volume.dimensions()[0] * volume.dimensions()[1] > 4 * 1024 * 1024 {
                return Err("Label editing is limited to four million pixels per slice".into());
            }
            Output::Start(Arc::new(Labels::empty(volume)?))
        }
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
            let volume = study.series[0].volume(cancelled)?;
            let source = current.source_arc();
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
            let mut edited = (*current).clone();
            edited.paint(z, &points, radius, label, cancelled)?;
            Output::Change(Arc::new(edited))
        }
        Operation::Threshold(current, slice, range, label) => {
            let mut edited = (*current).clone();
            edited.threshold(slice, range, label, cancelled)?;
            Output::Change(Arc::new(edited))
        }
        Operation::Overlay(current, z) => {
            let [w, h, _] = current.source().dimensions();
            Output::Overlay(
                z,
                egui::ColorImage::from_rgba_unmultiplied([w, h], &current.plane_rgba(z)?),
            )
        }
    };
    if cancelled() {
        Ok(Output::Cancelled)
    } else {
        Ok(result)
    }
}
