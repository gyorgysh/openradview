// SPDX-License-Identifier: GPL-3.0-or-later
//! A single worker coalesces requests and publishes one consistent three-plane snapshot.
use eframe::{egui, egui_wgpu::wgpu};
use orv_core::{Plane, spatial::GeometryError, study::Frame, volume::Volume};
use orv_render::reslice::GpuReformatImage;
mod planes;
use planes::{Gpu, PlaneCache, render};
use std::sync::{
    Arc, Condvar, Mutex, MutexGuard,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

pub const PLANES: [Plane; 3] = [Plane::Axial, Plane::Coronal, Plane::Sagittal];
#[cfg(all(test, feature = "gpu-tests"))]
mod gpu_tests;
#[cfg(test)]
#[path = "mpr_worker_tests.rs"]
pub(crate) mod tests;
#[derive(Clone)]
pub struct MprSnapshot {
    pub volume: Arc<Volume>,
    pub point: [f64; 3],
    pub images: [Arc<Frame>; 3],
    pub gpu_images: [Option<Arc<GpuReformatImage>>; 3],
    pub generation: u64,
    pub preview: bool,
    pub reformat_side: u32,
    canonical_requested: bool,
}
#[derive(Clone)]
struct Request {
    point: [f64; 3],
    generation: u64,
    reformat_side: u32,
    interactive: bool,
    slab: Option<orv_core::slab::Slab>,
    canonical: bool,
}
#[derive(Default)]
struct Work {
    request: Option<Request>,
    latest: Option<Request>,
    gpu_context: Option<(wgpu::Device, wgpu::Queue)>,
    gpu_offered: bool,
    result: Option<Result<MprSnapshot, String>>,
    terminal_error: Option<String>,
}
#[derive(Default)]
struct Shared {
    work: Mutex<Work>,
    changed: Condvar,
    cancelled: AtomicBool,
    generation: AtomicU64,
    canonical: AtomicBool,
    suspended: AtomicBool,
}
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|p| p.into_inner())
}

