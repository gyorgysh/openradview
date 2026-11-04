// SPDX-License-Identifier: GPL-3.0-or-later
//! Worker-only physical source integration ending at a known cell interface.
use crate::{
    linear_display::LinearDisplay,
    mask_projection::MAX_ROW_DISPATCHES,
    material_volume::Lighting,
    surface_projection::SurfaceImage,
    volume::{allocation, flush_upload},
    volume_source::GpuVolumeSource,
};
use orv_core::{
    ViewState,
    resources::{MemoryBudget, Reservation},
    volume_material::VolumeMaterial,
    volume_view::VolumeView,
};
use std::{
    ops::Range,
    sync::{Arc, mpsc},
};

#[derive(Clone)]
pub struct OpaqueSurfaceScene {
    pub surface: Arc<SurfaceImage>,
    pub navigation: ViewState,
    pub volume: VolumeView,
    pub material: Arc<VolumeMaterial>,
    pub lighting: Lighting,
    /// Two-sided headlight on the actual affine cell-face normal.
    pub surface_lighting: Lighting,
    /// Linear base surface luminance. Foreground source still attenuates it.
    pub surface_luminance: f32,
}
impl OpaqueSurfaceScene {
    fn same(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.surface, &other.surface)
            && self.navigation == other.navigation
            && self.volume == other.volume
            && Arc::ptr_eq(&self.material, &other.material)
            && self.lighting == other.lighting
            && self.surface_lighting == other.surface_lighting
            && self.surface_luminance == other.surface_luminance
    }
}
/// Complete, GPU-checked immutable linear image. This retains the exact surface,
/// source, material and view needed to validate a later physical pick.
pub struct OpaqueSurfaceImage {
    scene: OpaqueSurfaceScene,
    texture: wgpu::Texture,
    bindings: wgpu::BindGroup,
    display: Arc<LinearDisplay>,
    _reservation: allocation::GpuReservation,
    _metadata: Reservation,
}
impl OpaqueSurfaceImage {
    pub fn scene(&self) -> &OpaqueSurfaceScene {
        &self.scene
    }
    pub fn texture(&self) -> &wgpu::Texture {
        &self.texture
    }
    pub fn paint(&self, pass: &mut wgpu::RenderPass<'_>) {
        self.display.paint(&self.bindings, pass);
    }
}
struct Pending {
    image: OpaqueSurfaceImage,
    target: wgpu::TextureView,
    surface_cells: wgpu::TextureView,
    surface_depth: wgpu::TextureView,
    status: wgpu::Buffer,
    next_row: u32,
    dispatches: usize,
}
pub struct OpaqueSurfaceRenderer {
    source: Arc<GpuVolumeSource>,
    pipeline: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
    display: Arc<LinearDisplay>,
    pending: [Option<Pending>; 2],
    _scratch: allocation::GpuReservation,
}
impl OpaqueSurfaceRenderer {
    pub fn new(
        source: Arc<GpuVolumeSource>,
        format: wgpu::TextureFormat,
    ) -> Result<Self, &'static str> {
        let device = source.device();
        let limits = device.limits();
        crate::volume::check_limits(&limits)?;
        if source.volume().is_color() {
            return Err("Opaque material composition requires a scalar source");
        }
        if limits.max_storage_buffers_per_shader_stage < 1
            || limits.max_storage_buffer_binding_size < 4
            || limits.max_sampled_textures_per_shader_stage < 4
            || limits.max_uniform_buffers_per_shader_stage < 2
            || limits.max_uniform_buffer_binding_size < 1344
            || limits.max_buffer_size < 1344
            || limits.max_bindings_per_bind_group < 8
            || limits.max_bind_groups < 1
        {
            return Err("This GPU cannot bind the checked opaque material resources");
        }
        let scratch = allocation::reserve_bytes(65536, device, source.queue())?;
        let memory = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let layout = composition_layout(device, wgpu::TextureFormat::Rgba16Float);
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("checked opaque material layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("ordered source and opaque cell interface optics"),
            source: wgpu::ShaderSource::Wgsl(shader(include_str!("opaque_surface.wgsl")).into()),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("source before opaque cell interface"),
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
            return Err("The graphics device could not prepare opaque surface composition");
        }
        Ok(Self {
            source,
            pipeline,
            layout,
            display,
            pending: Default::default(),
            _scratch: scratch,
        })
    }
    /// Worker-only: retain a completed immutable surface image. Each row batch
    /// is submitted here; the final batch checks a
    /// bounded GPU status readback before returning an image. Never call on UI.
    pub fn render_rows(
        &mut self,
        scene: &OpaqueSurfaceScene,
        slot: usize,
        rows: Range<u32>,
        cancelled: impl Fn() -> bool,
    ) -> Result<Option<Arc<OpaqueSurfaceImage>>, &'static str> {
        if slot >= self.pending.len() {
            return Err("Unsupported opaque material slot");
        }
        if cancelled() {
            self.cancel(slot)?;
            return Err("Cancelled");
        }
        let request = scene.surface.request();
        let size = request.size;
        if !Arc::ptr_eq(request.surface.source(), &self.source)
            || request.camera != scene.volume.camera
            || request.display != scene.navigation
            || request.clip != scene.volume.clip
        {
            return Err("Source and surface views no longer match");
        }
        if !scene.surface_luminance.is_finite() || !(0. ..=1.).contains(&scene.surface_luminance) {
            return Err("Opaque surface luminance must be finite and between zero and one");
        }
        if rows.start >= rows.end || rows.end > size[1] {
            return Err("Opaque material rows exceed the image");
        }
        scene.surface_lighting.validate()?;
        let parameters = parameters(
            &self.source,
            scene,
            [[0, rows.start], [size[0], rows.end - rows.start]],
        )?;
        let device = self.source.device();
        if [size[0].div_ceil(8), (rows.end - rows.start).div_ceil(8)]
            .into_iter()
            .any(|v| v > device.limits().max_compute_workgroups_per_dimension)
        {
            return Err("Opaque material batch exceeds this GPU's dispatch limit");
        }
        if rows.start == 0 {
            if self.pending[slot].is_some() {
                return Err("Finish or cancel the pending opaque image first");
            }
            self.pending[slot] = Some(self.allocate(scene)?);
        }
        let pending = self.pending[slot]
            .as_mut()
            .ok_or("Start opaque material at the first row")?;
        if !pending.image.scene.same(scene) || pending.next_row != rows.start {
            return Err("Opaque scene or rows no longer match");
        }
        if pending.dispatches >= MAX_ROW_DISPATCHES {
            return Err("Opaque image has too many row batches");
        }
        let uniform = allocation::initialized_buffer(
            device,
            "immutable ordered material parameters",
            bytemuck::cast_slice(&parameters),
            wgpu::BufferUsages::UNIFORM,
        )?;
        let material = allocation::initialized_buffer(
            device,
            "immutable opaque cell interface material",
            bytemuck::cast_slice(&[
                scene.surface_luminance,
                f32::from(request.surface.surface().key().number),
                scene.surface_lighting.ambient,
                scene.surface_lighting.diffuse,
            ]),
            wgpu::BufferUsages::UNIFORM,
        )?;
        let bindings = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ordered opaque material dispatch"),
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
                    resource: wgpu::BindingResource::TextureView(&pending.target),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&self.source.validity),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(&pending.surface_cells),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::TextureView(&pending.surface_depth),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: pending.status.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: material.as_entire_binding(),
                },
            ],
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("checked opaque rows"),
        });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("ordered opaque material batch"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bindings, &[]);
            pass.dispatch_workgroups(size[0].div_ceil(8), (rows.end - rows.start).div_ceil(8), 1);
        }
        self.source.queue().submit([encoder.finish()]);
        pending.next_row = rows.end;
        pending.dispatches += 1;
        if rows.end == size[1] {
            let pending = self.pending[slot]
                .take()
                .ok_or("Opaque output unavailable")?;
            self.check(&pending, &cancelled)?;
            return Ok(Some(Arc::new(pending.image)));
        }
        Ok(None)
    }
    fn allocate(&self, scene: &OpaqueSurfaceScene) -> Result<Pending, &'static str> {
        let device = self.source.device();
        let size = scene.surface.request().size;
        // 256 batches: aligned 1344-byte material and 16-byte surface uniforms,
        // plus bounded binding metadata. Driver overhead is outside admission.
        let reservation = allocation::reserve_bytes(
            u64::from(size[0]) * u64::from(size[1]) * 8 + 512 * 1024 + 4,
            device,
            self.source.queue(),
        )?;
        let metadata = MemoryBudget::volumes()
            .reserve((std::mem::size_of::<Pending>() + 2 * std::mem::size_of::<usize>()) as u64)
            .ok_or("Not enough memory for the opaque surface image identity")?;
        let texture = allocation::texture(
            device,
            &wgpu::TextureDescriptor {
                label: Some("checked source and opaque cell interface radiance"),
                size: wgpu::Extent3d {
                    width: size[0],
                    height: size[1],
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
        let bindings = self.display.bindings(device, &target);
        let status = allocation::initialized_buffer(
            device,
            "opaque surface validation flag",
            &[0; 4],
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        )?;
        Ok(Pending {
            image: OpaqueSurfaceImage {
                scene: scene.clone(),
                texture,
                bindings,
                display: self.display.clone(),
                _reservation: reservation,
                _metadata: metadata,
            },
            target,
            surface_cells: scene.surface.cells().create_view(&Default::default()),
            surface_depth: scene.surface.depth().create_view(&Default::default()),
            status,
            next_row: 0,
            dispatches: 0,
        })
    }
    fn check(&self, pending: &Pending, cancelled: &impl Fn() -> bool) -> Result<(), &'static str> {
        let device = self.source.device();
        let queue = self.source.queue();
        let _gpu = allocation::reserve_bytes(4, device, queue)?;
        let _cpu = MemoryBudget::volumes()
            .reserve(4)
            .ok_or("Not enough memory for the opaque status check")?;
        let buffer = allocation::buffer(
            device,
            &wgpu::BufferDescriptor {
                label: Some("opaque material status readback"),
                size: 4,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            },
        )?;
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.copy_buffer_to_buffer(&pending.status, 0, &buffer, 0, 4);
        queue.submit([encoder.finish()]);
        let (sender, receiver) = mpsc::sync_channel(1);
        buffer.map_async(wgpu::MapMode::Read, .., move |result| {
            let _ = sender.send(result);
        });
        flush_upload(device, queue, cancelled)?;
        receiver
            .try_recv()
            .map_err(|_| "Opaque status readback unavailable")?
            .map_err(|_| "Opaque status readback failed")?;
        let data = buffer
            .get_mapped_range(..)
            .map_err(|_| "Opaque status mapping failed")?;
        let word: [u8; 4] = data
            .get(..4)
            .ok_or("Opaque status length is invalid")?
            .try_into()
            .map_err(|_| "Opaque status length is invalid")?;
        let failed = u32::from_le_bytes(word) != 0;
        drop(data);
        buffer.unmap();
        if failed {
            return Err(
                "Surface composition arithmetic could not resolve this view; no image was published",
            );
        }
        if cancelled() {
            return Err("Cancelled");
        }
        Ok(())
    }
    pub fn cancel(&mut self, slot: usize) -> Result<(), &'static str> {
        self.pending
            .get_mut(slot)
            .ok_or("Unsupported opaque material slot")?
            .take();
        Ok(())
    }
}

