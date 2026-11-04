// SPDX-License-Identifier: GPL-3.0-or-later
//! One admitted, immutable GPU source shared by volume and plane rendering.
use crate::volume::{allocation, flush_upload, validity};
use orv_core::volume::Volume;
use std::sync::Arc;

pub struct GpuVolumeSource {
    pub(crate) volume: Arc<Volume>,
    pub(crate) source: wgpu::TextureView,
    pub(crate) validity: wgpu::TextureView,
    pub(crate) voi: wgpu::TextureView,
    device: wgpu::Device,
    queue: wgpu::Queue,
    _reservation: allocation::GpuReservation,
}
impl GpuVolumeSource {
    /// Upload sorted slices on a cancellable worker, without flattening a second
    /// CPU volume. Retain this Arc only while a local view needs the source.
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        volume: Arc<Volume>,
        cancelled: impl Fn() -> bool,
    ) -> Result<Arc<Self>, &'static str> {
        let dimensions = volume.dimensions();
        let limit = device.limits().max_texture_dimension_3d as usize;
        if dimensions.iter().any(|d| *d > limit) {
            return Err("Volume dimensions exceed this GPU's 3D texture limit; use 2D/MPR");
        }
        let bytes_per_voxel = if volume.has_missing_scalar() { 5u64 } else { 4 };
        let bytes = dimensions
            .iter()
            .try_fold(bytes_per_voxel, |n, d| n.checked_mul(*d as u64));
        if bytes.is_none_or(|b| b > orv_core::index::MAX_VOLUME_BYTES) {
            return Err("Volume and validity exceed the GPU source budget; use 2D/MPR");
        }
        if cancelled() {
            return Err("Cancelled");
        }
        let reservation =
            allocation::reserve(dimensions, volume.has_missing_scalar(), device, queue)?;
        let texture = allocation::texture(
            device,
            &wgpu::TextureDescriptor {
                label: Some("affine scalar volume"),
                size: wgpu::Extent3d {
                    width: dimensions[0] as u32,
                    height: dimensions[1] as u32,
                    depth_or_array_layers: dimensions[2] as u32,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D3,
                format: if volume.is_color() {
                    wgpu::TextureFormat::Rgba8UnormSrgb
                } else {
                    wgpu::TextureFormat::R32Float
                },
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            },
        )?;
        let mut pending_bytes = 0usize;
        for (z, frame) in volume.frames().iter().enumerate() {
            if cancelled() {
                return Err("Cancelled");
            }
            if frame.pixels.iter().any(|v| v.abs() > 1e20) {
                return Err("Volume scalar values exceed GPU arithmetic limits");
            }
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: 0,
                        y: 0,
                        z: z as u32,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                frame
                    .rgba
                    .as_deref()
                    .unwrap_or_else(|| bytemuck::cast_slice(&frame.pixels)),
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(frame.width * 4),
                    rows_per_image: Some(frame.height),
                },
                wgpu::Extent3d {
                    width: frame.width,
                    height: frame.height,
                    depth_or_array_layers: 1,
                },
            );
            pending_bytes += frame.width as usize * frame.height as usize * 4;
            if pending_bytes >= 16 * 1024 * 1024 {
                flush_upload(device, queue, &cancelled)?;
                pending_bytes = 0;
            }
        }
        if pending_bytes > 0 {
            flush_upload(device, queue, &cancelled)?;
        }
        let source = texture.create_view(&Default::default());
        let validity = validity::upload(device, queue, &volume, &cancelled)?;
        let voi = crate::voi::upload(
            device,
            queue,
            volume.frames()[0].scalar.as_deref(),
            &cancelled,
        )?;
        if cancelled() {
            return Err("Cancelled");
        }
        Ok(Arc::new(Self {
            volume,
            source,
            validity,
            voi,
            device: device.clone(),
            queue: queue.clone(),
            _reservation: reservation,
        }))
    }
    pub fn volume(&self) -> &Arc<Volume> {
        &self.volume
    }
    /// Encoders using this source must be created on this device. Keeping the
    /// device/queue pair here avoids cross-instance handle equality ambiguities.
    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }
    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }
}
