// SPDX-License-Identifier: GPL-3.0-or-later
use image::{ExtendedColorType, ImageEncoder, codecs::png::PngEncoder};
use orv_core::resources::{MemoryBudget, Reservation};
use std::{
    io::{self, Write},
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};

struct Layout {
    row_bytes: u32,
    rgba_bytes: usize,
    gpu_bytes: u64,
    png_limit: usize,
    cpu_bytes: u64,
}
impl Layout {
    fn new([width, height]: [u32; 2]) -> Result<Self, &'static str> {
        if width == 0 || height == 0 || width > 1024 || height > 1024 {
            return Err("Snapshot readback dimensions exceed the bounded image limit");
        }
        // The dimension check bounds these products on all supported usize widths.
        let row_bytes = (width * 4).div_ceil(256) * 256;
        let rgba_bytes = width as usize * height as usize * 4;
        let png_limit = rgba_bytes * 2 + 1024 * 1024;
        // The pinned PNG encoder retains a compressed intermediate as well as
        // output, row filters and compression state. Returned bytes need a
        // separate retained-output admission at the artifact/cache owner.
        let cpu_bytes = (rgba_bytes + 2 * png_limit + 4 * 1024 * 1024) as u64;
        Ok(Self {
            row_bytes,
            rgba_bytes,
            gpu_bytes: u64::from(row_bytes) * u64::from(height),
            png_limit,
            cpu_bytes,
        })
    }
    fn admit(
        &self,
        cpu: &Arc<MemoryBudget>,
        gpu: &Arc<MemoryBudget>,
    ) -> Result<(Reservation, Reservation), &'static str> {
        let cpu = cpu
            .reserve(self.cpu_bytes)
            .ok_or("Not enough memory for snapshot encoding")?;
        let gpu = gpu
            .reserve(self.gpu_bytes)
            .ok_or("Not enough GPU allowance for snapshot readback")?;
        Ok((cpu, gpu))
    }
}
struct PngOutput<'a> {
    bytes: Vec<u8>,
    limit: usize,
    cancelled: &'a dyn Fn() -> bool,
}
impl Write for PngOutput<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if (self.cancelled)() {
            return Err(io::ErrorKind::Interrupted.into());
        }
        if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
            return Err(io::Error::other(
                "Encoded snapshot exceeds its byte allowance",
            ));
        }
        self.bytes
            .try_reserve_exact(bytes.len())
            .map_err(|_| io::Error::other("Not enough memory for encoded snapshot"))?;
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub(super) fn wait(
    device: &wgpu::Device,
    submission: Option<wgpu::SubmissionIndex>,
    cancelled: &impl Fn() -> bool,
) -> Result<(), &'static str> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if cancelled() || Instant::now() >= deadline {
            return Err("Snapshot cancelled or GPU work timed out");
        }
        match device.poll(wgpu::PollType::Wait {
            submission_index: submission.clone(),
            timeout: Some(Duration::from_millis(50)),
        }) {
            Ok(_) => return Ok(()),
            Err(wgpu::PollError::Timeout) => (),
            Err(_) => return Err("Snapshot GPU work failed"),
        }
    }
}

