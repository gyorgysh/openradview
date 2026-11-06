// SPDX-License-Identifier: GPL-3.0-or-later
//! Ordered source optics with a partial candidate surface; never edit authority.
use crate::{
    candidate_projection::CandidateImage,
    linear_display::LinearDisplay,
    material_volume::Lighting,
    volume::{allocation, flush_upload},
    volume_source::GpuVolumeSource,
};
use orv_core::{
    ViewState,
    resources::{MemoryBudget, Reservation},
    volume_material::VolumeMaterial,
    volume_view::VolumeView,
};
use std::sync::{Arc, mpsc};

const BATCH_ROWS: u32 = 16;
const MATERIAL_BYTES: u64 = 1344;

#[derive(Clone)]
pub struct CandidateOpaqueScene {
    /// A partial candidate projection; missing surface strata remain absent.
    pub surface: Arc<CandidateImage>,
    pub navigation: ViewState,
    pub volume: VolumeView,
    pub material: Arc<VolumeMaterial>,
    pub lighting: Lighting,
    pub surface_lighting: Lighting,
    /// Linear surface luminance before foreground attenuation.
    pub surface_luminance: f32,
}
/// A partial preview retaining its candidate assembly and exact view. It does
/// not establish complete surface coverage or authorize picking or edits.
pub struct CandidateOpaqueImage {
    scene: CandidateOpaqueScene,
    texture: wgpu::Texture,
    bindings: wgpu::BindGroup,
    display: Arc<LinearDisplay>,
    _reservation: allocation::GpuReservation,
    _metadata: Reservation,
}
impl CandidateOpaqueImage {
    pub fn scene(&self) -> &CandidateOpaqueScene {
        &self.scene
    }
    pub fn texture(&self) -> &wgpu::Texture {
        &self.texture
    }
    pub fn paint(&self, pass: &mut wgpu::RenderPass<'_>) {
        self.display.paint(&self.bindings, pass);
    }
}

