// SPDX-License-Identifier: GPL-3.0-or-later
//! Shared GPU admission and recoverable texture allocation, without study metadata.
use orv_core::resources::{MemoryBudget, Reservation};
mod hardware;

pub(super) fn reserve(
    dimensions: [usize; 3],
    masked: bool,
    device: &wgpu::Device,
) -> Result<Reservation, &'static str> {
    let limits = device.limits();
    let budget = MemoryBudget::gpu();
    let total = allocation_bytes(dimensions, masked)?;
    let device_budget = if limits.max_texture_dimension_3d < 1024 {
        512 * 1024 * 1024
    } else {
        hardware::ceiling(&device.adapter_info(), budget.used())
    }
    .min(budget.limit());
    if total > device_budget {
        return Err("Volume and output buffers exceed this device's GPU budget; use 2D/MPR");
    }
    budget
        .reserve_with_limit(total, device_budget)
        .ok_or("Active volume views exceed the shared GPU budget; close a 3D view or use 2D/MPR")
}

fn allocation_bytes(dimensions: [usize; 3], masked: bool) -> Result<u64, &'static str> {
    let source = dimensions
        .into_iter()
        .try_fold(if masked { 5u64 } else { 4 }, |bytes, side| {
            bytes.checked_mul(side as u64)
        })
        .ok_or("Volume allocation size overflow; use 2D/MPR")?;
    // Reserve both maximum outputs and bounded dispatch metadata before any GPU allocation.
    source
        .checked_add(2 * 1024 * 1024 * 4 + 65536 + 1)
        .ok_or("Volume allocation size overflow")
}

pub(super) fn texture(
    device: &wgpu::Device,
    descriptor: &wgpu::TextureDescriptor<'_>,
) -> Result<wgpu::Texture, &'static str> {
    let memory = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
    let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let texture = device.create_texture(descriptor);
    let invalid = pollster::block_on(validation.pop()).is_some();
    let exhausted = pollster::block_on(memory.pop()).is_some();
    if invalid || exhausted {
        Err("The graphics device cannot allocate this volume; use 2D/MPR or close another view")
    } else {
        Ok(texture)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "gpu-tests")]
    #[test]
    fn invalid_texture_allocation_returns_an_error_without_poisoning_the_device() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: crate::configured_backends().unwrap(),
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = pollster::block_on(instance.request_adapter(&Default::default()))
            .expect("Real GPU required");
        let (device, _) =
            pollster::block_on(adapter.request_device(&crate::device_descriptor(&adapter)))
                .unwrap();
        let mut descriptor = wgpu::TextureDescriptor {
            label: Some("synthetic allocation failure"),
            size: wgpu::Extent3d {
                width: 0,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        };
        assert!(texture(&device, &descriptor).is_err());
        descriptor.size.width = 1;
        assert!(texture(&device, &descriptor).is_ok());
    }
    #[test]
    fn gpu_admission_counts_masks_outputs_and_constrained_devices() {
        assert!(allocation_bytes([usize::MAX, 2, 2], false).is_err());
        assert!(allocation_bytes([512, 512, 505], false).unwrap() > 512 * 1024 * 1024);
        assert!(allocation_bytes([512, 512, 410], true).unwrap() > 512 * 1024 * 1024);
        assert!(allocation_bytes([16; 3], false).unwrap() < 512 * 1024 * 1024);
    }
}
