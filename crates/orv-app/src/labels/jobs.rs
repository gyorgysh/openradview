// SPDX-License-Identifier: GPL-3.0-or-later
//! One active job and one replaceable request; no timer or idle polling.
use eframe::egui;
use std::sync::{
    Arc, Condvar, Mutex, MutexGuard,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

struct Request<I> {
    generation: u64,
    input: I,
}
struct Work<I, O> {
    request: Option<Request<I>>,
    result: Option<(u64, Result<O, String>)>,
}
struct Shared<I, O> {
    work: Mutex<Work<I, O>>,
    changed: Condvar,
    stopped: AtomicBool,
    generation: AtomicU64,
}
fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value.lock().unwrap_or_else(|p| p.into_inner())
}
pub(super) struct Worker<I, O> {
    shared: Arc<Shared<I, O>>,
}
impl<I: Send + 'static, O: Send + 'static> Worker<I, O> {
    pub fn start(
        ctx: egui::Context,
        name: &str,
        run: fn(I, &dyn Fn() -> bool) -> Result<O, String>,
    ) -> Result<Self, String> {
        let shared = Arc::new(Shared {
            work: Mutex::new(Work {
                request: None::<Request<I>>,
                result: None,
            }),
            changed: Condvar::new(),
            stopped: AtomicBool::new(false),
            generation: AtomicU64::new(0),
        });
        let worker = shared.clone();
        std::thread::Builder::new()
            .name(name.into())
            .spawn(move || {
                loop {
                    let request = {
                        let mut work = lock(&worker.work);
                        while work.request.is_none() && !worker.stopped.load(Ordering::Acquire) {
                            work = worker.changed.wait(work).unwrap_or_else(|p| p.into_inner());
                        }
                        if worker.stopped.load(Ordering::Acquire) {
                            return;
                        }
                        work.request.take()
                    };
                    let Some(request) = request else { continue };
                    let cancelled = || {
                        worker.stopped.load(Ordering::Acquire)
                            || worker.generation.load(Ordering::Acquire) != request.generation
                    };
                    let result = run(request.input, &cancelled);
                    let mut work = lock(&worker.work);
                    let publish = !cancelled();
                    if publish {
                        work.result = Some((request.generation, result));
                    }
                    drop(work);
                    if publish {
                        ctx.request_repaint();
                    }
                }
            })
            .map_err(|_| "Could not start the local label worker. Try again.".to_owned())?;
        Ok(Self { shared })
    }
    pub fn submit(&self, input: I) -> u64 {
        let mut work = lock(&self.shared.work);
        let generation = self
            .shared
            .generation
            .fetch_add(1, Ordering::AcqRel)
            .wrapping_add(1);
        work.result = None;
        work.request = Some(Request { generation, input });
        drop(work);
        self.shared.changed.notify_one();
        generation
    }
    pub fn take(&self) -> Option<(u64, Result<O, String>)> {
        lock(&self.shared.work).result.take()
    }
    pub fn clear(&self) {
        let mut work = lock(&self.shared.work);
        self.shared.generation.fetch_add(1, Ordering::AcqRel);
        work.request = None;
        work.result = None;
    }
}
impl<I, O> Drop for Worker<I, O> {
    fn drop(&mut self) {
        // Synchronize the predicate with wait so shutdown cannot lose its wakeup.
        let mut work = lock(&self.shared.work);
        self.shared.stopped.store(true, Ordering::Release);
        work.request = None;
        work.result = None;
        drop(work);
        self.shared.changed.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    struct Job {
        id: u32,
        entered: Arc<AtomicBool>,
        release: Arc<AtomicBool>,
        seen: Arc<Mutex<Vec<u32>>>,
    }
    fn run(job: Job, cancel: &dyn Fn() -> bool) -> Result<u32, String> {
        lock(&job.seen).push(job.id);
        if job.id == 0 {
            job.entered.store(true, Ordering::Release);
            let deadline = Instant::now() + Duration::from_secs(3);
            while !job.release.load(Ordering::Acquire) && Instant::now() < deadline {
                std::thread::yield_now();
            }
        }
        if cancel() {
            Err("Cancelled".into())
        } else {
            Ok(job.id)
        }
    }
    fn until(mut condition: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !condition() {
            assert!(Instant::now() < deadline, "label worker timed out");
            std::thread::yield_now();
        }
    }
    #[test]
    fn active_old_work_is_suppressed_and_only_the_latest_pending_request_runs() {
        let entered = Arc::new(AtomicBool::new(false));
        let release = Arc::new(AtomicBool::new(false));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let job = |id| Job {
            id,
            entered: entered.clone(),
            release: release.clone(),
            seen: seen.clone(),
        };
        let worker = Worker::start(egui::Context::default(), "test-label-coalescing", run).unwrap();
        worker.submit(job(0));
        until(|| entered.load(Ordering::Acquire));
        worker.submit(job(1));
        let expected = worker.submit(job(2));
        release.store(true, Ordering::Release);
        let mut output = None;
        until(|| {
            output = worker.take();
            output.is_some()
        });
        let (generation, value) = output.unwrap();
        assert_eq!(generation, expected);
        assert_eq!(value.unwrap(), 2);
        assert_eq!(*lock(&seen), vec![0, 2]);
        assert!(worker.take().is_none());
    }
    #[test]
    fn clearing_drops_pending_inputs_and_cancels_the_active_generation() {
        let entered = Arc::new(AtomicBool::new(false));
        let release = Arc::new(AtomicBool::new(false));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let job = |id| Job {
            id,
            entered: entered.clone(),
            release: release.clone(),
            seen: seen.clone(),
        };
        let worker = Worker::start(egui::Context::default(), "test-label-clear", run).unwrap();
        worker.submit(job(0));
        until(|| entered.load(Ordering::Acquire));
        worker.submit(job(1));
        worker.clear();
        assert!(lock(&worker.shared.work).request.is_none());
        release.store(true, Ordering::Release);
        let expected = worker.submit(job(3));
        let mut output = None;
        until(|| {
            output = worker.take();
            output.is_some()
        });
        let (generation, value) = output.unwrap();
        assert_eq!(generation, expected);
        assert_eq!(value.unwrap(), 3);
        assert_eq!(*lock(&seen), vec![0, 3]);
    }
    #[test]
    fn dropping_an_idle_worker_releases_its_shared_state() {
        for _ in 0..32 {
            let worker = Worker::start(
                egui::Context::default(),
                "test-label-stop",
                |input: u32, _| Ok(input),
            )
            .unwrap();
            let shared = Arc::downgrade(&worker.shared);
            drop(worker);
            until(|| shared.upgrade().is_none());
        }
    }
}
