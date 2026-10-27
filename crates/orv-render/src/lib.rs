// SPDX-License-Identifier: GPL-3.0-or-later
//! UI-independent GPU rendering. Surface ownership belongs to the application.
mod backend;
pub mod slice;
pub mod snapshot;
mod voi;
pub mod volume;
mod volume_uniform;
pub use backend::{configured_backends, device_descriptor};

/// Never select a web backend. macOS uses Metal exclusively.
pub fn native_backends() -> wgpu::Backends {
    if cfg!(target_os = "macos") {
        wgpu::Backends::METAL
    } else if cfg!(target_os = "windows") {
        wgpu::Backends::DX12 | wgpu::Backends::VULKAN
    } else {
        wgpu::Backends::VULKAN | wgpu::Backends::GL
    }
}
