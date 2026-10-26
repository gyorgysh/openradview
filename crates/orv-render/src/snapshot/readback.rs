// SPDX-License-Identifier: GPL-3.0-or-later
use image::{ExtendedColorType, ImageEncoder, codecs::png::PngEncoder};
use std::{
    sync::mpsc,
    time::{Duration, Instant},
};

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
    let [width, height] = size;
    let row_bytes = (width * 4).div_ceil(256) * 256;
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
    let mut rgba = Vec::with_capacity(width as usize * height as usize * 4);
    {
        let mapped = buffer
            .get_mapped_range(..)
            .map_err(|_| "Snapshot mapping failed")?;
        for row in mapped.chunks_exact(row_bytes as usize) {
            rgba.extend_from_slice(&row[..width as usize * 4]);
        }
    }
    buffer.unmap();
    if cancelled() {
        return Err("Cancelled");
    }
    let mut png = Vec::new();
    PngEncoder::new(&mut png)
        .write_image(&rgba, width, height, ExtendedColorType::Rgba8)
        .map_err(|_| "Could not encode the snapshot")?;
    if cancelled() {
        return Err("Cancelled");
    }
    Ok(png)
}
