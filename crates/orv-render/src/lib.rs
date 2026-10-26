// SPDX-License-Identifier: GPL-3.0-or-later
//! UI-independent GPU rendering. Surface ownership belongs to the application.
pub mod slice;
pub mod snapshot;
pub mod volume;
mod volume_uniform;

/// Never select a web backend. macOS uses Metal exclusively.
pub fn native_backends() -> wgpu::Backends {
    if cfg!(target_os = "macos") {
        wgpu::Backends::METAL
    } else if cfg!(target_os = "windows") {
        wgpu::Backends::DX12 | wgpu::Backends::VULKAN
    } else {
        wgpu::Backends::VULKAN
    }
}
