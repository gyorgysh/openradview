// SPDX-License-Identifier: GPL-3.0-or-later
use super::Entry;
use eframe::egui;
use std::sync::{
    Arc, Condvar, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
struct Work {
    request: Option<(u64, String)>,
    result: Option<(u64, Vec<usize>)>,
}
struct Shared {
    work: Mutex<Work>,
    changed: Condvar,
    cancel: AtomicBool,
    generation: AtomicU64,
}
pub(super) struct Search {
    shared: Arc<Shared>,
}
impl Search {
    pub fn start(entries: Arc<[Entry]>, ctx: egui::Context) -> Option<Self> {
        let shared = Arc::new(Shared {
            work: Mutex::new(Work {
                request: None,
                result: None,
            }),
            changed: Condvar::new(),
            cancel: AtomicBool::new(false),
            generation: AtomicU64::new(0),
        });
        let worker = shared.clone();
        std::thread::Builder::new()
            .name("orv-series-filter".into())
            .spawn(move || {
                loop {
                    let mut work = worker.work.lock().unwrap_or_else(|p| p.into_inner());
                    while work.request.is_none() && !worker.cancel.load(Ordering::Acquire) {
                        work = worker.changed.wait(work).unwrap_or_else(|p| p.into_inner());
                    }
                    if worker.cancel.load(Ordering::Acquire) {
                        return;
                    }
                    let Some((generation, query)) = work.request.take() else {
                        continue;
                    };
                    drop(work);
                    let cancelled = || {
                        worker.cancel.load(Ordering::Acquire)
                            || worker.generation.load(Ordering::Acquire) != generation
                    };
                    if let Some(rows) = matches(&entries, &query, cancelled) {
                        let mut work = worker.work.lock().unwrap_or_else(|p| p.into_inner());
                        let publish = !cancelled();
                        if publish {
                            work.result = Some((generation, rows));
                        }
                        drop(work);
                        if publish {
                            ctx.request_repaint();
                        }
                    }
                }
            })
            .ok()?;
        Some(Self { shared })
    }
    pub fn request(&self, query: String) -> u64 {
        let mut work = self.shared.work.lock().unwrap_or_else(|p| p.into_inner());
        let generation = self.shared.generation.fetch_add(1, Ordering::AcqRel) + 1;
        work.request = (!query.trim().is_empty()).then_some((generation, query));
        work.result = None;
        self.shared.changed.notify_one();
        generation
    }
    pub fn take(&self) -> Option<(u64, Vec<usize>)> {
        self.shared.work.try_lock().ok()?.result.take()
    }
}
impl Drop for Search {
    fn drop(&mut self) {
        let _work = self.shared.work.lock().unwrap_or_else(|p| p.into_inner());
        self.shared.cancel.store(true, Ordering::Release);
        self.shared.changed.notify_all();
    }
}
pub(super) fn matches(
    entries: &[Entry],
    query: &str,
    cancelled: impl Fn() -> bool,
) -> Option<Vec<usize>> {
    let query = query.to_lowercase();
    let terms: Vec<_> = query.split_whitespace().collect();
    let mut result = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        if index % 256 == 0 && cancelled() {
            return None;
        }
        if terms.iter().all(|term| entry.searchable.contains(term)) {
            result.push(index);
        }
    }
    (!cancelled()).then_some(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    #[test]
    fn latest_filter_wins_and_closing_an_idle_browser_releases_its_catalogue() {
        let entries: Arc<[Entry]> = Arc::from([
            Entry {
                text: "CT 1".into(),
                searchable: "ct 1".into(),
            },
            Entry {
                text: "MR 2".into(),
                searchable: "mr 2".into(),
            },
        ]);
        let weak = Arc::downgrade(&entries);
        let search = Search::start(entries.clone(), egui::Context::default()).unwrap();
        drop(entries);
        search.request("ct".into());
        let latest = search.request("mr".into());
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some((generation, rows)) = search.take() {
                assert_eq!(generation, latest);
                assert_eq!(rows, vec![1]);
                break;
            }
            assert!(Instant::now() < deadline, "filter did not finish");
            std::thread::sleep(Duration::from_millis(5));
        }
        drop(search);
        while weak.upgrade().is_some() {
            assert!(
                Instant::now() < deadline,
                "idle filter retained its catalogue"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}
