// SPDX-License-Identifier: GPL-3.0-or-later
//! One coalescing worker retains only the latest target and completed plane.
use eframe::egui;
use orv_core::{oblique::ObliquePlane, spatial::GeometryError, study::Frame, volume::Volume};
use std::sync::{
    Arc, Condvar, Mutex, MutexGuard,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

#[cfg(test)]
mod tests;

#[derive(Clone)]
pub(super) struct Snapshot {
    pub volume: Arc<Volume>,
    pub plane: ObliquePlane,
    pub image: Arc<Frame>,
    pub generation: u64,
    pub reformat_side: u32,
}
struct Request {
    plane: ObliquePlane,
    generation: u64,
    side: u32,
}
#[derive(Default)]
struct Work {
    request: Option<Request>,
    result: Option<Result<Snapshot, String>>,
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
pub(super) struct Worker {
    shared: Arc<Shared>,
}
impl Worker {
    pub fn start(frames: impl Into<crate::frame_source::FrameSource>, ctx: egui::Context) -> Self {
        let frames = frames.into();
        let shared = Arc::new(Shared::default());
        let worker = shared.clone();
        let repaint = ctx.clone();
        let spawn = std::thread::Builder::new()
            .name("orv-oblique".into())
            .spawn(move || {
                let initial = (|| {
                    let volume = frames.volume(|| worker.cancelled.load(Ordering::Relaxed))?;
                    let plane = ObliquePlane::from_angles(volume.center(), [0.; 3])
                        .map_err(|e| e.to_string())?;
                    Ok::<_, String>((volume, plane))
                })();
                let (volume, plane) = match initial {
                    Ok(initial) => initial,
                    Err(error) => {
                        lock(&worker.work).result = Some(Err(error.to_string()));
                        ctx.request_repaint();
                        return;
                    }
                };
                let mut request = Request {
                    plane,
                    generation: 0,
                    side: 512,
                };
                loop {
                    let cancel = || {
                        worker.cancelled.load(Ordering::Acquire)
                            || worker.generation.load(Ordering::Acquire) != request.generation
                    };
                    let image = volume.reformat_oblique(&request.plane, request.side, cancel);
                    let mut work = lock(&worker.work);
                    if !cancel() {
                        work.result = match image {
                            Ok(image) => Some(Ok(Snapshot {
                                volume: volume.clone(),
                                plane: request.plane.clone(),
                                image,
                                generation: request.generation,
                                reformat_side: request.side,
                            })),
                            Err(GeometryError::Cancelled) => None,
                            Err(error) => Some(Err(error.to_string())),
                        };
                        ctx.request_repaint();
                    }
                    while work.request.is_none() && !worker.cancelled.load(Ordering::Acquire) {
                        work = worker.changed.wait(work).unwrap_or_else(|p| p.into_inner());
                    }
                    if worker.cancelled.load(Ordering::Acquire) {
                        return;
                    }
                    if let Some(next) = work.request.take() {
                        request = next;
                    }
                }
            });
        if spawn.is_err() {
            lock(&shared.work).result = Some(Err("Could not start the oblique worker".into()));
            repaint.request_repaint();
        }
        Self { shared }
    }
    pub fn request(&self, plane: ObliquePlane, side: u32) {
        let mut work = lock(&self.shared.work);
        let generation = self.shared.generation.fetch_add(1, Ordering::AcqRel) + 1;
        work.result = None;
        work.request = Some(Request {
            plane,
            generation,
            side,
        });
        drop(work);
        self.shared.changed.notify_one();
    }
    pub fn generation(&self) -> u64 {
        self.shared.generation.load(Ordering::Acquire)
    }
    pub fn take(&self) -> Option<Result<Snapshot, String>> {
        let result = lock(&self.shared.work).result.take()?;
        if result
            .as_ref()
            .is_ok_and(|s| s.generation != self.generation())
        {
            return None;
        }
        Some(result)
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        // Use the same mutex as the waiter so cancellation cannot lose its wakeup.
        let _work = lock(&self.shared.work);
        self.shared.cancelled.store(true, Ordering::Release);
        self.shared.changed.notify_all();
    }
}
