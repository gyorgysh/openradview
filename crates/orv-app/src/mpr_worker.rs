// SPDX-License-Identifier: GPL-3.0-or-later
//! A single worker coalesces requests and publishes one consistent three-plane snapshot.
use eframe::egui;
use orv_core::{Plane, spatial::GeometryError, study::Frame, volume::Volume};
use std::sync::{
    Arc, Condvar, Mutex, MutexGuard,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

pub const PLANES: [Plane; 3] = [Plane::Axial, Plane::Coronal, Plane::Sagittal];
#[cfg(test)]
#[path = "mpr_worker_tests.rs"]
pub(crate) mod tests;
#[derive(Clone)]
pub struct MprSnapshot {
    pub volume: Arc<Volume>,
    pub point: [f64; 3],
    pub images: [Arc<Frame>; 3],
    pub generation: u64,
    pub preview: bool,
    pub reformat_side: u32,
}
struct Request {
    point: [f64; 3],
    generation: u64,
    reformat_side: u32,
    interactive: bool,
    slab: Option<orv_core::slab::Slab>,
}
#[derive(Default)]
struct Work {
    request: Option<Request>,
    result: Option<Result<MprSnapshot, String>>,
}
#[derive(Default)]
struct Shared {
    work: Mutex<Work>,
    changed: Condvar,
    cancelled: AtomicBool,
    generation: AtomicU64,
}
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|p| p.into_inner())
}

pub struct MprWorker {
    shared: Arc<Shared>,
}
impl MprWorker {
    pub fn start(frames: impl Into<crate::frame_source::FrameSource>, ctx: egui::Context) -> Self {
        let frames = frames.into();
        let shared = Arc::new(Shared::default());
        let worker = shared.clone();
        let repaint = ctx.clone();
        let spawn = std::thread::Builder::new()
            .name("orv-mpr".into())
            .spawn(move || {
                let volume = match frames.volume(|| worker.cancelled.load(Ordering::Relaxed)) {
                    Ok(volume) => volume,
                    Err(error) => {
                        lock(&worker.work).result = Some(Err(error.to_string()));
                        ctx.request_repaint();
                        return;
                    }
                };
                let mut request = Request {
                    point: volume.center(),
                    generation: 0,
                    reformat_side: 512,
                    interactive: false,
                    slab: None,
                };
                'requests: loop {
                    if worker.cancelled.load(Ordering::Relaxed) {
                        return;
                    }
                    if request.interactive && request.reformat_side > 128 {
                        // Finish a small coherent set even if a newer pointer request arrives.
                        // Cancelling every input generation can starve the display during a drag.
                        let result = render(&volume, &request, 128, true, || {
                            worker.cancelled.load(Ordering::Relaxed)
                        });
                        if worker.cancelled.load(Ordering::Relaxed) {
                            return;
                        }
                        if !matches!(result, Err(GeometryError::Cancelled)) {
                            lock(&worker.work).result = Some(result.map_err(|e| e.to_string()));
                            ctx.request_repaint();
                        }
                        let mut work = lock(&worker.work);
                        loop {
                            if worker.cancelled.load(Ordering::Relaxed) {
                                return;
                            }
                            if let Some(next) = work.request.take() {
                                request = next;
                                continue 'requests;
                            }
                            let (next, timeout) = worker
                                .changed
                                .wait_timeout(work, std::time::Duration::from_millis(120))
                                .unwrap_or_else(|p| p.into_inner());
                            work = next;
                            if timeout.timed_out() {
                                break;
                            }
                        }
                    }
                    let cancel = || {
                        worker.cancelled.load(Ordering::Relaxed)
                            || worker.generation.load(Ordering::Relaxed) != request.generation
                    };
                    let result = render(&volume, &request, request.reformat_side, false, cancel);
                    if !cancel() && !matches!(result, Err(GeometryError::Cancelled)) {
                        lock(&worker.work).result = Some(result.map_err(|e| e.to_string()));
                        ctx.request_repaint();
                    }
                    let mut work = lock(&worker.work);
                    while work.request.is_none() && !worker.cancelled.load(Ordering::Relaxed) {
                        work = worker.changed.wait(work).unwrap_or_else(|p| p.into_inner());
                    }
                    if worker.cancelled.load(Ordering::Relaxed) {
                        return;
                    }
                    if let Some(next) = work.request.take() {
                        request = next;
                    }
                }
            });
        if spawn.is_err() {
            lock(&shared.work).result = Some(Err(
                "Could not start the view worker. Close a study and try again.".into(),
            ));
            repaint.request_repaint();
        }
        Self { shared }
    }
    #[cfg(any(test, feature = "mcp"))]
    pub fn request(&self, point: [f64; 3], reformat_side: u32) {
        self.enqueue(point, reformat_side, false, None);
    }
    #[cfg(test)]
    pub fn request_interactive(&self, point: [f64; 3], reformat_side: u32) {
        self.enqueue(point, reformat_side, true, None);
    }
    pub fn request_slab(
        &self,
        point: [f64; 3],
        reformat_side: u32,
        slab: Option<orv_core::slab::Slab>,
    ) {
        self.enqueue(point, reformat_side, true, slab);
    }
    fn enqueue(
        &self,
        point: [f64; 3],
        reformat_side: u32,
        interactive: bool,
        slab: Option<orv_core::slab::Slab>,
    ) {
        let mut work = lock(&self.shared.work);
        let generation = self.shared.generation.fetch_add(1, Ordering::Relaxed) + 1;
        work.request = Some(Request {
            point,
            generation,
            reformat_side,
            interactive,
            slab,
        });
        drop(work);
        self.shared.changed.notify_one();
    }
    pub fn take(&self) -> Option<Result<MprSnapshot, String>> {
        let result = lock(&self.shared.work).result.take()?;
        if result
            .as_ref()
            .is_ok_and(|s| !s.preview && s.generation != self.generation())
        {
            return None;
        }
        Some(result)
    }
    pub fn generation(&self) -> u64 {
        self.shared.generation.load(Ordering::Relaxed)
    }
}
impl Drop for MprWorker {
    fn drop(&mut self) {
        // Pair cancellation with the condition-variable mutex to prevent a lost wakeup
        // retaining private volume data after the study is closed.
        let _work = lock(&self.shared.work);
        self.shared.cancelled.store(true, Ordering::Relaxed);
        self.shared.changed.notify_all();
    }
}

fn render(
    volume: &Arc<Volume>,
    request: &Request,
    side: u32,
    preview: bool,
    cancel: impl Fn() -> bool,
) -> Result<MprSnapshot, GeometryError> {
    let images = PLANES
        .into_iter()
        .map(|plane| {
            if let Some(slab) = request.slab {
                let plane = orv_core::oblique::ObliquePlane::anatomical(plane, request.point)?;
                volume.reformat_slab(
                    &plane,
                    if preview { slab.preview() } else { slab },
                    side,
                    &cancel,
                )
            } else {
                volume.reformat(plane, request.point, side, &cancel)
            }
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(MprSnapshot {
        volume: volume.clone(),
        point: request.point,
        images: images.try_into().map_err(|_| GeometryError::Dimensions)?,
        generation: request.generation,
        preview,
        reformat_side: side,
    })
}
