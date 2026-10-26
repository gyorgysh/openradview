// SPDX-License-Identifier: GPL-3.0-or-later
//! Optional one-byte source mask. Dense volumes bind only a one-texel placeholder.
use orv_core::volume::Volume;

pub(super) fn upload(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    volume: &Volume,
    cancelled: &impl Fn() -> bool,
) -> Result<wgpu::TextureView, &'static str> {
    let needed = volume.has_missing_scalar();
    let [width, height, depth] = if needed { volume.dimensions() } else { [1; 3] };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("scalar volume validity"),
        size: wgpu::Extent3d {
            width: width as u32,
            height: height as u32,
            depth_or_array_layers: depth as u32,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D3,
        format: wgpu::TextureFormat::R8Uint,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    if needed {
        // At most one source plane of scratch space; no flattened CPU volume.
        let valid = if volume.frames().iter().any(|f| f.validity.is_none()) {
            vec![1; width * height]
        } else {
            Vec::new()
        };
        let mut pending = 0;
        for (z, frame) in volume.frames().iter().enumerate() {
            if cancelled() {
                return Err("Cancelled");
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
                frame.validity.as_deref().unwrap_or(&valid),
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(width as u32),
                    rows_per_image: Some(height as u32),
                },
                wgpu::Extent3d {
                    width: width as u32,
                    height: height as u32,
                    depth_or_array_layers: 1,
                },
            );
            pending += width * height;
            if pending >= 16 * 1024 * 1024 {
                super::flush_upload(device, queue, cancelled)?;
                pending = 0;
            }
        }
        if pending > 0 {
            super::flush_upload(device, queue, cancelled)?;
        }
    }
    Ok(texture.create_view(&Default::default()))
}
