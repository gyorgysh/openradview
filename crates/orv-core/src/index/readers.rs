// SPDX-License-Identifier: GPL-3.0-or-later
//! Bounded hot decompression streams, independent of the decoded-entry lock.
use super::ReadState;
use crate::load::LoadError;
use std::sync::{
    Mutex, MutexGuard,
    atomic::{AtomicU64, Ordering},
};

const EMPTY: u64 = u64::MAX;
struct Slot {
    source: AtomicU64,
    used: AtomicU64,
    state: Mutex<ReadState>,
}
pub(super) struct Readers {
    slots: [Slot; 4],
    clock: AtomicU64,
}
impl Default for Readers {
    fn default() -> Self {
        Self {
            slots: std::array::from_fn(|_| Slot {
                source: AtomicU64::new(EMPTY),
                used: AtomicU64::new(0),
                state: Mutex::new(ReadState::default()),
            }),
            clock: AtomicU64::new(0),
        }
    }
}
impl Readers {
    pub fn acquire(
        &self,
        source: u64,
        cancelled: &impl Fn() -> bool,
    ) -> Result<MutexGuard<'_, ReadState>, LoadError> {
        loop {
            if cancelled() {
                return Err(LoadError::Cancelled);
            }
            // Prefer the latest matching stream, then an empty or least-used slot.
            let mut order = [0, 1, 2, 3];
            order.sort_by_key(|index| {
                let slot = &self.slots[*index];
                let affinity = slot.source.load(Ordering::Acquire);
                let used = slot.used.load(Ordering::Acquire);
                (
                    if affinity == source {
                        0
                    } else if affinity == EMPTY {
                        1
                    } else {
                        2
                    },
                    if affinity == source {
                        u64::MAX - used
                    } else {
                        used
                    },
                )
            });
            for index in order {
                let slot = &self.slots[index];
                match slot.state.try_lock() {
                    Ok(reader) => {
                        slot.source.store(source, Ordering::Release);
                        slot.used.store(
                            self.clock.fetch_add(1, Ordering::Relaxed).wrapping_add(1),
                            Ordering::Release,
                        );
                        return Ok(reader);
                    }
                    Err(std::sync::TryLockError::Poisoned(_)) => return Err(LoadError::Io),
                    Err(std::sync::TryLockError::WouldBlock) => (),
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
    pub fn clear(&self) -> bool {
        let mut cleared = true;
        for slot in &self.slots {
            if let Ok(mut state) = slot.state.try_lock() {
                state.stream = None;
                slot.source.store(EMPTY, Ordering::Release);
            } else {
                cleared = false;
            }
        }
        cleared
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hot_streams_reuse_empty_slots_then_replace_the_oldest_and_remain_bounded() {
        let readers = Readers::default();
        for source in 0..4 {
            drop(readers.acquire(source, &|| false).unwrap());
        }
        for (index, slot) in readers.slots.iter().enumerate() {
            assert_eq!(slot.source.load(Ordering::Acquire), index as u64);
        }
        drop(readers.acquire(0, &|| false).unwrap());
        drop(readers.acquire(4, &|| false).unwrap());
        assert_eq!(readers.slots[1].source.load(Ordering::Acquire), 4);
        assert_eq!(readers.slots[0].source.load(Ordering::Acquire), 0);
        let guards: Vec<_> = (0..4)
            .map(|source| readers.acquire(source, &|| false).unwrap())
            .collect();
        let polls = std::cell::Cell::new(0);
        assert!(matches!(
            readers.acquire(5, &|| {
                polls.set(polls.get() + 1);
                polls.get() > 2
            }),
            Err(LoadError::Cancelled)
        ));
        assert!(!readers.clear());
        drop(guards);
        assert!(readers.clear());
        assert!(
            readers
                .slots
                .iter()
                .all(|slot| slot.source.load(Ordering::Acquire) == EMPTY)
        );
    }
}
