// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
pub struct GpuBrickPage {
    source: Arc<GpuBrickSource>,
    index: usize,
    brick: Brick,
    bindings: wgpu::BindGroup,
    pub(super) texture: wgpu::Texture,
    pub(super) validity: wgpu::Texture,
    uploaded: u64,
    _metadata: Reservation,
    _gpu: allocation::GpuReservation,
}
impl GpuBrickPage {
    pub(super) fn upload(
        source: Arc<GpuBrickSource>,
        index: usize,
        cancelled: &impl Fn() -> bool,
    ) -> Result<Arc<Self>, &'static str> {
        if cancelled() {
            return Err("Cancelled");
        }
        let brick = source
            .layout
            .brick(index)
            .ok_or("Invalid source brick index")?;
        let cells = source
            .layout
            .cells(index)
            .ok_or("Invalid source brick cells")?;
        let [w, h, d] = brick.size;
        let masked = source.source.has_missing_scalar();
        let gpu = allocation::reserve_bytes(
            source.resident_bytes(index)?,
            &source.device,
            &source.queue,
        )?;
        let metadata = MemoryBudget::volumes()
            .reserve((std::mem::size_of::<Self>() + 2 * std::mem::size_of::<usize>()) as u64)
            .ok_or("Not enough memory for the source brick page")?;
        let aligned_plane = (w * 4).next_multiple_of(256) * h
            + if masked {
                w.next_multiple_of(256) * h
            } else {
                0
            };
        let staging = allocation::reserve_bytes(
            (aligned_plane * d).min(16 * 1024 * 1024) as u64 + aligned_plane as u64,
            &source.device,
            &source.queue,
        )?;
        let _scratch = MemoryBudget::volumes()
            .reserve((w * h * if masked { 5 } else { 4 }) as u64)
            .ok_or("Not enough memory for source brick upload scratch")?;
        let mut values = Vec::new();
        values
            .try_reserve_exact(w * h)
            .map_err(|_| "Not enough memory for source brick scalar rows")?;
        let mut validity = Vec::new();
        if masked {
            validity
                .try_reserve_exact(w * h)
                .map_err(|_| "Not enough memory for source brick validity rows")?;
        }
        let descriptor = |label, format, size| wgpu::TextureDescriptor {
            label: Some(label),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D3,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        };
        let extent = wgpu::Extent3d {
            width: w as u32,
            height: h as u32,
            depth_or_array_layers: d as u32,
        };
        let texture = allocation::texture(
            &source.device,
            &descriptor(
                "immutable source brick scalars",
                wgpu::TextureFormat::R32Float,
                extent,
            ),
        )?;
        let mask_size = if masked {
            extent
        } else {
            wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            }
        };
        let mask = allocation::texture(
            &source.device,
            &descriptor(
                "immutable source brick validity",
                wgpu::TextureFormat::R8Uint,
                mask_size,
            ),
        )?;
        if !masked {
            source.queue.write_texture(
                mask.as_image_copy(),
                &[1],
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(1),
                    rows_per_image: Some(1),
                },
                mask_size,
            );
        }
        let [sw, _, _] = source.source.dimensions();
        let mut pending = 0;
        for local_z in 0..d {
            if cancelled() {
                return Err("Cancelled");
            }
            values.clear();
            validity.clear();
            let frame = &source.source.frames()[brick.origin[2] + local_z];
            for local_y in 0..h {
                if cancelled() {
                    return Err("Cancelled");
                }
                let start = (brick.origin[1] + local_y) * sw + brick.origin[0];
                let row = &frame.pixels[start..start + w];
                if row.iter().any(|v| !v.is_finite() || v.abs() > 1e20) {
                    return Err("Source brick scalar values exceed GPU arithmetic limits");
                }
                values.extend_from_slice(row);
                if masked {
                    validity.extend((start..start + w).map(|i| u8::from(frame.has_sample(i))));
                }
            }
            let mut target = texture.as_image_copy();
            target.origin.z = local_z as u32;
            let plane = wgpu::Extent3d {
                depth_or_array_layers: 1,
                ..extent
            };
            source.queue.write_texture(
                target,
                bytemuck::cast_slice(&values),
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(w as u32 * 4),
                    rows_per_image: Some(h as u32),
                },
                plane,
            );
            if masked {
                let mut target = mask.as_image_copy();
                target.origin.z = local_z as u32;
                source.queue.write_texture(
                    target,
                    &validity,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(w as u32),
                        rows_per_image: Some(h as u32),
                    },
                    plane,
                );
            }
            pending += aligned_plane;
            if pending >= 16 * 1024 * 1024 {
                flush_upload(&source.device, &source.queue, cancelled)?;
                pending = 0;
            }
        }
        flush_upload(&source.device, &source.queue, cancelled)?;
        drop(staging);
        if cancelled() {
            return Err("Cancelled");
        }
        let vector = |v: [usize; 3], flag| [v[0] as u32, v[1] as u32, v[2] as u32, flag];
        let words = [
            vector(source.source.dimensions(), u32::from(masked)),
            vector(
                cells[0],
                u32::from(source.layout.stencil() == BrickStencil::CentralSecants),
            ),
            vector(cells[1], 0),
            vector(brick.origin, 0),
            vector(brick.size, index as u32),
        ];
        let uniform = allocation::initialized_buffer(
            &source.device,
            "immutable source brick geometry",
            bytemuck::cast_slice(&words),
            wgpu::BufferUsages::UNIFORM,
        )?;
        let view = texture.create_view(&Default::default());
        let valid = mask.create_view(&Default::default());
        let memory = source
            .device
            .push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let validation = source
            .device
            .push_error_scope(wgpu::ErrorFilter::Validation);
        let bindings = source.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("source bound brick input"),
            layout: &source.bindings,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&valid),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: uniform.as_entire_binding(),
                },
            ],
        });
        let invalid = pollster::block_on(validation.pop()).is_some();
        let exhausted = pollster::block_on(memory.pop()).is_some();
        if invalid || exhausted {
            return Err("The GPU could not bind the source brick page");
        }
        if cancelled() {
            return Err("Cancelled");
        }
        let uploaded = (w * h * d) as u64 * if masked { 5 } else { 4 };
        Ok(Arc::new(Self {
            source,
            index,
            brick,
            bindings,
            texture,
            validity: mask,
            uploaded,
            _metadata: metadata,
            _gpu: gpu,
        }))
    }
    pub fn source(&self) -> &Arc<GpuBrickSource> {
        &self.source
    }
    pub fn index(&self) -> usize {
        self.index
    }
    pub fn brick(&self) -> Brick {
        self.brick
    }
    pub fn bindings(&self) -> &wgpu::BindGroup {
        &self.bindings
    }
    pub fn uploaded_bytes(&self) -> u64 {
        self.uploaded
    }
}