pub(super) fn capture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    target: &wgpu::Texture,
    size: [u32; 2],
    cancelled: &impl Fn() -> bool,
) -> Result<Vec<u8>, &'static str> {
    if cancelled() {
        return Err("Cancelled");
    }
    let layout = Layout::new(size)?;
    let (_cpu, _gpu) = layout.admit(&MemoryBudget::volumes(), &MemoryBudget::gpu())?;
    let [width, height] = size;
    let row_bytes = layout.row_bytes;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("snapshot readback"),
        size: u64::from(row_bytes) * u64::from(height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: target,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row_bytes),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([encoder.finish()]);
    let (tx, rx) = mpsc::sync_channel(1);
    buffer.map_async(wgpu::MapMode::Read, .., move |result| {
        let _ = tx.send(result);
    });
    wait(device, None, cancelled)?;
    rx.recv_timeout(Duration::from_millis(100))
        .map_err(|_| "Snapshot mapping did not finish")?
        .map_err(|_| "Snapshot mapping failed")?;
    let mut rgba = Vec::new();
    rgba.try_reserve_exact(layout.rgba_bytes)
        .map_err(|_| "Not enough memory for snapshot pixels")?;
    {
        let mapped = buffer
            .get_mapped_range(..)
            .map_err(|_| "Snapshot mapping failed")?;
        for row in mapped.chunks_exact(row_bytes as usize) {
            if cancelled() {
                return Err("Cancelled");
            }
            rgba.extend_from_slice(&row[..width as usize * 4]);
        }
    }
    buffer.unmap();
    if cancelled() {
        return Err("Cancelled");
    }
    let mut png = PngOutput {
        bytes: Vec::new(),
        limit: layout.png_limit,
        cancelled,
    };
    PngEncoder::new(&mut png)
        .write_image(&rgba, width, height, ExtendedColorType::Rgba8)
        .map_err(|_| "Could not encode the snapshot")?;
    if cancelled() {
        return Err("Cancelled");
    }
    Ok(png.bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn readback_padding_bounds_and_two_pool_admission_are_checked_before_allocation() {
        for size in [[0, 1], [1, 0], [1025, 1], [1, 1025], [u32::MAX, u32::MAX]] {
            assert!(Layout::new(size).is_err());
        }
        let layout = Layout::new([1, 1024]).unwrap();
        assert_eq!(layout.row_bytes, 256);
        assert_eq!(layout.rgba_bytes, 4096);
        assert_eq!(layout.gpu_bytes, 256 * 1024);
        let cpu = MemoryBudget::new(layout.cpu_bytes - 1);
        let gpu = MemoryBudget::new(layout.gpu_bytes - 1);
        assert!(layout.admit(&cpu, &gpu).is_err());
        assert_eq!((cpu.used(), gpu.used()), (0, 0));
        cpu.set_limit(layout.cpu_bytes);
        assert!(layout.admit(&cpu, &gpu).is_err());
        assert_eq!((cpu.used(), gpu.used()), (0, 0));
        gpu.set_limit(layout.gpu_bytes);
        let admitted = layout.admit(&cpu, &gpu).unwrap();
        assert_eq!(
            (cpu.used(), gpu.used()),
            (layout.cpu_bytes, layout.gpu_bytes)
        );
        drop(admitted);
        assert_eq!((cpu.used(), gpu.used()), (0, 0));
    }
    #[test]
    fn encoded_output_refuses_growth_and_cancel_without_appending_partial_writes() {
        let cancel = std::cell::Cell::new(false);
        let cancelled = || cancel.get();
        let mut output = PngOutput {
            bytes: Vec::new(),
            limit: 3,
            cancelled: &cancelled,
        };
        output.write_all(&[1, 2]).unwrap();
        assert!(output.write_all(&[3, 4]).is_err());
        assert_eq!(output.bytes, [1, 2]);
        output.write_all(&[3]).unwrap();
        cancel.set(true);
        assert_eq!(
            output.write(&[]).unwrap_err().kind(),
            io::ErrorKind::Interrupted
        );
        assert_eq!(output.bytes, [1, 2, 3]);
    }
    #[test]
    fn incompressible_full_size_pixels_fit_the_output_bound_and_keep_png_bytes_compatible() {
        let layout = Layout::new([1024, 1024]).unwrap();
        let mut seed = 0x1a2b3c4du32;
        let data: Vec<u8> = (0..layout.rgba_bytes)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                seed as u8
            })
            .collect();
        let mut output = PngOutput {
            bytes: Vec::new(),
            limit: layout.png_limit,
            cancelled: &|| false,
        };
        PngEncoder::new(&mut output)
            .write_image(&data, 1024, 1024, ExtendedColorType::Rgba8)
            .unwrap();
        assert!(output.bytes.len() > layout.rgba_bytes / 2);
        assert!(output.bytes.len() <= layout.png_limit);
        let restored = image::load_from_memory(&output.bytes).unwrap().to_rgba8();
        assert_eq!(restored.as_raw(), &data);
        let mut legacy = Vec::new();
        PngEncoder::new(&mut legacy)
            .write_image(&data[..64], 4, 4, ExtendedColorType::Rgba8)
            .unwrap();
        let mut bounded = PngOutput {
            bytes: Vec::new(),
            limit: layout.png_limit,
            cancelled: &|| false,
        };
        PngEncoder::new(&mut bounded)
            .write_image(&data[..64], 4, 4, ExtendedColorType::Rgba8)
            .unwrap();
        assert_eq!(bounded.bytes, legacy);
    }
}
