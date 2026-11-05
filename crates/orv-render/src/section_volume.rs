// SPDX-License-Identifier: GPL-3.0-or-later
//! Worker-only, depth-ordered source integration before completed thin MPR planes.
use crate::{
    linear_display::LinearDisplay,
    mask_projection::MAX_ROW_DISPATCHES,
    material_volume::Lighting,
    reslice::GpuReformatImage,
    volume::{allocation, flush_upload},
    volume_source::GpuVolumeSource,
};
use orv_core::{
    ViewState, WindowLevel,
    resources::{MemoryBudget, Reservation},
    volume_material::VolumeMaterial,
    volume_view::VolumeView,
};
use std::{
    ops::Range,
    sync::{Arc, mpsc},
};

/// An opaque, two-sided physical plane. Its pixels and validity belong to the
/// completed reformat, and its appearance excludes 2D pan, zoom and display flips.
#[derive(Clone)]
pub struct SectionPlane {
    pub image: Arc<GpuReformatImage>,
    pub window: WindowLevel,
    pub invert: bool,
}
#[derive(Clone)]
pub struct SectionScene {
    /// Equal-depth valid planes use the first slot. Invalid pixels are transparent.
    pub planes: [Option<SectionPlane>; 3],
    pub navigation: ViewState,
    pub volume: VolumeView,
    pub material: Arc<VolumeMaterial>,
    pub lighting: Lighting,
    pub size: [u32; 2],
}
impl SectionScene {
    fn same(&self, other: &Self) -> bool {
        self.planes
            .iter()
            .zip(&other.planes)
            .all(|(a, b)| match (a, b) {
                (None, None) => true,
                (Some(a), Some(b)) => {
                    Arc::ptr_eq(&a.image, &b.image) && a.window == b.window && a.invert == b.invert
                }
                _ => false,
            })
            && self.navigation == other.navigation
            && self.volume == other.volume
            && Arc::ptr_eq(&self.material, &other.material)
            && self.lighting == other.lighting
            && self.size == other.size
    }
}
/// Immutable complete radiance, retaining the exact source, planes and optics.
/// This image supplies no region-picking or mutation authority.
pub struct SectionImage {
    source: Arc<GpuVolumeSource>,
    scene: SectionScene,
    texture: wgpu::Texture,
    bindings: wgpu::BindGroup,
    display: Arc<LinearDisplay>,
    _gpu: allocation::GpuReservation,
    _metadata: Reservation,
}
impl SectionImage {
    pub fn source(&self) -> &Arc<GpuVolumeSource> {
        &self.source
    }
    pub fn scene(&self) -> &SectionScene {
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
    image: SectionImage,
    target: wgpu::TextureView,
    planes: [wgpu::TextureView; 3],
    status: wgpu::Buffer,
    next_row: u32,
    dispatches: usize,
}
pub struct SectionRenderer {
    source: Arc<GpuVolumeSource>,
    pipeline: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
    display: Arc<LinearDisplay>,
    empty: wgpu::Texture,
    pending: [Option<Pending>; 2],
    _gpu: allocation::GpuReservation,
}
impl SectionRenderer {
    pub fn new(
        source: Arc<GpuVolumeSource>,
        format: wgpu::TextureFormat,
    ) -> Result<Self, &'static str> {
        let device = source.device();
        crate::volume::check_limits(&device.limits())?;
        let limits = device.limits();
        if source.volume().is_color() {
            return Err("Source sections require a scalar volume");
        }
        if limits.max_sampled_textures_per_shader_stage < 6
            || limits.max_bindings_per_bind_group < 10
            || limits.max_uniform_buffers_per_shader_stage < 2
            || limits.max_uniform_buffer_binding_size < 1344
            || limits.max_storage_buffers_per_shader_stage < 1
            || limits.max_storage_buffer_binding_size < 4
            || limits.max_buffer_size < 1344
        {
            return Err("This GPU cannot bind the checked source section resources");
        }
        let gpu = allocation::reserve_bytes(65536 + 16, device, source.queue())?;
        let empty = allocation::texture(
            device,
            &wgpu::TextureDescriptor {
                label: Some("absent source section"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba32Float,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
        )?;
        let memory = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let layout = layout(device);
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("ordered source section layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("source before physical MPR planes"),
            source: wgpu::ShaderSource::Wgsl(
                format!(
                    "{}\n{}\n{}\n{}\n{}\n{}\n{}",
                    include_str!("pixel_ray.wgsl"),
                    crate::VOLUME_SAMPLING_SHADER,
                    crate::material_volume::optics_shader(true, false),
                    include_str!("opaque_interval.wgsl"),
                    include_str!("surface_optics.wgsl"),
                    include_str!("voi.wgsl"),
                    include_str!("section_volume.wgsl")
                )
                .into(),
            ),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("checked physical source sections"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        let display = Arc::new(LinearDisplay::new(device, format));
        let invalid = pollster::block_on(validation.pop());
        let exhausted = pollster::block_on(memory.pop()).is_some();
        if invalid.is_some() || exhausted {
            return Err("The GPU could not prepare source section rendering");
        }
        Ok(Self {
            source,
            pipeline,
            layout,
            display,
            empty,
            pending: Default::default(),
            _gpu: gpu,
        })
    }
    /// Submit complete, contiguous row batches on a worker. Only the final checked
    /// batch exposes an image; callers may retain older complete images throughout.
    pub fn render_rows(
        &mut self,
        scene: &SectionScene,
        slot: usize,
        rows: Range<u32>,
        cancelled: impl Fn() -> bool,
    ) -> Result<Option<Arc<SectionImage>>, &'static str> {
        if slot >= self.pending.len() {
            return Err("Unsupported source section slot");
        }
        if cancelled() {
            self.cancel(slot)?;
            return Err("Cancelled");
        }
        if rows.start >= rows.end || rows.end > scene.size[1] {
            return Err("Source section rows exceed the image");
        }
        let region = [[0, rows.start], [scene.size[0], rows.end - rows.start]];
        let parameters = crate::material_volume::parameters(
            &self.source,
            &scene.navigation,
            &scene.volume,
            &scene.material,
            scene.lighting,
            scene.size,
            region,
        )?;
        let planes = plane_parameters(&self.source, scene)?;
        let device = self.source.device();
        if [
            scene.size[0].div_ceil(8),
            (rows.end - rows.start).div_ceil(8),
        ]
        .into_iter()
        .any(|v| v > device.limits().max_compute_workgroups_per_dimension)
        {
            return Err("Source section batch exceeds the GPU dispatch limit");
        }
        if rows.start == 0 {
            if self.pending[slot].is_some() {
                return Err("Finish or cancel the pending source section first");
            }
            self.pending[slot] = Some(self.allocate(scene)?);
        }
        let pending = self.pending[slot]
            .as_mut()
            .ok_or("Start source sections at the first row")?;
        if !pending.image.scene.same(scene) || pending.next_row != rows.start {
            return Err("Source section scene or rows no longer match");
        }
        if pending.dispatches >= MAX_ROW_DISPATCHES {
            return Err("Source section has too many row batches");
        }
        let uniform = allocation::initialized_buffer(
            device,
            "immutable source section optics",
            bytemuck::cast_slice(&parameters),
            wgpu::BufferUsages::UNIFORM,
        )?;
        let section = allocation::initialized_buffer(
            device,
            "immutable physical section coordinates",
            bytemuck::cast_slice(&planes),
            wgpu::BufferUsages::UNIFORM,
        )?;
        let bindings = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("source section row bindings"),
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
                    resource: wgpu::BindingResource::TextureView(&pending.planes[0]),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::TextureView(&pending.planes[1]),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: wgpu::BindingResource::TextureView(&pending.planes[2]),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: wgpu::BindingResource::TextureView(&self.source.voi),
                },
                wgpu::BindGroupEntry {
                    binding: 8,
                    resource: pending.status.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 9,
                    resource: section.as_entire_binding(),
                },
            ],
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("ordered source section rows"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bindings, &[]);
            pass.dispatch_workgroups(
                scene.size[0].div_ceil(8),
                (rows.end - rows.start).div_ceil(8),
                1,
            );
        }
        self.source.queue().submit([encoder.finish()]);
        pending.next_row = rows.end;
        pending.dispatches += 1;
        if rows.end == scene.size[1] {
            let pending = self.pending[slot]
                .take()
                .ok_or("Source section output unavailable")?;
            check(&self.source, &pending.status, &cancelled)?;
            return Ok(Some(Arc::new(pending.image)));
        }
        Ok(None)
    }
    pub fn cancel(&mut self, slot: usize) -> Result<(), &'static str> {
        self.pending
            .get_mut(slot)
            .ok_or("Unsupported source section slot")?
            .take();
        Ok(())
    }
    fn allocate(&self, scene: &SectionScene) -> Result<Pending, &'static str> {
        let device = self.source.device();
        let gpu = allocation::reserve_bytes(
            u64::from(scene.size[0]) * u64::from(scene.size[1]) * 8 + 512 * 1024 + 4,
            device,
            self.source.queue(),
        )?;
        let metadata = MemoryBudget::volumes()
            .reserve((std::mem::size_of::<Pending>() + 2 * std::mem::size_of::<usize>()) as u64)
            .ok_or("Not enough memory for the source section image identity")?;
        let texture = allocation::texture(
            device,
            &wgpu::TextureDescriptor {
                label: Some("checked source section radiance"),
                size: wgpu::Extent3d {
                    width: scene.size[0],
                    height: scene.size[1],
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
            "source section arithmetic flag",
            &[0; 4],
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        )?;
        let planes = std::array::from_fn(|i| {
            scene.planes[i]
                .as_ref()
                .map_or(&self.empty, |p| p.image.texture())
                .create_view(&Default::default())
        });
        Ok(Pending {
            image: SectionImage {
                source: self.source.clone(),
                scene: scene.clone(),
                texture,
                bindings,
                display: self.display.clone(),
                _gpu: gpu,
                _metadata: metadata,
            },
            target,
            planes,
            status,
            next_row: 0,
            dispatches: 0,
        })
    }
}

