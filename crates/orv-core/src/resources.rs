// SPDX-License-Identifier: GPL-3.0-or-later
//! Reservations bound concurrently retained resources without retaining study data.
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

pub const VOLUME_BYTES: u64 = 1536 * 1024 * 1024;

#[derive(Debug)]
pub struct MemoryBudget {
    limit: u64,
    used: AtomicU64,
}

#[derive(Debug)]
pub struct Reservation {
    budget: Arc<MemoryBudget>,
    bytes: u64,
}

impl MemoryBudget {
    pub fn volumes() -> Arc<Self> {
        static VOLUMES: std::sync::OnceLock<Arc<MemoryBudget>> = std::sync::OnceLock::new();
        VOLUMES.get_or_init(|| Self::new(VOLUME_BYTES)).clone()
    }
    pub fn new(limit: u64) -> Arc<Self> {
        Arc::new(Self {
            limit,
            used: AtomicU64::new(0),
        })
    }
    pub fn used(&self) -> u64 {
        self.used.load(Ordering::Acquire)
    }
    pub fn reserve(self: &Arc<Self>, bytes: u64) -> Option<Reservation> {
        self.reserve_with_limit(bytes, self.limit)
    }
    pub fn reserve_with_limit(self: &Arc<Self>, bytes: u64, limit: u64) -> Option<Reservation> {
        self.used
            .try_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_add(bytes)
                    .filter(|total| *total <= self.limit.min(limit))
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
}