pub(crate) fn composition_layout(
    device: &wgpu::Device,
    output_format: wgpu::TextureFormat,
) -> wgpu::BindGroupLayout {
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
        label: Some("source and opaque cell interface bindings"),
        entries: &[
            texture(
                0,
                wgpu::TextureSampleType::Float { filterable: false },
                wgpu::TextureViewDimension::D3,
            ),
            buffer(1, wgpu::BufferBindingType::Uniform, 1344),
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::StorageTexture {
                    access: wgpu::StorageTextureAccess::WriteOnly,
                    format: output_format,
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
        ],
    })
}
pub(crate) fn shader(entry: &str) -> String {
    format!(
        "{}\n{}\n{}\n{}\n{}\n{}\n{}",
        include_str!("pixel_ray.wgsl"),
        include_str!("volume_sampling.wgsl"),
        crate::material_volume::optics_shader(false, false),
        include_str!("opaque_interval.wgsl"),
        include_str!("surface_optics.wgsl"),
        include_str!("surface_sample.wgsl"),
        entry
    )
}

pub(crate) fn parameters(
    source: &GpuVolumeSource,
    scene: &OpaqueSurfaceScene,
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
    let frame = crate::surface_projection::centred_frame(request)?;
    let volume = source.volume();
    let delta = volume.world_vector_to_voxel(frame.origin_mm);
    let origin: [f64; 3] =
        std::array::from_fn(|a| (volume.dimensions()[a] - 1) as f64 * 0.5 + delta[a]);
    for (a, v) in origin.into_iter().enumerate() {
        values[a] = v as f32;
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
            for (a, v) in vector.into_iter().enumerate() {
                values[36 + i * 4 + a] = v as f32;
            }
        }
        if (0..3).any(|a| values[52 + a] >= values[56 + a]) {
            return Err("Surface composition clip exceeds GPU coordinate precision");
        }
    }
    if values.iter().any(|v| !v.is_finite() || v.abs() > 1e20) {
        return Err("Surface composition exceeds GPU arithmetic limits");
    }
    Ok(values)
}

#[cfg(all(test, feature = "gpu-tests"))]
mod tests;
