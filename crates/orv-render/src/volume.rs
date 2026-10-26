// SPDX-License-Identifier: GPL-3.0-or-later
//! GPU-only ray marching output. Construction/upload belongs on a cancellable worker.
use orv_core::{ViewState, volume::Volume, volume_view::VolumeView};
use std::sync::Arc;
use wgpu::util::DeviceExt;
mod validity;

pub fn check_limits(limits: &wgpu::Limits) -> Result<(), &'static str> {
    if limits.max_compute_workgroup_size_x < 8
        || limits.max_compute_workgroup_size_y < 8
        || limits.max_compute_workgroup_size_z < 1
        || limits.max_compute_invocations_per_workgroup < 64
        || limits.max_compute_workgroups_per_dimension < 1
        || limits.max_storage_textures_per_shader_stage < 1
    {
        return Err(
            "3D / MIP needs compute shaders, which this graphics driver does not provide. Use 2D/MPR, or update the graphics driver.",
        );
    }
    Ok(())
}

struct Output {
    size: [u32; 2],
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    display: wgpu::BindGroup,
}

/// Two outputs share one uploaded source: interactive and idle refinement.
pub struct VolumeTarget {
    pub slot: usize,
    pub size: [u32; 2],
    pub region: [[u32; 2]; 2],
}

