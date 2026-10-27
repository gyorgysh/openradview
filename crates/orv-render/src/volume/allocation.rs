// SPDX-License-Identifier: GPL-3.0-or-later
//! Shared GPU admission and recoverable texture allocation, without study metadata.
use orv_core::resources::{MemoryBudget, Reservation};
mod hardware;

pub(crate) struct GpuReservation {
    memory: Option<Reservation>,
    queue: wgpu::Queue,
}
impl Drop for GpuReservation {
    fn drop(&mut self) {
        if let Some(memory) = self.memory.take() {
            // Flush pending uploads without waiting on the UI. Submitted GPU work
            // can retain resources after the app drops its renderer.
            self.queue.submit([]);
            self.queue.on_submitted_work_done(move || drop(memory));
        }
    }
}

pub(super) fn reserve(
    dimensions: [usize; 3],
    masked: bool,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> Result<GpuReservation, &'static str> {
    reserve_bytes(allocation_bytes(dimensions, masked)?, device, queue)
}
pub(crate) fn reserve_bytes(
    total: u64,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> Result<GpuReservation, &'static str> {
    let _ = device.poll(wgpu::PollType::Poll);
    let limits = device.limits();
    let budget = MemoryBudget::gpu();
    let device_budget = if limits.max_texture_dimension_3d < 1024 {
        512 * 1024 * 1024
    } else {
        hardware::ceiling(&device.adapter_info(), budget.used())
    }
    .min(budget.limit());
    if total > device_budget {
        return Err(
            "Rendering buffers exceed this device's GPU budget; close another view or open a smaller source",
        );
    }
    let memory = budget
        .reserve_with_limit(total, device_budget)
        .ok_or("Active views exceed the shared GPU budget; close another view or use 2D/MPR")?;
    Ok(GpuReservation {
        memory: Some(memory),
        queue: queue.clone(),
    })
}

fn allocation_bytes(dimensions: [usize; 3], masked: bool) -> Result<u64, &'static str> {
    let source = dimensions
        .into_iter()
        .try_fold(if masked { 5u64 } else { 4 }, |bytes, side| {
            bytes.checked_mul(side as u64)
        })
        .ok_or("Volume allocation size overflow; use 2D/MPR")?;
    // Include retired outputs, one upload chunk plus its last source plane, and
    // table staging. Driver allocation overhead is outside this admission policy.
    let plane = (dimensions[0] as u64)
        .checked_mul(dimensions[1] as u64)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or("Volume staging size overflow")?;
    source
        .checked_add(plane)
        .and_then(|bytes| {
            bytes.checked_add(
                3 * 2 * 1024 * 1024 * 4
                    + 16 * 1024 * 1024
                    + 2 * crate::voi::ATLAS_BYTES
                    + 65536
                    + 1,
            )
        })
        .ok_or("Volume allocation size overflow")
}

pub(crate) fn texture(
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
        let (device, queue) =
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
        let budget = MemoryBudget::new(100);
        let memory = budget.reserve(60).unwrap();
        let retained = GpuReservation {
            memory: Some(memory),
            queue: queue.clone(),
        };
        assert_eq!(budget.used(), 60);
        drop(retained);
        device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: Some(std::time::Duration::from_secs(30)),
            })
            .unwrap();
        assert_eq!(budget.used(), 0);
    }
    #[test]
    fn gpu_admission_counts_masks_outputs_and_constrained_devices() {
        assert!(allocation_bytes([usize::MAX, 2, 2], false).is_err());
        assert!(allocation_bytes([512, 512, 505], false).unwrap() > 512 * 1024 * 1024);
        assert!(allocation_bytes([512, 512, 410], true).unwrap() > 512 * 1024 * 1024);
        assert!(allocation_bytes([16; 3], false).unwrap() < 512 * 1024 * 1024);
    }
}