pub struct CandidateOpaqueRenderer {
    source: Arc<GpuVolumeSource>,
    pipeline: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
    display: Arc<LinearDisplay>,
    _scratch: allocation::GpuReservation,
}
impl CandidateOpaqueRenderer {
    /// Worker-only creation for a scalar source; no accelerated range bindings.
    pub fn new(
        source: Arc<GpuVolumeSource>,
        format: wgpu::TextureFormat,
    ) -> Result<Self, &'static str> {
        let device = source.device();
        let limits = device.limits();
        crate::volume::check_limits(&limits)?;
        if source.volume().is_color() {
            return Err("Candidate optical composition requires a scalar source");
        }
        if limits.max_storage_buffers_per_shader_stage < 1
            || limits.max_storage_buffer_binding_size < 4
            || limits.max_sampled_textures_per_shader_stage < 5
            || limits.max_uniform_buffers_per_shader_stage < 2
            || limits.max_uniform_buffer_binding_size < MATERIAL_BYTES
            || limits.max_buffer_size < MATERIAL_BYTES
            || limits.max_bindings_per_bind_group < 9
            || limits.max_bind_groups < 1
        {
            return Err("This GPU cannot bind candidate optical composition resources");
        }
        let scratch = allocation::reserve_bytes(65536, device, source.queue())?;
        let memory = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let layout = layout(device);
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("partial candidate optical layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("ordered source and candidate optics"),
            source: wgpu::ShaderSource::Wgsl(
                format!(
                    "{}\n{}\n{}\n{}\n{}\n{}\n{}",
                    include_str!("pixel_ray.wgsl"),
                    crate::VOLUME_SAMPLING_SHADER,
                    crate::material_volume::optics_shader(false, false),
                    include_str!("opaque_interval.wgsl"),
                    include_str!("surface_optics.wgsl"),
                    include_str!("candidate_sample.wgsl"),
                    include_str!("opaque_surface.wgsl"),
                )
                .into(),
            ),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("source before partial opaque candidate"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        let display = Arc::new(LinearDisplay::new(device, format));
        let invalid = pollster::block_on(validation.pop()).is_some();
        let exhausted = pollster::block_on(memory.pop()).is_some();
        if invalid || exhausted {
            return Err("The graphics device could not prepare candidate optical composition");
        }
        Ok(Self {
            source,
            pipeline,
            layout,
            display,
            _scratch: scratch,
        })
    }

    /// Worker-only; each bounded row batch is fenced. An image is published only
    /// after all rows and the GPU status check succeed.
    pub fn render(
        &self,
        scene: CandidateOpaqueScene,
        cancelled: impl Fn() -> bool,
    ) -> Result<Arc<CandidateOpaqueImage>, &'static str> {
        if cancelled() {
            return Err("Cancelled");
        }
        let request = scene.surface.request();
        if !Arc::ptr_eq(request.surface.source(), &self.source)
            || request.camera != scene.volume.camera
            || request.display != scene.navigation
            || request.clip != scene.volume.clip
            || scene.surface.depth_reference_mm() != self.source.volume().center()
        {
            return Err("Source and candidate views no longer match");
        }
        if !scene.surface_luminance.is_finite() || !(0. ..=1.).contains(&scene.surface_luminance) {
            return Err("Candidate luminance must be finite and between zero and one");
        }
        scene.surface_lighting.validate()?;
        let [width, height] = request.size;
        let device = self.source.device();
        let queue = self.source.queue();
        if [width.div_ceil(8), BATCH_ROWS.min(height).div_ceil(8)]
            .into_iter()
            .any(|v| v > device.limits().max_compute_workgroups_per_dimension)
        {
            return Err("Candidate optical batch exceeds this GPU's dispatch limit");
        }
        let reservation = allocation::reserve_bytes(
            u64::from(width) * u64::from(height) * 8 + 512 * 1024 + 4,
            device,
            queue,
        )?;
        let metadata = MemoryBudget::volumes()
            .reserve(
                (std::mem::size_of::<CandidateOpaqueImage>() + 2 * std::mem::size_of::<usize>())
                    as u64,
            )
            .ok_or("Not enough memory for candidate optical image identity")?;
        let texture = allocation::texture(
            device,
            &wgpu::TextureDescriptor {
                label: Some("partial candidate and source linear radiance"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba16Float,
                usage: wgpu::TextureUsages::STORAGE_BINDING
                    | wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            },
        )?;
        let target = texture.create_view(&Default::default());
        let display_bindings = self.display.bindings(device, &target);
        let cell_view = scene.surface.cells().create_view(&Default::default());
        let depth_view = scene.surface.depth().create_view(&Default::default());
        let normal_view = scene.surface.normals().create_view(&Default::default());
        let status = allocation::initialized_buffer(
            device,
            "candidate optical status",
            &[0; 4],
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        )?;
        let material = allocation::initialized_buffer(
            device,
            "candidate optical surface material",
            bytemuck::cast_slice(&[
                scene.surface_luminance,
                f32::from(scene.surface.key().number),
                scene.surface_lighting.ambient,
                scene.surface_lighting.diffuse,
            ]),
            wgpu::BufferUsages::UNIFORM,
        )?;
        for start in (0..height).step_by(BATCH_ROWS as usize) {
            if cancelled() {
                return Err("Cancelled");
            }
            let rows = BATCH_ROWS.min(height - start);
            let values = parameters(&self.source, &scene, [[0, start], [width, rows]])?;
            let uniform = allocation::initialized_buffer(
                device,
                "candidate optical parameters",
                bytemuck::cast_slice(&values),
                wgpu::BufferUsages::UNIFORM,
            )?;
            let bindings = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("partial candidate optical dispatch"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&self.source.source),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: uniform.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(&target),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::TextureView(&self.source.validity),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: wgpu::BindingResource::TextureView(&cell_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: wgpu::BindingResource::TextureView(&depth_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 6,
                        resource: status.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 7,
                        resource: material.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 8,
                        resource: wgpu::BindingResource::TextureView(&normal_view),
                    },
                ],
            });
            let mut encoder = device.create_command_encoder(&Default::default());
            {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("bounded candidate optical rows"),
                    timestamp_writes: None,
                });
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &bindings, &[]);
                pass.dispatch_workgroups(width.div_ceil(8), rows.div_ceil(8), 1);
            }
            queue.submit([encoder.finish()]);
            flush_upload(device, queue, &cancelled)?;
        }
        check_status(&self.source, &status, &cancelled)?;
        if cancelled() {
            return Err("Cancelled");
        }
        Ok(Arc::new(CandidateOpaqueImage {
            scene,
            texture,
            bindings: display_bindings,
            display: self.display.clone(),
            _reservation: reservation,
            _metadata: metadata,
        }))
    }
}

