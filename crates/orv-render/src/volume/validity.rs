// SPDX-License-Identifier: GPL-3.0-or-later
//! Packed source-stencil validity. Dense volumes bind a one-texel placeholder.
use orv_core::volume::Volume;

pub(crate) fn upload(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    volume: &Volume,
    cancelled: &impl Fn() -> bool,
) -> Result<wgpu::TextureView, &'static str> {
    let needed = volume.has_missing_scalar();
    let [width, height, depth] = if needed { volume.dimensions() } else { [1; 3] };
    let texture = super::allocation::texture(
        device,
        &wgpu::TextureDescriptor {
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
        },
    )?;
    if needed {
        // One admitted plane holds the eight clamped source-vertex validity
        // bits for each lower interpolation node; no flattened CPU volume.
        let plane = width * height;
        let _scratch = orv_core::resources::MemoryBudget::volumes()
            .reserve(plane as u64)
            .ok_or("Not enough memory for validity staging")?;
        let mut packed = Vec::new();
        packed
            .try_reserve_exact(plane)
            .map_err(|_| "Not enough memory for validity staging")?;
        if packed.capacity() != plane {
            return Err("Validity staging exceeds its admitted size");
        }
        packed.resize(plane, 0);
        let mut pending = 0;
        for (z, frame) in volume.frames().iter().enumerate() {
            if cancelled() {
                return Err("Cancelled");
            }
            let upper = &volume.frames()[(z + 1).min(depth - 1)];
            pack_plane(
                frame.validity.as_deref(),
                upper.validity.as_deref(),
                width,
                height,
                &mut packed,
                cancelled,
            )?;
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
                &packed,
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

// Bit x+2*y+4*z corresponds to the clamped neighboring source vertex.
// Bit zero always retains the original single-voxel validity.
fn pack_plane(
    lower: Option<&[u8]>,
    upper: Option<&[u8]>,
    width: usize,
    height: usize,
    output: &mut [u8],
    cancelled: &impl Fn() -> bool,
) -> Result<(), &'static str> {
    let plane = width
        .checked_mul(height)
        .ok_or("Invalid validity dimensions")?;
    if width == 0
        || height == 0
        || output.len() != plane
        || lower.is_some_and(|v| v.len() != plane)
        || upper.is_some_and(|v| v.len() != plane)
    {
        return Err("Invalid validity dimensions");
    }
    let get = |data: Option<&[u8]>, index: usize| -> u8 { data.map_or(1, |data| data[index]) };
    for y in 0..height {
        if cancelled() {
            return Err("Cancelled");
        }
        let row = y * width;
        let next_row = (y + 1).min(height - 1) * width;
        for x in 0..width {
            let next_x = (x + 1).min(width - 1);
            let a = row + x;
            let b = row + next_x;
            let c = next_row + x;
            let d = next_row + next_x;
            output[a] = get(lower, a)
                | (get(lower, b) << 1)
                | (get(lower, c) << 2)
                | (get(lower, d) << 3)
                | (get(upper, a) << 4)
                | (get(upper, b) << 5)
                | (get(upper, c) << 6)
                | (get(upper, d) << 7);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::pack_plane;
    #[test]
    fn packed_stencils_match_all_corner_patterns_and_clamped_edges() {
        for pattern in 0..=255u16 {
            let lower = std::array::from_fn::<_, 4, _>(|i| ((pattern >> i) & 1) as u8);
            let upper = std::array::from_fn::<_, 4, _>(|i| ((pattern >> (i + 4)) & 1) as u8);
            let mut output = [0; 4];
            pack_plane(Some(&lower), Some(&upper), 2, 2, &mut output, &|| false).unwrap();
            assert_eq!(output[0], pattern as u8);
            for y in 0..2 {
                for x in 0..2 {
                    let mut expected = 0;
                    for z in 0..2 {
                        for dy in 0..2 {
                            for dx in 0..2 {
                                let source = if z == 0 { &lower } else { &upper };
                                let index = (y + dy).min(1) * 2 + (x + dx).min(1);
                                expected |= source[index] << (dx + 2 * dy + 4 * z);
                            }
                        }
                    }
                    assert_eq!(output[y * 2 + x], expected, "pattern {pattern}, {x},{y}");
                }
            }
        }
        let mut output = [0; 4];
        pack_plane(None, None, 2, 2, &mut output, &|| false).unwrap();
        assert_eq!(output, [255; 4]);
        let mut single = [0];
        pack_plane(Some(&[0]), None, 1, 1, &mut single, &|| false).unwrap();
        assert_eq!(single, [0xf0]);
        pack_plane(None, Some(&[0]), 1, 1, &mut single, &|| false).unwrap();
        assert_eq!(single, [0x0f]);
        assert!(pack_plane(None, None, 2, 2, &mut output, &|| true).is_err());
        assert!(pack_plane(Some(&[1]), None, 2, 2, &mut output, &|| false).is_err());
    }
}
