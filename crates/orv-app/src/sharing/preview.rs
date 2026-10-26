// SPDX-License-Identifier: GPL-3.0-or-later
//! One cancellable/coalescing worker prepares local previews; no networking.
use super::ViewSpec;
use eframe::egui;
use orv_core::study::Series;
use orv_mcp::catalog::{PreparedSeries, Resolved};
use orv_render::snapshot::{Snapshot, SnapshotRenderer};
use std::sync::{
    Arc, Condvar, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

pub(super) enum Input {
    Source(u32, Arc<Series>),
    Prepared(Arc<PreparedSeries>),
}
pub(super) struct Preview {
    pub source: Arc<PreparedSeries>,
    pub snapshot: Snapshot,
    pub rgba: image::RgbaImage,
}
struct Request {
    spec: ViewSpec,
    size: [u32; 2],
    generation: u64,
}
struct Work {
    request: Option<Request>,
    result: Option<(u64, Result<Preview, &'static str>)>,
    terminal_error: Option<&'static str>,
}
struct Shared {
    work: Mutex<Work>,
    changed: Condvar,
    cancel: AtomicBool,
    generation: AtomicU64,
}
pub(super) struct PreviewWorker {
    shared: Arc<Shared>,
}

impl PreviewWorker {
    pub fn start(input: Input, spec: ViewSpec, size: [u32; 2], ctx: egui::Context) -> Self {
        let shared = Arc::new(Shared {
            work: Mutex::new(Work {
                request: Some(Request {
                    spec,
                    size,
                    generation: 0,
                }),
                result: None,
                terminal_error: None,
            }),
            changed: Condvar::new(),
            cancel: AtomicBool::new(false),
            generation: AtomicU64::new(0),
        });
        let worker = shared.clone();
        let fallback = ctx.clone();
        let spawn = std::thread::Builder::new()
            .name("orv-share-preview".into())
            .spawn(move || {
                let init = (|| {
                    let source = match input {
                        Input::Source(index, series) => {
                            Arc::new(PreparedSeries::new(index, series, || {
                                worker.cancel.load(Ordering::Relaxed)
                            })?)
                        }
                        Input::Prepared(source) => source,
                    };
                    let renderer = pollster::block_on(SnapshotRenderer::new())?;
                    Ok::<_, &'static str>((source, renderer))
                })();
                let (source, mut renderer) = match init {
                    Ok(value) => value,
                    Err(error) => {
                        let mut work = worker.work.lock().unwrap_or_else(|p| p.into_inner());
                        work.terminal_error = Some(error);
                        work.result = Some((worker.generation.load(Ordering::Relaxed), Err(error)));
                        ctx.request_repaint();
                        return;
                    }
                };
                loop {
                    let mut work = worker.work.lock().unwrap_or_else(|p| p.into_inner());
                    while work.request.is_none() && !worker.cancel.load(Ordering::Relaxed) {
                        work = worker.changed.wait(work).unwrap_or_else(|p| p.into_inner());
                    }
                    if worker.cancel.load(Ordering::Relaxed) {
                        return;
                    }
                    let Some(request) = work.request.take() else {
                        continue;
                    };
                    drop(work);
                    let cancelled = || {
                        worker.cancel.load(Ordering::Relaxed)
                            || worker.generation.load(Ordering::Relaxed) != request.generation
                    };
                    let result = (|| {
                        let scene = request.spec.scene(source.description(), request.size);
                        let snapshot = match source.resolve_scene(&scene, cancelled)? {
                            Resolved::Image(frame) => {
                                renderer.capture(&frame, &scene, cancelled)?
                            }
                            Resolved::Volume(volume) => {
                                renderer.capture_volume(volume, &scene, cancelled)?
                            }
                        };
                        let rgba = image::load_from_memory(&snapshot.png)
                            .map_err(|_| "Could not prepare the image preview")?
                            .to_rgba8();
                        Ok(Preview {
                            source: source.clone(),
                            snapshot,
                            rgba,
                        })
                    })();
                    if !cancelled() {
                        worker.work.lock().unwrap_or_else(|p| p.into_inner()).result =
                            Some((request.generation, result));
                        ctx.request_repaint();
                    }
                }
            });
        if spawn.is_err() {
            let mut work = shared.work.lock().unwrap_or_else(|p| p.into_inner());
            work.terminal_error = Some("Could not start the preview worker");
            work.result = Some((0, Err("Could not start the preview worker")));
            fallback.request_repaint();
        }
        Self { shared }
    }
    pub fn request(&self, spec: ViewSpec, size: [u32; 2]) {
        let mut work = self.shared.work.lock().unwrap_or_else(|p| p.into_inner());
        let generation = self.shared.generation.fetch_add(1, Ordering::Relaxed) + 1;
        if let Some(error) = work.terminal_error {
            work.result = Some((generation, Err(error)));
            return;
        }
        work.request = Some(Request {
            spec,
            size,
            generation,
        });
        work.result = None;
        self.shared.changed.notify_one();
    }
    pub fn take(&self) -> Option<Result<Preview, &'static str>> {
        let (generation, result) = self
            .shared
            .work
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .result
            .take()?;
        (generation == self.shared.generation.load(Ordering::Relaxed)).then_some(result)
    }
}
impl Drop for PreviewWorker {
    fn drop(&mut self) {
        let _work = self.shared.work.lock().unwrap_or_else(|p| p.into_inner());
        self.shared.cancel.store(true, Ordering::Relaxed);
        self.shared.changed.notify_all();
    }
}