pub struct MprWorker {
    shared: Arc<Shared>,
    #[cfg(test)]
    worker: Option<std::thread::JoinHandle<()>>,
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
                        unavailable(&worker, error, &ctx);
                        return;
                    }
                };
                let mut request = {
                    let mut work = lock(&worker.work);
                    let request = work.request.take().unwrap_or_else(|| Request {
                        point: volume.center(),
                        generation: worker.generation.load(Ordering::Relaxed),
                        reformat_side: 512,
                        interactive: false,
                        slab: None,
                        canonical: worker.canonical.load(Ordering::Relaxed),
                    });
                    work.latest = Some(request.clone());
                    request
                };
                let mut cache = PlaneCache::default();
                let mut gpu = Gpu::default();
                'requests: loop {
                    if worker.cancelled.load(Ordering::Relaxed) {
                        return;
                    }
                    if worker.suspended.load(Ordering::Relaxed) {
                        gpu.release();
                        cache = PlaneCache::default();
                        let mut work = lock(&worker.work);
                        while worker.suspended.load(Ordering::Relaxed)
                            && !worker.cancelled.load(Ordering::Relaxed)
                        {
                            work = worker.changed.wait(work).unwrap_or_else(|p| p.into_inner());
                        }
                        if worker.cancelled.load(Ordering::Relaxed) {
                            return;
                        }
                        if let Some(next) = work.request.take() {
                            request = next;
                        }
                        continue;
                    }
                    if let Some(context) = lock(&worker.work).gpu_context.take() {
                        gpu.offer(context);
                    }
                    if request.interactive && request.reformat_side > 128 {
                        // Finish a coherent set even if a newer pointer request arrives.
                        // Cancelling every input generation can starve the display during a drag.
                        // Provisional refers to input settling, not reduced sampling.
                        let result = render(&volume, &request, true, &mut cache, &mut gpu, || {
                            worker.cancelled.load(Ordering::Relaxed)
                                || worker.suspended.load(Ordering::Relaxed)
                                || worker.canonical.load(Ordering::Relaxed) != request.canonical
                        });
                        if worker.cancelled.load(Ordering::Relaxed) {
                            return;
                        }
                        publish(&worker, &request, true, result, &ctx);
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
                            || worker.suspended.load(Ordering::Relaxed)
                            || worker.generation.load(Ordering::Relaxed) != request.generation
                    };
                    let result = render(&volume, &request, false, &mut cache, &mut gpu, cancel);
                    publish(&worker, &request, false, result, &ctx);
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
            unavailable(
                &shared,
                "Could not start the view worker. Close a study and try again.".into(),
                &repaint,
            );
        }
        Self {
            shared,
            #[cfg(test)]
            worker: spawn.ok(),
        }
    }
    /// Offer only handles on the UI thread; preparation and upload stay on the worker.
    pub fn offer_gpu(&self, device: &wgpu::Device, queue: &wgpu::Queue) {
        let mut work = lock(&self.shared.work);
        if work.gpu_offered {
            return;
        }
        work.gpu_offered = true;
        work.gpu_context = Some((device.clone(), queue.clone()));
        self.reschedule(&mut work);
        drop(work);
        self.shared.changed.notify_one();
    }
    pub fn set_suspended(&self, suspended: bool) {
        let mut work = lock(&self.shared.work);
        if self.shared.suspended.swap(suspended, Ordering::Relaxed) == suspended {
            return;
        }
        self.reschedule(&mut work);
        drop(work);
        self.shared.changed.notify_one();
    }
    #[cfg(any(test, feature = "mcp"))]
    pub fn set_canonical(&self, canonical: bool) {
        let mut work = lock(&self.shared.work);
        if self.shared.canonical.swap(canonical, Ordering::Relaxed) == canonical {
            return;
        }
        self.reschedule(&mut work);
        drop(work);
        self.shared.changed.notify_one();
    }
    fn reschedule(&self, work: &mut Work) {
        let generation = self.shared.generation.fetch_add(1, Ordering::Relaxed) + 1;
        work.result = work.terminal_error.as_ref().map(|error| Err(error.clone()));
        if let Some(mut latest) = work.latest.clone() {
            latest.generation = generation;
            latest.canonical = self.shared.canonical.load(Ordering::Relaxed);
            latest.interactive = false;
            work.latest = Some(latest.clone());
            work.request = Some(latest);
        }
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
        if let Some(error) = &work.terminal_error {
            work.result = Some(Err(error.clone()));
        } else if work.result.as_ref().is_some_and(Result::is_err) {
            work.result = None;
        }
        let request = Request {
            point,
            generation,
            reformat_side,
            interactive,
            slab,
            canonical: self.shared.canonical.load(Ordering::Relaxed),
        };
        work.latest = Some(request.clone());
        work.request = Some(request);
        drop(work);
        self.shared.changed.notify_one();
    }
    pub fn take(&self) -> Option<Result<MprSnapshot, String>> {
        let result = lock(&self.shared.work).result.take()?;
        if self.shared.suspended.load(Ordering::Relaxed) {
            return None;
        }
        if result.as_ref().is_ok_and(|s| {
            s.canonical_requested != self.shared.canonical.load(Ordering::Relaxed)
                || (!s.preview && s.generation != self.generation())
        }) {
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

fn unavailable(worker: &Shared, error: String, ctx: &egui::Context) {
    let mut work = lock(&worker.work);
    if worker.cancelled.load(Ordering::Relaxed) {
        return;
    }
    work.terminal_error = Some(error.clone());
    work.result = Some(Err(error));
    drop(work);
    ctx.request_repaint();
}
fn publish(
    worker: &Shared,
    request: &Request,
    preview: bool,
    result: Result<MprSnapshot, GeometryError>,
    ctx: &egui::Context,
) {
    let mut work = lock(&worker.work);
    if worker.cancelled.load(Ordering::Relaxed)
        || worker.suspended.load(Ordering::Relaxed)
        || worker.canonical.load(Ordering::Relaxed) != request.canonical
        || (!preview && worker.generation.load(Ordering::Relaxed) != request.generation)
        || (result.is_err() && worker.generation.load(Ordering::Relaxed) != request.generation)
        || matches!(result, Err(GeometryError::Cancelled))
    {
        return;
    }
    work.result = Some(result.map_err(|e| e.to_string()));
    drop(work);
    ctx.request_repaint();
}
