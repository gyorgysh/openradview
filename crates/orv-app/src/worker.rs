// SPDX-License-Identifier: GPL-3.0-or-later
use eframe::egui;
use orv_core::{index::index_paths, load::LoadError, study::Study};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        mpsc,
    },
};

pub enum Source {
    Files,
    Folder,
    Paths(Vec<PathBuf>),
    Demo(crate::demos::Demo),
}
pub struct ImportedStudy {
    pub study: Study,
    pub catalog: crate::series_browser::Catalog,
}

pub struct ImportTask {
    pub demo: Option<crate::demos::Demo>,
    pub receiver: mpsc::Receiver<Result<Option<ImportedStudy>, LoadError>>,
    pub cancel: Arc<AtomicBool>,
    pub processed: Arc<AtomicUsize>,
    pub checked_bytes: Arc<AtomicU64>,
}
impl ImportTask {
    pub fn start(source: Source, ctx: egui::Context) -> Self {
        let demo = match source {
            Source::Demo(demo) => Some(demo),
            _ => None,
        };
        let (sender, receiver) = mpsc::sync_channel(1);
        let cancel = Arc::new(AtomicBool::new(false));
        let processed = Arc::new(AtomicUsize::new(0));
        let checked_bytes = Arc::new(AtomicU64::new(0));
        let worker_checked = checked_bytes.clone();
        let worker_cancel = cancel.clone();
        let worker_processed = processed.clone();
        let fallback = sender.clone();
        let repaint = ctx.clone();
        let spawn = std::thread::Builder::new()
            .name("orv-study-import".into())
            .spawn(move || {
                let result = if let Some(demo) = demo {
                    orv_core::load::load_zip(demo.bytes(), &worker_cancel, |files| {
                        worker_processed.store(files, Ordering::Relaxed);
                    })
                    .map(Some)
                } else {
                    let paths = match source {
                        Source::Files => rfd::FileDialog::new()
                            .set_title("Open local DICOM, NIfTI or ZIP")
                            .pick_files(),
                        Source::Folder => rfd::FileDialog::new()
                            .set_title("Open local image folder")
                            .pick_folder()
                            .map(|p| vec![p]),
                        Source::Paths(paths) => Some(paths),
                        Source::Demo(_) => None,
                    };
                    match paths {
                        Some(paths) => {
                            let mut repaint = std::time::Instant::now();
                            index_paths(
                                &paths,
                                || worker_cancel.load(Ordering::Relaxed),
                                |p| {
                                    worker_processed.store(p.files, Ordering::Relaxed);
                                    worker_checked.store(p.checked_bytes, Ordering::Relaxed);
                                    if repaint.elapsed() >= std::time::Duration::from_millis(40) {
                                        ctx.request_repaint();
                                        repaint = std::time::Instant::now();
                                    }
                                },
                            )
                        }
                        .map(Some),
                        None => Ok(None),
                    }
                };
                let result = result.and_then(|study| {
                    study
                        .map(|study| {
                            let catalog = crate::series_browser::Catalog::build(&study, || {
                                worker_cancel.load(Ordering::Relaxed)
                            })?;
                            Ok(ImportedStudy { study, catalog })
                        })
                        .transpose()
                });
                let _ = sender.send(result);
                ctx.request_repaint();
            });
        if spawn.is_err() {
            let _ = fallback.send(Err(LoadError::Io));
            repaint.request_repaint();
        }
        Self {
            demo,
            receiver,
            cancel,
            processed,
            checked_bytes,
        }
    }
}
impl Drop for ImportTask {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}
