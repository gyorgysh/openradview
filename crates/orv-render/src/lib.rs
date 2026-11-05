// SPDX-License-Identifier: GPL-3.0-or-later
//! UI-independent GPU rendering. Surface ownership belongs to the application.
mod backend;
pub mod brick_source;
pub mod label_source;
mod linear_display;
pub mod mask_input;
pub mod mask_projection;
pub mod material_ranges;
pub mod material_volume;
pub mod opaque_material;
pub mod opaque_surface;
pub mod paged_material;
pub mod paged_material_ranges;
pub mod reslice;
pub mod segment_source;
pub mod slice;
pub mod snapshot;
pub mod surface_pick;
pub mod surface_projection;
pub mod surface_source;
mod voi;
pub mod volume;
pub mod volume_source;
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

pub use volume::allocation::GpuReservation;

pub(crate) const VOLUME_SAMPLING_SHADER: &str = concat!(
    include_str!("volume_interval.wgsl"),
    "\n",
    include_str!("volume_sampling.wgsl")
);