fn plane_parameters(
    source: &GpuVolumeSource,
    scene: &SectionScene,
) -> Result<[f32; 72], &'static str> {
    let camera = scene.volume.camera.frame(
        &scene.navigation,
        f64::from(scene.size[0]) / f64::from(scene.size[1]),
    )?;
    let mut result = [0.; 72];
    for (slot, plane) in scene.planes.iter().enumerate() {
        let Some(plane) = plane else {
            continue;
        };
        // A native Device ID is not a source identity.
        if !std::ptr::eq(source, plane.image.source().as_ref())
            || !plane.image.grid().belongs_to(source.volume())
        {
            return Err("Source section requires its exact completed source owner");
        }
        let grid = plane.image.grid();
        let frame = plane.image.frame();
        if grid.slab().is_some() || frame.slab.is_some() {
            return Err("Only thin completed planes can be source sections");
        }
        if grid.dimensions() != [frame.width, frame.height]
            || !frame.valid_dimensions()
            || frame.rgba.is_some()
        {
            return Err("Source section and completed frame differ");
        }
        let geometry = grid.geometry();
        geometry
            .validate()
            .map_err(|_| "Invalid source section geometry")?;
        let (u, v) = geometry.directions();
        let u = u.map(|x| x * geometry.spacing[1]);
        let v = v.map(|x| x * geometry.spacing[0]);
        let dot = |a: [f64; 3], b: [f64; 3]| a.into_iter().zip(b).map(|(a, b)| a * b).sum::<f64>();
        let uu = dot(u, u);
        let vv = dot(v, v);
        let uv = dot(u, v);
        let determinant = uu * vv - uv * uv;
        if !determinant.is_finite() || determinant <= 0. {
            return Err("Source section basis cannot be inverted");
        }
        let dual_u = std::array::from_fn(|a| (u[a] * vv - v[a] * uv) / determinant);
        let dual_v = std::array::from_fn(|a| (v[a] * uu - u[a] * uv) / determinant);
        let n = geometry.normal();
        let length = dot(n, n).sqrt();
        let normal = n.map(|v| v / length);
        let delta = std::array::from_fn(|a| camera.origin_mm[a] - geometry.position[a]);
        let output = &mut result[slot * 24..(slot + 1) * 24];
        // Project relative f64 rays before conversion. Large LPS origins and
        // tiny spacings never subtract as f32 world positions in the shader.
        for (i, vector) in [
            delta,
            camera.right_mm,
            camera.down_mm,
            camera.direction_unit,
        ]
        .into_iter()
        .enumerate()
        {
            for (a, basis) in [dual_u, dual_v, normal].into_iter().enumerate() {
                let value = dot(vector, basis);
                let narrowed = value as f32;
                if value != 0. && (narrowed == 0. || narrowed.is_subnormal()) {
                    return Err("Source section coordinates are below GPU arithmetic limits");
                }
                output[i * 4 + a] = narrowed;
            }
        }
        output[3] = frame.width as f32;
        output[7] = frame.height as f32;
        output[11] = 1.;
        if !plane.window.is_valid() {
            return Err("Invalid source section display window");
        }
        output[16..20].copy_from_slice(&[
            plane.window.width,
            plane.window.center,
            u8::from(plane.invert ^ frame.monochrome1) as f32,
            0.,
        ]);
        output[20..24].copy_from_slice(&crate::voi::parameters(frame, plane.window)?);
    }
    if result.iter().any(|v| !v.is_finite() || v.abs() > 1e20) {
        return Err("Source section exceeds GPU coordinate limits");
    }
    Ok(result)
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
    let uniform = |binding, bytes| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: wgpu::BufferSize::new(bytes),
        },
        count: None,
    };
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("source section resources"),
        entries: &[
            texture(
                0,
                wgpu::TextureSampleType::Float { filterable: false },
                wgpu::TextureViewDimension::D3,
            ),
            uniform(1, 1344),
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
                wgpu::TextureSampleType::Float { filterable: false },
                wgpu::TextureViewDimension::D2,
            ),
            texture(
                5,
                wgpu::TextureSampleType::Float { filterable: false },
                wgpu::TextureViewDimension::D2,
            ),
            texture(
                6,
                wgpu::TextureSampleType::Float { filterable: false },
                wgpu::TextureViewDimension::D2,
            ),
            crate::voi::layout(7, wgpu::ShaderStages::COMPUTE),
            wgpu::BindGroupLayoutEntry {
                binding: 8,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: false },
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(4),
                },
                count: None,
            },
            uniform(9, 288),
        ],
    })
}
fn check(
    source: &GpuVolumeSource,
    status: &wgpu::Buffer,
    cancelled: &impl Fn() -> bool,
) -> Result<(), &'static str> {
    let device = source.device();
    let queue = source.queue();
    let _gpu = allocation::reserve_bytes(4, device, queue)?;
    let _cpu = MemoryBudget::volumes()
        .reserve(4)
        .ok_or("Not enough memory for the source section status")?;
    let buffer = allocation::buffer(
        device,
        &wgpu::BufferDescriptor {
            label: Some("source section status readback"),
            size: 4,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        },
    )?;
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_buffer_to_buffer(status, 0, &buffer, 0, 4);
    queue.submit([encoder.finish()]);
    let (tx, rx) = mpsc::sync_channel(1);
    buffer.map_async(wgpu::MapMode::Read, .., move |r| {
        let _ = tx.send(r);
    });
    flush_upload(device, queue, cancelled)?;
    rx.try_recv()
        .map_err(|_| "Source section status unavailable")?
        .map_err(|_| "Source section status readback failed")?;
    let data = buffer
        .get_mapped_range(..)
        .map_err(|_| "Source section status mapping failed")?;
    let word: [u8; 4] = data
        .get(..4)
        .ok_or("Invalid source section status length")?
        .try_into()
        .map_err(|_| "Invalid source section status length")?;
    let failed = u32::from_le_bytes(word) != 0;
    drop(data);
    buffer.unmap();
    if failed {
        return Err(
            "Source section arithmetic could not resolve this view; no image was published",
        );
    }
    if cancelled() {
        return Err("Cancelled");
    }
    Ok(())
}

#[cfg(all(test, feature = "gpu-tests"))]
mod tests;
