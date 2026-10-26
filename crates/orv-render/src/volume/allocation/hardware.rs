// SPDX-License-Identifier: GPL-3.0-or-later
//! Local selected-adapter memory hints. Unknown capacity is never summed or guessed.
use orv_core::resources::MAX_VOLUME_BYTES;
const MIB: u64 = 1024 * 1024;

struct Profile {
    total: u64,
    available: Option<u64>,
    unified: bool,
}
fn allowance(profile: Option<Profile>, used: u64, software: bool) -> u64 {
    if software {
        return 256 * MIB;
    }
    let Some(profile) = profile.filter(|profile| profile.total > 0) else {
        return 1536 * MIB;
    };
    let capacity = profile.total / if profile.unified { 3 } else { 4 };
    let headroom = profile.available.map_or(u64::MAX, |free| {
        used.saturating_add(free.min(profile.total) / 2)
    });
    capacity.min(headroom).min(MAX_VOLUME_BYTES)
}
pub(super) fn ceiling(info: &wgpu::AdapterInfo, used: u64) -> u64 {
    allowance(
        profile(info),
        used,
        info.device_type == wgpu::DeviceType::Cpu,
    )
}

#[cfg(target_os = "macos")]
fn profile(info: &wgpu::AdapterInfo) -> Option<Profile> {
    use objc2_metal::MTLDevice;
    if info.backend != wgpu::Backend::Metal {
        return None;
    }
    let device = objc2_metal::MTLCreateSystemDefaultDevice()?;
    if device.name().to_string() != info.name {
        return None;
    }
    Some(Profile {
        total: device.recommendedMaxWorkingSetSize(),
        available: None,
        unified: device.hasUnifiedMemory(),
    })
}

#[cfg(target_os = "windows")]
fn profile(info: &wgpu::AdapterInfo) -> Option<Profile> {
    use winsafe::prelude::*;
    if info.device_type != wgpu::DeviceType::DiscreteGpu {
        return None;
    }
    let factory = winsafe::CreateDXGIFactory1().ok()?;
    let mut found = None;
    for adapter in factory.EnumAdapters1().take(64) {
        let description = adapter.ok()?.GetDesc1().ok()?;
        if description.VendorId == info.vendor
            && description.DeviceId == info.device
            && description.Description() == info.name
        {
            if found.is_some() {
                return None;
            }
            // DXGI's descriptor is dedicated capacity, not currently free VRAM.
            found = Some(Profile {
                total: description.DedicatedVideoMemory as u64,
                available: None,
                unified: false,
            });
        }
    }
    found
}

#[cfg(target_os = "linux")]
fn profile(info: &wgpu::AdapterInfo) -> Option<Profile> {
    use std::io::Read;
    fn number(path: &std::path::Path, hexadecimal: bool) -> Option<u64> {
        let mut text = String::new();
        std::fs::File::open(path)
            .ok()?
            .take(129)
            .read_to_string(&mut text)
            .ok()?;
        if text.len() > 128 {
            return None;
        }
        if hexadecimal {
            u64::from_str_radix(text.trim().strip_prefix("0x")?, 16).ok()
        } else {
            text.trim().parse().ok()
        }
    }
    if info.device_type != wgpu::DeviceType::DiscreteGpu {
        return None;
    }
    let mut found = None;
    for entry in std::fs::read_dir("/sys/class/drm").ok()?.take(256) {
        let entry = entry.ok()?;
        let name = entry.file_name();
        let name = name.to_str()?;
        if !name.strip_prefix("card").is_some_and(|suffix| {
            !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit())
        }) {
            continue;
        }
        let device = entry.path().join("device");
        if number(&device.join("vendor"), true) != Some(u64::from(info.vendor))
            || number(&device.join("device"), true) != Some(u64::from(info.device))
        {
            continue;
        }
        if found.is_some() {
            return None;
        }
        let total = number(&device.join("mem_info_vram_total"), false)?;
        let used = number(&device.join("mem_info_vram_used"), false)?;
        found = Some(Profile {
            total,
            available: Some(total.saturating_sub(used)),
            unified: false,
        });
    }
    found
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
fn profile(_: &wgpu::AdapterInfo) -> Option<Profile> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn adapter_capacity_and_available_memory_limit_new_allocations() {
        let profile = |total, available, unified| {
            Some(Profile {
                total,
                available,
                unified,
            })
        };
        assert_eq!(allowance(None, 0, false), 1536 * MIB);
        assert_eq!(allowance(None, u64::MAX, true), 256 * MIB);
        assert_eq!(
            allowance(
                profile(64 * 1024 * MIB, Some(48 * 1024 * MIB), false),
                0,
                false
            ),
            MAX_VOLUME_BYTES
        );
        assert_eq!(
            allowance(profile(8 * 1024 * MIB, Some(512 * MIB), false), 0, false),
            256 * MIB
        );
        assert_eq!(
            allowance(profile(8 * 1024 * MIB, Some(0), false), 1024 * MIB, false),
            1024 * MIB
        );
        assert_eq!(
            allowance(profile(24 * 1024 * MIB, None, true), 0, false),
            8 * 1024 * MIB
        );
        assert!(
            allowance(profile(u64::MAX, Some(u64::MAX), false), u64::MAX, false)
                <= MAX_VOLUME_BYTES
        );
    }
}
