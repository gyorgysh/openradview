// SPDX-License-Identifier: GPL-3.0-or-later
//! Physical-memory admission. Swap and process/study information are never queried.
use super::{MAX_CACHE_BYTES, MAX_VOLUME_BYTES};

pub const MIB: u64 = 1024 * 1024;
const GIB: u64 = 1024 * MIB;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemoryPolicy {
    pub cache_bytes: u64,
    pub volume_bytes: u64,
    /// Host-side ceiling for GPU resources, before the selected adapter's limits.
    pub gpu_bytes: u64,
}
impl MemoryPolicy {
    pub fn from_memory(total: u64, available: u64) -> Self {
        if total == 0 {
            return Self {
                cache_bytes: 128 * MIB,
                volume_bytes: 1536 * MIB,
                gpu_bytes: 512 * MIB,
            };
        }
        // CPU sources, cache and GPU copies compete on unified-memory machines.
        // Preserve headroom for other applications, decode/upload scratch and drivers.
        let pool = (total / 3).min(available.min(total) / 3 * 2).min(40 * GIB);
        let cache_bytes = tier(pool / 5, false).min(MAX_CACHE_BYTES);
        let volume_bytes = tier(pool.saturating_sub(cache_bytes) / 2, true).min(MAX_VOLUME_BYTES);
        Self {
            cache_bytes,
            volume_bytes,
            gpu_bytes: volume_bytes,
        }
    }
    pub(super) fn with_usage(total: u64, available: u64, used: Self) -> Self {
        let hard = Self::from_memory(total, total);
        let free = Self::from_memory(total, available);
        Self {
            // Optional cached pixels can be evicted. Live views stay valid while
            // pressure restricts additional preparation; no active pixels are freed.
            cache_bytes: free.cache_bytes,
            volume_bytes: hard
                .volume_bytes
                .min(used.volume_bytes.saturating_add(free.volume_bytes)),
            gpu_bytes: hard
                .gpu_bytes
                .min(used.gpu_bytes.saturating_add(free.gpu_bytes)),
        }
    }
}
fn tier(bytes: u64, volume: bool) -> u64 {
    [8192, 4096, 2048, 1536, 1024, 768, 512, 256, 128, 64, 32]
        .into_iter()
        .filter(|size| volume || !matches!(size, 1536 | 768))
        .map(|size| size * MIB)
        .find(|size| *size <= bytes)
        .unwrap_or(0)
}

pub(super) fn query() -> (u64, u64) {
    let mut system = sysinfo::System::new();
    system.refresh_memory_specifics(sysinfo::MemoryRefreshKind::nothing().with_ram());
    let mut total = system.total_memory();
    let mut available = system.available_memory();
    if let Some(limits) = system.cgroup_limits() {
        total = total.min(limits.total_memory);
        available = available.min(limits.free_memory);
    }
    (total, available.min(total))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn larger_hosts_can_retain_large_studies_without_preallocation() {
        for (ram, available, cache, volume) in [
            (8, 4, 512 * MIB, GIB),
            (16, 8, GIB, 2 * GIB),
            (32, 20, 2 * GIB, 4 * GIB),
            (64, 48, 4 * GIB, 8 * GIB),
            (128, 100, 8 * GIB, 8 * GIB),
        ] {
            let policy = MemoryPolicy::from_memory(ram * GIB, available * GIB);
            assert_eq!(policy.cache_bytes, cache);
            assert_eq!(policy.volume_bytes, volume);
            assert!(policy.cache_bytes + policy.volume_bytes + policy.gpu_bytes <= ram * GIB / 3);
        }
    }
    #[test]
    fn pressure_unknown_data_and_overflow_never_create_unbounded_allowances() {
        let unknown = MemoryPolicy::from_memory(0, u64::MAX);
        assert_eq!(unknown.cache_bytes, 128 * MIB);
        let empty = MemoryPolicy::from_memory(GIB, 0);
        assert_eq!(empty.cache_bytes + empty.volume_bytes + empty.gpu_bytes, 0);
        let used = MemoryPolicy {
            cache_bytes: GIB,
            volume_bytes: 2 * GIB,
            gpu_bytes: GIB,
        };
        let pressure = MemoryPolicy::with_usage(32 * GIB, 0, used);
        assert_eq!(pressure.cache_bytes, 0);
        assert_eq!(pressure.volume_bytes, used.volume_bytes);
        assert_eq!(pressure.gpu_bytes, used.gpu_bytes);
        for total in [1, 128 * MIB, GIB, u64::MAX] {
            for available in [0, 1, total, u64::MAX] {
                let policy = MemoryPolicy::from_memory(total, available);
                assert!(policy.cache_bytes <= MAX_CACHE_BYTES);
                assert!(policy.volume_bytes <= MAX_VOLUME_BYTES);
                assert!(policy.gpu_bytes <= MAX_VOLUME_BYTES);
                assert!(policy.cache_bytes + policy.volume_bytes + policy.gpu_bytes <= total / 3);
            }
        }
    }
    #[test]
    fn native_probe_reports_only_a_bounded_memory_snapshot() {
        let (total, available) = query();
        assert!(
            total > 0,
            "Native memory query must run on supported test hosts"
        );
        assert!(available <= total);
    }
}