pub struct VolumeRenderer {
    volume: Arc<Volume>,
    source: wgpu::TextureView,
    validity: wgpu::TextureView,
    compute: wgpu::ComputePipeline,
    compute_layout: wgpu::BindGroupLayout,
    display: wgpu::RenderPipeline,
    display_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    color_space: wgpu::Buffer,
    outputs: [Option<Output>; 2],
}
impl VolumeRenderer {
    /// Upload one sorted slice at a time. No second full CPU volume allocation.
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        volume: Arc<Volume>,
        format: wgpu::TextureFormat,
        cancelled: impl Fn() -> bool,
    ) -> Result<Self, &'static str> {
        check_limits(&device.limits())?;
        let dimensions = volume.dimensions();
        let limit = device.limits().max_texture_dimension_3d as usize;
        if dimensions.iter().any(|d| *d > limit) {
            return Err("Volume dimensions exceed this GPU's 3D texture limit; use 2D/MPR");
        }
        let bytes_per_voxel = if volume.has_missing_scalar() { 5u64 } else { 4 };
        let bytes = dimensions
            .iter()
            .try_fold(bytes_per_voxel, |n, d| n.checked_mul(*d as u64));
        if bytes.is_none_or(|b| b > orv_core::index::VOLUME_BYTES) {
            return Err("Volume and validity exceed the GPU source budget; use 2D/MPR");
        }
        if cancelled() {
            return Err("Cancelled");
        }
        let texture = device.create_texture(&wgpu::TextureDescriptor {
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
        });
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
        let compute_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("volume compute bindings"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Uint,
                        view_dimension: wgpu::TextureViewDimension::D3,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
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
                        min_binding_size: wgpu::BufferSize::new(128),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("volume compute layout"),
            bind_group_layouts: &[Some(&compute_layout)],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("physical volume rays"),
            source: wgpu::ShaderSource::Wgsl(include_str!("volume.wgsl").into()),
        });
        let compute = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("physical ray marcher"),
            layout: Some(&layout),
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        let display_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("volume display bindings"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(16),
                    },
                    count: None,
                },
            ],
        });
        let display_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("volume texture display"),
            source: wgpu::ShaderSource::Wgsl(include_str!("volume_display.wgsl").into()),
        });
        let display_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("volume display layout"),
                bind_group_layouts: &[Some(&display_layout)],
                immediate_size: 0,
            });
        let display = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("volume display"),
            layout: Some(&display_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &display_shader,
                entry_point: Some("vs"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &display_shader,
                entry_point: Some("fs"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("volume output interpolation"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let color_space = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("volume display color space"),
            contents: bytemuck::cast_slice(&[u8::from(format.is_srgb()) as f32, 0., 0., 0.]),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        if cancelled() {
            return Err("Cancelled");
        }
        Ok(Self {
            volume,
            source,
            validity,
            compute,
            compute_layout,
            display,
            display_layout,
            sampler,
            color_space,
            outputs: [None, None],
        })
    }

    /// Encode a complete bounded output. The caller submits; no readback or wait.
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        display: &ViewState,
        state: &VolumeView,
        size: [u32; 2],
    ) -> Result<(), &'static str> {
        self.render_region(device, encoder, display, state, size, [[0, 0], size])
    }
    /// Tiles allow idle refinement in bounded GPU batches. Do not present an
    /// incomplete new target as a completed scene; keep the interactive output.
    pub fn render_region(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        display: &ViewState,
        state: &VolumeView,
        size: [u32; 2],
        region: [[u32; 2]; 2],
    ) -> Result<(), &'static str> {
        self.render_target(
            device,
            encoder,
            display,
            state,
            VolumeTarget {
                slot: 0,
                size,
                region,
            },
        )
    }
    pub fn render_target(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        display: &ViewState,
        state: &VolumeView,
        target: VolumeTarget,
    ) -> Result<(), &'static str> {
        let VolumeTarget { slot, size, region } = target;
        if slot >= self.outputs.len() {
            return Err("Unsupported volume output slot");
        }
        let params = crate::volume_uniform::parameters(&self.volume, display, state, size, region)?;
        self.ensure_output(device, size, slot);
        let output = self.outputs[slot]
            .as_ref()
            .ok_or("Volume output unavailable")?;
        // Immutable per-dispatch uniforms prevent later tiles/views replacing an
        // earlier command's parameters before the shared encoder is submitted.
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("volume ray parameters"),
            contents: bytemuck::cast_slice(&params),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let bindings = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("volume dispatch"),
            layout: &self.compute_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&self.validity),
                },
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&self.source),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&output.view),
                },
            ],
        });
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("volume ray tile"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.compute);
        pass.set_bind_group(0, &bindings, &[]);
        pass.dispatch_workgroups(region[1][0].div_ceil(8), region[1][1].div_ceil(8), 1);
        Ok(())
    }
    fn ensure_output(&mut self, device: &wgpu::Device, size: [u32; 2], slot: usize) {
        if self.outputs[slot].as_ref().is_some_and(|o| o.size == size) {
            return;
        }
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("volume GPU output"),
            size: wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[wgpu::TextureFormat::Rgba8UnormSrgb],
        });
        let view = texture.create_view(&Default::default());
        let display_view = texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(wgpu::TextureFormat::Rgba8UnormSrgb),
            usage: Some(wgpu::TextureUsages::TEXTURE_BINDING),
            ..Default::default()
        });
        let display = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("volume output display"),
            layout: &self.display_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&display_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: self.color_space.as_entire_binding(),
                },
            ],
        });
        self.outputs[slot] = Some(Output {
            size,
            texture,
            view,
            display,
        });
    }
    pub fn output_texture(&self) -> Option<&wgpu::Texture> {
        self.output_texture_for(0)
    }
    pub fn output_texture_for(&self, slot: usize) -> Option<&wgpu::Texture> {
        self.outputs.get(slot)?.as_ref().map(|o| &o.texture)
    }
    pub fn paint(&self, pass: &mut wgpu::RenderPass<'_>) {
        self.paint_target(0, pass);
    }
    pub fn paint_target(&self, slot: usize, pass: &mut wgpu::RenderPass<'_>) {
        if let Some(Some(output)) = self.outputs.get(slot) {
            pass.set_pipeline(&self.display);
            pass.set_bind_group(0, &output.display, &[]);
            pass.draw(0..3, 0..1);
        }
    }
}

fn flush_upload(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    cancelled: &impl Fn() -> bool,
) -> Result<(), &'static str> {
    let submission = queue.submit([]);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        if cancelled() {
            return Err("Cancelled");
        }
        if std::time::Instant::now() >= deadline {
            return Err("GPU volume upload timed out");
        }
        match device.poll(wgpu::PollType::Wait {
            submission_index: Some(submission.clone()),
            timeout: Some(std::time::Duration::from_millis(50)),
        }) {
            Ok(_) => return Ok(()),
            Err(wgpu::PollError::Timeout) => (),
            Err(_) => return Err("GPU volume upload failed"),
        }
    }
}
