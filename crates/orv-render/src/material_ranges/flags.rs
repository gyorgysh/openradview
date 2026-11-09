// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
pub(crate) struct GpuRangeFlags {
    pub(super) ranges: Arc<VolumeRanges>,
    pub(super) material: Arc<VolumeMaterial>,
    pub(crate) layout: wgpu::BindGroupLayout,
    pub(crate) bindings: wgpu::BindGroup,
    pub(crate) transparent_count: usize,
    pub(crate) zero: Vec<u8>,
    _reservation: allocation::GpuReservation,
    _metadata: Reservation,
}
impl GpuRangeFlags {
    pub(crate) fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        ranges: Arc<VolumeRanges>,
        material: Arc<VolumeMaterial>,
        allowed: impl Fn(usize) -> bool,
        cancelled: &impl Fn() -> bool,
    ) -> Result<Self, &'static str> {
        if material.points().iter().any(|p| p.value.is_subnormal())
            || material
                .points()
                .windows(2)
                .any(|p| f64::from(p[1].value) - f64::from(p[0].value) < 1e-20)
        {
            return Err(
                "Material scalar knots are too close or small for GPU range classification",
            );
        }
        let counts = ranges.counts().map(|n| n as u32);
        if counts
            .iter()
            .any(|n| *n > device.limits().max_texture_dimension_3d)
        {
            return Err("Scalar range grid exceeds this GPU's texture limit");
        }
        let row = counts[0].div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
            * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let staging_bytes = row as usize * counts[1] as usize * counts[2] as usize;
        let staging_charge = MemoryBudget::volumes()
            .reserve(staging_bytes as u64)
            .ok_or("Not enough memory for material range staging")?;
        let metadata = MemoryBudget::volumes()
            .reserve(
                (std::mem::size_of::<Self>()
                    + 3 * std::mem::size_of::<usize>()
                    + ranges.summaries().len()) as u64,
            )
            .ok_or("Not enough memory for material range identity")?;
        let reservation = allocation::reserve_bytes(
            ranges.summaries().len() as u64 + staging_bytes as u64 + 16 + 512,
            device,
            queue,
        )?;
        let mut data = Vec::new();
        data.try_reserve_exact(staging_bytes)
            .map_err(|_| "Not enough memory for material range staging")?;
        data.resize(staging_bytes, 0);
        let mut zero = Vec::new();
        zero.try_reserve_exact(ranges.summaries().len())
            .map_err(|_| "Not enough memory for material range flags")?;
        zero.resize(ranges.summaries().len(), 0);
        let mut transparent_count = 0;
        for z in 0..counts[2] as usize {
            for y in 0..counts[1] as usize {
                if cancelled() {
                    return Err("Cancelled");
                }
                for x in 0..counts[0] as usize {
                    let index = x + counts[0] as usize * (y + counts[1] as usize * z);
                    let bounds = scalar_interval(ranges.summaries()[index]);
                    let transparent = zero_coefficients(&material, bounds) && allowed(index);
                    zero[index] = u8::from(transparent);
                    data[(z * counts[1] as usize + y) * row as usize + x] = u8::from(transparent);
                    transparent_count += usize::from(transparent);
                }
            }
        }
        let texture = allocation::texture(
            device,
            &wgpu::TextureDescriptor {
                label: Some("scalar zero extinction tiles"),
                size: wgpu::Extent3d {
                    width: counts[0],
                    height: counts[1],
                    depth_or_array_layers: counts[2],
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D3,
                format: wgpu::TextureFormat::R8Uint,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            },
        )?;
        let uniform = allocation::initialized_buffer(
            device,
            "scalar range cell side",
            bytemuck::cast_slice(&[ranges.side() as u32, 0, 0, 0]),
            wgpu::BufferUsages::UNIFORM,
        )?;
        let memory = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("scalar range bindings"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Uint,
                        view_dimension: wgpu::TextureViewDimension::D3,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(16),
                    },
                    count: None,
                },
            ],
        });
        let bindings = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("immutable scalar range flags"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(
                        &texture.create_view(&Default::default()),
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: uniform.as_entire_binding(),
                },
            ],
        });
        let invalid = pollster::block_on(validation.pop()).is_some();
        let exhausted = pollster::block_on(memory.pop()).is_some();
        if invalid || exhausted {
            return Err("The graphics device could not prepare material range bindings");
        }
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: Some(counts[1]),
            },
            texture.size(),
        );
        flush_upload(device, queue, &cancelled)?;
        drop(staging_charge);
        if cancelled() {
            return Err("Cancelled");
        }
        Ok(Self {
            ranges,
            material,
            layout,
            bindings,
            transparent_count,
            zero,
            _reservation: reservation,
            _metadata: metadata,
        })
    }
}
