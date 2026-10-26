// SPDX-License-Identifier: GPL-3.0-or-later
//! Cancellable latest-frame decoding. Only ready images reach the UI; cache
//! lookups, file access, decompression and conversion all stay off the UI thread.
mod workspace;
use eframe::egui;
use orv_core::{
    load::LoadError,
    study::{Frame, Series},
};
use std::sync::{
    Arc, Condvar, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

#[cfg(test)]
#[path = "frame_worker_tests.rs"]
mod tests;

pub struct ReadyFrame {
    pub index: usize,
    pub image: Result<Arc<Frame>, LoadError>,
}
struct Work {
    request: Option<(usize, u64)>,
    result: Option<(u64, ReadyFrame)>,
    terminal: Option<LoadError>,
}
struct Shared {
    work: Mutex<Work>,
    changed: Condvar,
    cancel: AtomicBool,
    generation: AtomicU64,
}
pub struct FrameWorker {
    shared: Arc<Shared>,
    requested: usize,
    repaint: egui::Context,
}
impl FrameWorker {
    pub fn start(series: Arc<Series>, index: usize, ctx: egui::Context) -> Self {
        let shared = Arc::new(Shared {
            work: Mutex::new(Work {
                request: Some((index, 0)),
                result: None,
                terminal: None,
            }),
            changed: Condvar::new(),
            cancel: AtomicBool::new(false),
            generation: AtomicU64::new(0),
        });
        let worker = shared.clone();
        let fallback = ctx.clone();
        let spawn = std::thread::Builder::new()
            .name("orv-frame-decode".into())
            .spawn(move || {
                loop {
                    let mut work = worker.work.lock().unwrap_or_else(|p| p.into_inner());
                    while work.request.is_none() && !worker.cancel.load(Ordering::Relaxed) {
                        work = worker.changed.wait(work).unwrap_or_else(|p| p.into_inner());
                    }
                    if worker.cancel.load(Ordering::Relaxed) {
                        return;
                    }
                    let Some((index, generation)) = work.request.take() else {
                        continue;
                    };
                    drop(work);
                    let cancelled = || {
                        worker.cancel.load(Ordering::Relaxed)
                            || worker.generation.load(Ordering::Relaxed) != generation
                    };
                    let image = series.read_frame(index, cancelled);
                    let success = image.is_ok();
                    if !cancelled() {
                        worker.work.lock().unwrap_or_else(|p| p.into_inner()).result =
                            Some((generation, ReadyFrame { index, image }));
                        ctx.request_repaint();
                    }
                    if success {
                        for next in index.saturating_add(1)
                            ..index.saturating_add(3).min(series.frame_count())
                        {
                            if cancelled() {
                                break;
                            }
                            if series.read_frame(next, cancelled).is_err() {
                                break;
                            }
                        }
                    }
                }
            });
        if spawn.is_err() {
            shared
                .work
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .terminal = Some(LoadError::Io);
            shared.work.lock().unwrap_or_else(|p| p.into_inner()).result = Some((
                0,
                ReadyFrame {
                    index,
                    image: Err(LoadError::Io),
                },
            ));
            fallback.request_repaint();
        }
        Self {
            shared,
            requested: index,
            repaint: fallback,
        }
    }
    pub fn request(&mut self, index: usize) {
        if self.requested == index {
            return;
        }
        self.requested = index;
        let mut work = self.shared.work.lock().unwrap_or_else(|p| p.into_inner());
        let generation = self.shared.generation.fetch_add(1, Ordering::Relaxed) + 1;
        if let Some(error) = work.terminal {
            work.result = Some((
                generation,
                ReadyFrame {
                    index,
                    image: Err(error),
                },
            ));
        } else {
            work.request = Some((index, generation));
            work.result = None;
        }
        self.repaint.request_repaint();
        self.shared.changed.notify_one();
    }
    pub fn take(&self) -> Option<ReadyFrame> {
        let mut work = self.shared.work.try_lock().ok()?;
        let (generation, result) = work.result.take()?;
        (generation == self.shared.generation.load(Ordering::Relaxed)).then_some(result)
    }
}
impl Drop for FrameWorker {
    fn drop(&mut self) {
        let _work = self.shared.work.lock().unwrap_or_else(|p| p.into_inner());
        self.shared.cancel.store(true, Ordering::Relaxed);
        self.shared.changed.notify_all();
    }
}
