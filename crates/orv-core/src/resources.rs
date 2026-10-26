// SPDX-License-Identifier: GPL-3.0-or-later
//! Reservations bound concurrently retained resources without retaining study data.
mod policy;
pub use policy::MemoryPolicy;
use std::sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicU64, Ordering},
};
use std::time::{Duration, Instant};

pub const MAX_VOLUME_BYTES: u64 = 8 * 1024 * 1024 * 1024;
pub const MAX_CACHE_BYTES: u64 = 8 * 1024 * 1024 * 1024;

struct Resources {
    volumes: Arc<MemoryBudget>,
    cache: Arc<MemoryBudget>,
    gpu: Arc<MemoryBudget>,
    refreshed: Mutex<Instant>,
}
fn resources() -> &'static Resources {
    static RESOURCES: OnceLock<Resources> = OnceLock::new();
    RESOURCES.get_or_init(|| {
        let (total, available) = policy::query();
        let policy = MemoryPolicy::from_memory(total, available);
        Resources {
            volumes: MemoryBudget::new(policy.volume_bytes),
            cache: MemoryBudget::new(policy.cache_bytes),
            gpu: MemoryBudget::new(policy.gpu_bytes),
            refreshed: Mutex::new(Instant::now()),
        }
    })
}
pub fn memory_policy() -> MemoryPolicy {
    let resources = resources();
    MemoryPolicy {
        cache_bytes: resources.cache.limit(),
        volume_bytes: resources.volumes.limit(),
        gpu_bytes: resources.gpu.limit(),
    }
}
/// Worker-only, activity-driven refresh. No timer, study state or process list.
pub fn refresh_memory_policy() -> MemoryPolicy {
    let resources = resources();
    let mut refreshed = resources
        .refreshed
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if refreshed.elapsed() >= Duration::from_secs(1) {
        let (total, available) = policy::query();
        let policy = MemoryPolicy::with_usage(
            total,
            available,
            MemoryPolicy {
                cache_bytes: resources.cache.used(),
                volume_bytes: resources.volumes.used(),
                gpu_bytes: resources.gpu.used(),
            },
        );
        resources.cache.set_limit(policy.cache_bytes);
        resources.volumes.set_limit(policy.volume_bytes);
        resources.gpu.set_limit(policy.gpu_bytes);
        *refreshed = Instant::now();
    }
    memory_policy()
}

#[derive(Debug)]
pub struct MemoryBudget {
    limit: AtomicU64,
    used: AtomicU64,
}

#[derive(Debug)]
pub struct Reservation {
    budget: Arc<MemoryBudget>,
    bytes: u64,
}

impl MemoryBudget {
    pub fn volumes() -> Arc<Self> {
        resources().volumes.clone()
    }
    pub fn decoded_cache() -> Arc<Self> {
        resources().cache.clone()
    }
    pub fn gpu() -> Arc<Self> {
        resources().gpu.clone()
    }
    pub fn new(limit: u64) -> Arc<Self> {
        Arc::new(Self {
            limit: AtomicU64::new(limit),
            used: AtomicU64::new(0),
        })
    }
    pub fn used(&self) -> u64 {
        self.used.load(Ordering::Acquire)
    }
    pub fn limit(&self) -> u64 {
        self.limit.load(Ordering::Acquire)
    }
    pub fn set_limit(&self, limit: u64) {
        self.limit.store(limit, Ordering::Release);
    }
    pub fn reserve(self: &Arc<Self>, bytes: u64) -> Option<Reservation> {
        self.reserve_with_limit(bytes, self.limit())
    }
    pub fn reserve_with_limit(self: &Arc<Self>, bytes: u64, limit: u64) -> Option<Reservation> {
        self.used
            .try_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_add(bytes)
                    .filter(|total| *total <= self.limit().min(limit))
            })
            .ok()?;
        Some(Reservation {
            budget: self.clone(),
            bytes,
        })
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        self.budget.used.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reservations_share_a_limit_and_release_on_failure_or_cancellation() {
        let budget = MemoryBudget::new(100);
        let first = budget.reserve(60).unwrap();
        assert!(budget.reserve(41).is_none());
        assert!(budget.reserve(u64::MAX).is_none());
        let last = budget.reserve(40).unwrap();
        assert_eq!(budget.used(), 100);
        drop(first);
        assert_eq!(budget.used(), 40);
        assert!(budget.reserve(61).is_none());
        drop(last);
        assert_eq!(budget.used(), 0);
        assert!(budget.reserve(100).is_some());
        assert_eq!(budget.used(), 0);
    }
    #[test]
    fn concurrent_admission_cannot_overbook() {
        let budget = MemoryBudget::new(100);
        std::thread::scope(|scope| {
            let barrier = Arc::new(std::sync::Barrier::new(8));
            let workers: Vec<_> = (0..8)
                .map(|_| {
                    let budget = budget.clone();
                    let barrier = barrier.clone();
                    scope.spawn(move || {
                        let reservation = budget.reserve(30);
                        barrier.wait();
                        assert!(budget.used() <= 100);
                        barrier.wait();
                        reservation.is_some()
                    })
                })
                .collect();
            let admitted = workers
                .into_iter()
                .map(|worker| worker.join().unwrap())
                .filter(|ok| *ok)
                .count();
            assert_eq!(admitted, 3);
        });
        assert_eq!(budget.used(), 0);
    }
    #[test]
    fn lowering_a_limit_preserves_live_allocations_and_blocks_new_admission() {
        let budget = MemoryBudget::new(100);
        let retained = budget.reserve(80).unwrap();
        budget.set_limit(40);
        assert_eq!(budget.used(), 80);
        assert!(budget.reserve(1).is_none());
        drop(retained);
        assert_eq!(budget.used(), 0);
        assert!(budget.reserve(41).is_none());
        let next = budget.reserve(40).unwrap();
        budget.set_limit(80);
        assert!(budget.reserve(40).is_some());
        drop(next);
        assert_eq!(budget.used(), 0);
    }
}
