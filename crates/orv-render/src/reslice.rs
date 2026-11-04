// SPDX-License-Identifier: GPL-3.0-or-later
//! Scalar plane/slab sampling on an admitted GPU source. No UI or readback.
use crate::{volume::allocation, volume_source::GpuVolumeSource};
use orv_core::{slab::SlabProjection, volume::ReformatGrid};
use std::sync::Arc;
use wgpu::util::DeviceExt;

struct Output {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    _reservation: allocation::GpuReservation,
}

/// Three independent outputs retain full requested grids during navigation.
/// Each texel is (scalar value, validity 0/1, 0, 1), before any display mapping.
/// Callers must keep output textures within this renderer's admitted lifetime.
pub struct GpuReslicer {
    source: Arc<GpuVolumeSource>,
    pipeline: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
    outputs: [Option<Output>; 3],
    _scratch: allocation::GpuReservation,
}
impl GpuReslicer {
    pub fn new(source: Arc<GpuVolumeSource>) -> Result<Self, &'static str> {
        let device = source.device();
        let queue = source.queue();
        crate::volume::check_limits(&device.limits())?;
        if source.volume.is_color() {
            return Err("GPU scalar reslicing does not accept color sources");
        }
        let scratch = allocation::reserve_bytes(65536, device, queue)?;
        let texture = |binding, sample_type| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Texture {
                sample_type,
                view_dimension: wgpu::TextureViewDimension::D3,
                multisampled: false,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("scalar reslice bindings"),
            entries: &[
                texture(0, wgpu::TextureSampleType::Float { filterable: false }),
                texture(1, wgpu::TextureSampleType::Uint),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(96),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: wgpu::TextureFormat::Rgba32Float,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("scalar reslice layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("scalar plane/slab sampling"),
            source: wgpu::ShaderSource::Wgsl(include_str!("reslice.wgsl").into()),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("scalar reslice"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        Ok(Self {
            source,
            pipeline,
            layout,
            outputs: Default::default(),
            _scratch: scratch,
        })
    }

    /// Encode one complete grid without a CPU wait. Uniforms are immutable per
    /// dispatch so several panes can safely share one command encoder, created
    /// on source.device(). Submit it before dropping the renderer.
    pub fn render(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        grid: &ReformatGrid,
        slot: usize,
    ) -> Result<(), &'static str> {
        self.render_region(encoder, grid, slot, [[0, 0], grid.dimensions()])
    }

    /// Encode a bounded tile. Present only after the requested set completes;
    /// callers can discard superseded work between submissions without blur.
    pub fn render_region(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        grid: &ReformatGrid,
        slot: usize,
        region: [[u32; 2]; 2],
    ) -> Result<(), &'static str> {
        let device = self.source.device();
        if slot >= self.outputs.len() {
            return Err("Unsupported reslice output slot");
        }
        let parameters = parameters(&self.source, grid, region)?;
        let size = grid.dimensions();
        if size
            .iter()
            .any(|n| *n > device.limits().max_texture_dimension_2d)
        {
            return Err("Reslice grid exceeds this GPU's texture limit");
        }
        if self.outputs[slot]
            .as_ref()
            .is_none_or(|o| [o.texture.width(), o.texture.height()] != size)
        {
            let reservation = allocation::reserve_bytes(
                u64::from(size[0]) * u64::from(size[1]) * 16,
                device,
                self.source.queue(),
            )?;
            let texture = allocation::texture(
                device,
                &wgpu::TextureDescriptor {
                    label: Some("scalar reslice values and validity"),
                    size: wgpu::Extent3d {
                        width: size[0],
                        height: size[1],
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba32Float,
                    usage: wgpu::TextureUsages::STORAGE_BINDING
                        | wgpu::TextureUsages::TEXTURE_BINDING
                        | wgpu::TextureUsages::COPY_SRC,
                    view_formats: &[],
                },
            )?;
            let view = texture.create_view(&Default::default());
            self.outputs[slot] = Some(Output {
                texture,
                view,
                _reservation: reservation,
            });
        }
        let output = self.outputs[slot]
            .as_ref()
            .ok_or("Reslice output unavailable")?;
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("immutable reslice grid"),
            contents: bytemuck::cast_slice(&parameters),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let bindings = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("scalar reslice dispatch"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&self.source.source),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&self.source.validity),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&output.view),
                },
            ],
        });
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("full-detail scalar plane"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &bindings, &[]);
        pass.dispatch_workgroups(region[1][0].div_ceil(8), region[1][1].div_ceil(8), 1);
        Ok(())
    }
    pub fn output_texture(&self, slot: usize) -> Option<&wgpu::Texture> {
        self.outputs.get(slot)?.as_ref().map(|o| &o.texture)
    }
}

fn parameters(
    source: &GpuVolumeSource,
    grid: &ReformatGrid,
    region: [[u32; 2]; 2],
) -> Result<[f32; 24], &'static str> {
    let volume = &source.volume;
    let geometry = grid.geometry();
    let reference = volume.frames()[0]
        .geometry
        .as_ref()
        .ok_or("Source geometry unavailable")?;
    if geometry.frame_of_reference != reference.frame_of_reference
        || geometry.nifti_transform != reference.nifti_transform
    {
        return Err("Reslice grid belongs to another spatial reference");
    }
    let u = std::array::from_fn(|i| geometry.orientation[i] * geometry.spacing[1]);
    let v = std::array::from_fn(|i| geometry.orientation[i + 3] * geometry.spacing[0]);
    if region[1].contains(&0)
        || (0..2).any(|i| {
            region[0][i]
                .checked_add(region[1][i])
                .is_none_or(|end| end > grid.dimensions()[i])
        })
    {
        return Err("Reslice tile exceeds its sampling grid");
    }
    let mut p = [0.; 24];
    p[20..24].copy_from_slice(&[
        region[0][0] as f32,
        region[0][1] as f32,
        region[1][0] as f32,
        region[1][1] as f32,
    ]);
    // Rebase in f64 before conversion, including sources far from the LPS origin.
    for (index, vector) in [
        volume.world_to_voxel(geometry.position),
        volume.world_vector_to_voxel(u),
        volume.world_vector_to_voxel(v),
        volume.world_vector_to_voxel(geometry.normal()),
    ]
    .into_iter()
    .enumerate()
    {
        for axis in 0..3 {
            p[index * 4 + axis] = vector[axis] as f32;
        }
    }
    p[3] = u8::from(volume.has_missing_scalar()) as f32;
    if let Some(slab) = grid.slab() {
        slab.validate()?;
        p[16] = slab.thickness_mm as f32;
        p[17] = f32::from(slab.intervals);
        p[18] = match slab.projection {
            SlabProjection::Maximum => 1.,
            SlabProjection::Minimum => 2.,
            SlabProjection::Mean => 3.,
        };
    }
    if p.iter().any(|n| !n.is_finite() || n.abs() > 1e20) {
        return Err("Reslice grid exceeds GPU arithmetic limits");
    }
    Ok(p)
}