fn layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    let texture = |binding, sample_type, view_dimension| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Texture {
            sample_type,
            view_dimension,
            multisampled: false,
        },
        count: None,
    };
    let buffer = |binding, ty, bytes| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty,
            has_dynamic_offset: false,
            min_binding_size: wgpu::BufferSize::new(bytes),
        },
        count: None,
    };
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("partial candidate optical bindings"),
        entries: &[
            texture(
                0,
                wgpu::TextureSampleType::Float { filterable: false },
                wgpu::TextureViewDimension::D3,
            ),
            buffer(1, wgpu::BufferBindingType::Uniform, MATERIAL_BYTES),
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::StorageTexture {
                    access: wgpu::StorageTextureAccess::WriteOnly,
                    format: wgpu::TextureFormat::Rgba16Float,
                    view_dimension: wgpu::TextureViewDimension::D2,
                },
                count: None,
            },
            texture(
                3,
                wgpu::TextureSampleType::Uint,
                wgpu::TextureViewDimension::D3,
            ),
            texture(
                4,
                wgpu::TextureSampleType::Uint,
                wgpu::TextureViewDimension::D2,
            ),
            texture(
                5,
                wgpu::TextureSampleType::Float { filterable: false },
                wgpu::TextureViewDimension::D2,
            ),
            buffer(6, wgpu::BufferBindingType::Storage { read_only: false }, 4),
            buffer(7, wgpu::BufferBindingType::Uniform, 16),
            texture(
                8,
                wgpu::TextureSampleType::Float { filterable: false },
                wgpu::TextureViewDimension::D2,
            ),
        ],
    })
}
fn parameters(
    source: &GpuVolumeSource,
    scene: &CandidateOpaqueScene,
    region: [[u32; 2]; 2],
) -> Result<[f32; 336], &'static str> {
    let request = scene.surface.request();
    let mut values = crate::material_volume::parameters(
        source,
        &scene.navigation,
        &scene.volume,
        &scene.material,
        scene.lighting,
        request.size,
        region,
    )?;
    let volume = source.volume();
    let frame = crate::physical_camera::centred_frame(
        &request.camera,
        &request.display,
        request.size,
        volume.center(),
    )?;
    let delta = volume.world_vector_to_voxel(frame.origin_mm);
    for (axis, delta) in delta.into_iter().enumerate() {
        values[axis] = ((volume.dimensions()[axis] - 1) as f64 * 0.5 + delta) as f32;
    }
    if let Some(clip) = &scene.volume.clip {
        let reference = scene.surface.depth_reference_mm();
        for (i, vector) in [
            frame.origin_mm,
            frame.right_mm,
            frame.down_mm,
            frame.direction_unit,
            std::array::from_fn(|a| clip.minimum_mm[a] - reference[a]),
            std::array::from_fn(|a| clip.maximum_mm[a] - reference[a]),
        ]
        .into_iter()
        .enumerate()
        {
            for (axis, value) in vector.into_iter().enumerate() {
                values[36 + i * 4 + axis] = value as f32;
            }
        }
        if (0..3).any(|a| values[52 + a] >= values[56 + a]) {
            return Err("Candidate composition clip exceeds GPU coordinate precision");
        }
    }
    if values.iter().any(|v| !v.is_finite() || v.abs() > 1e20) {
        return Err("Candidate composition exceeds GPU arithmetic limits");
    }
    Ok(values)
}
fn check_status(
    source: &GpuVolumeSource,
    status: &wgpu::Buffer,
    cancelled: &impl Fn() -> bool,
) -> Result<(), &'static str> {
    let device = source.device();
    let queue = source.queue();
    let _gpu = allocation::reserve_bytes(4, device, queue)?;
    let _cpu = MemoryBudget::volumes()
        .reserve(4)
        .ok_or("Not enough memory for candidate status")?;
    let buffer = allocation::buffer(
        device,
        &wgpu::BufferDescriptor {
            label: Some("candidate optical status readback"),
            size: 4,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        },
    )?;
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_buffer_to_buffer(status, 0, &buffer, 0, 4);
    queue.submit([encoder.finish()]);
    let (sender, receiver) = mpsc::sync_channel(1);
    buffer.map_async(wgpu::MapMode::Read, .., move |result| {
        let _ = sender.send(result);
    });
    flush_upload(device, queue, cancelled)?;
    receiver
        .try_recv()
        .map_err(|_| "Candidate status readback unavailable")?
        .map_err(|_| "Candidate status readback failed")?;
    let data = buffer
        .get_mapped_range(..)
        .map_err(|_| "Candidate status mapping failed")?;
    let word: [u8; 4] = data
        .get(..4)
        .ok_or("Candidate status length is invalid")?
        .try_into()
        .map_err(|_| "Candidate status length is invalid")?;
    let failed = u32::from_le_bytes(word) != 0;
    drop(data);
    buffer.unmap();
    if failed {
        return Err("Candidate composition could not resolve this view; no image was published");
    }
    if cancelled() {
        return Err("Cancelled");
    }
    Ok(())
}
