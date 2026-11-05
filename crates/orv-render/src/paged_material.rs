// SPDX-License-Identifier: GPL-3.0-or-later
//! Complete worker-rendered optical images using bounded GPU source residency.
use crate::{
    brick_source::{ATLAS_SAMPLING_SHADER, GpuBrickAtlasPool, GpuBrickAtlasStream, GpuBrickSource},
    linear_display::LinearDisplay,
    material_volume::Lighting,
    paged_material_ranges::GpuPagedMaterialRanges,
    volume::{allocation, flush_upload},
};
use orv_core::{
    ViewState,
    bricks::BrickStencil,
    resources::{MemoryBudget, Reservation},
    volume_material::VolumeMaterial,
    volume_view::VolumeView,
};
use std::{
    ops::Range,
    sync::{Arc, mpsc},
};
pub const MAX_BATCH_ROWS: u32 = 16;
const MAX_WAVES_PER_BATCH: usize = 4096;
const MAX_IMAGE_UPLOAD_BYTES: u64 = 16 * 1024 * 1024 * 1024;
#[derive(Clone)]
pub struct PagedMaterialScene {
    pub source: Arc<GpuBrickSource>,
    pub navigation: ViewState,
    pub volume: VolumeView,
    pub material: Arc<VolumeMaterial>,
    pub lighting: Lighting,
    pub size: [u32; 2],
}
impl PagedMaterialScene {
    fn same(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.source, &other.source)
            && self.navigation == other.navigation
            && self.volume == other.volume
            && Arc::ptr_eq(&self.material, &other.material)
            && self.lighting == other.lighting
            && self.size == other.size
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PagedRenderStatistics {
    pub tile_batches: usize,
    pub fenced_waves: usize,
    pub source_pages_uploaded: usize,
    pub source_bytes_uploaded: u64,
    pub atlas_pages_copied: usize,
    pub atlas_bytes_copied: u64,
}
pub struct PagedMaterialImage {
    scene: PagedMaterialScene,
    ranges: Option<Arc<GpuPagedMaterialRanges>>,
    texture: wgpu::Texture,
    bindings: wgpu::BindGroup,
    display: Arc<LinearDisplay>,
    statistics: PagedRenderStatistics,
    _metadata: Reservation,
    _gpu: allocation::GpuReservation,
}
impl PagedMaterialImage {
    pub fn scene(&self) -> &PagedMaterialScene {
        &self.scene
    }
    pub fn texture(&self) -> &wgpu::Texture {
        &self.texture
    }
    pub fn statistics(&self) -> PagedRenderStatistics {
        self.statistics
    }
    pub fn ranges(&self) -> Option<&Arc<GpuPagedMaterialRanges>> {
        self.ranges.as_ref()
    }
    pub fn paint(&self, pass: &mut wgpu::RenderPass<'_>) {
        self.display.paint(&self.bindings, pass);
    }
}
struct Pending {
    image: PagedMaterialImage,
    target: wgpu::TextureView,
    states: wgpu::Buffer,
    progress: wgpu::Buffer,
    readback: wgpu::Buffer,
    next_row: u32,
    _scratch: allocation::GpuReservation,
    _cpu: Reservation,
}
struct WaveStatus {
    pending: u32,
    advanced: u32,
    required: Vec<usize>,
}
pub struct PagedMaterialRenderer {
    pool: Arc<GpuBrickAtlasPool>,
    ranges: Option<Arc<GpuPagedMaterialRanges>>,
    atlas: GpuBrickAtlasStream,
    pipeline: wgpu::ComputePipeline,
    resident_pipeline: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
    display: Arc<LinearDisplay>,
    pending: [Option<Pending>; 2],
    _metadata: Reservation,
    _gpu: allocation::GpuReservation,
}
impl PagedMaterialRenderer {
    pub fn new(
        pool: Arc<GpuBrickAtlasPool>,
        format: wgpu::TextureFormat,
        cancelled: impl Fn() -> bool,
    ) -> Result<Self, &'static str> {
        Self::prepare(pool, None, format, cancelled)
    }
    /// Worker-only. The exact source and material remain bound to this proof.
    pub fn new_accelerated(
        pool: Arc<GpuBrickAtlasPool>,
        ranges: Arc<GpuPagedMaterialRanges>,
        format: wgpu::TextureFormat,
        cancelled: impl Fn() -> bool,
    ) -> Result<Self, &'static str> {
        Self::prepare(pool, Some(ranges), format, cancelled)
    }
    fn prepare(
        pool: Arc<GpuBrickAtlasPool>,
        ranges: Option<Arc<GpuPagedMaterialRanges>>,
        format: wgpu::TextureFormat,
        cancelled: impl Fn() -> bool,
    ) -> Result<Self, &'static str> {
        if cancelled() {
            return Err("Cancelled");
        }
        let source = pool.source();
        if ranges
            .as_ref()
            .is_some_and(|r| !Arc::ptr_eq(r.source(), source))
        {
            return Err("Paged classification requires the exact GPU source");
        }
        let use_ranges = ranges.as_ref().filter(|r| r.transparent_tiles() != 0);
        let device = source.device();
        let queue = source.queue();
        let limits = device.limits();
        crate::volume::check_limits(&limits)?;
        if limits.max_bind_groups < 2
            || limits.max_bindings_per_bind_group < 4
            || limits.max_uniform_buffers_per_shader_stage < 2
            || limits.max_uniform_buffer_binding_size < 1344
            || limits.max_storage_buffers_per_shader_stage < 3
            || limits.max_storage_buffer_binding_size < 2048 * MAX_BATCH_ROWS as u64 * 16
            || limits.max_buffer_size < 2048 * MAX_BATCH_ROWS as u64 * 16
            || limits.max_compute_workgroups_per_dimension < 256
        {
            return Err("This GPU cannot bind bounded paged material rendering");
        }
        if use_ranges.is_some()
            && (limits.max_bind_groups < 3
                || limits.max_sampled_textures_per_shader_stage < 3
                || limits.max_uniform_buffers_per_shader_stage < 3)
        {
            return Err("This GPU cannot bind paged material classification");
        }
        let metadata = MemoryBudget::volumes()
            .reserve(
                (std::mem::size_of::<Self>() + pool.capacity() * std::mem::size_of::<usize>() + 16)
                    as u64,
            )
            .ok_or("Not enough memory for the paged renderer")?;
        let gpu = allocation::reserve_bytes(65536, device, queue)?;
        let atlas = GpuBrickAtlasStream::new(pool.clone(), &cancelled)?;
        let memory = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let buffer = |binding, ty, minimum| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty,
                has_dynamic_offset: false,
                min_binding_size: wgpu::BufferSize::new(minimum),
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("paged optical rows and continuation state"),
            entries: &[
                buffer(0, wgpu::BufferBindingType::Uniform, 1344),
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: wgpu::TextureFormat::Rgba16Float,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
                buffer(2, wgpu::BufferBindingType::Storage { read_only: false }, 16),
                buffer(3, wgpu::BufferBindingType::Storage { read_only: false }, 12),
            ],
        });
        let text = format!(
            "{ATLAS_SAMPLING_SHADER}\n{}\n{}\n{}\n{}\n{}",
            include_str!("volume_interval.wgsl"),
            include_str!("material_parameters.wgsl"),
            include_str!("material_mapping.wgsl"),
            if use_ranges.is_some() {
                concat!(
                    include_str!("paged_material_ranges.wgsl"),
                    "\n",
                    include_str!("material_ranges.wgsl")
                )
            } else {
                "fn transparent_advance(lower:vec3u,point:vec3f,origin:vec3f,near:f32,step:f32,index:u32,count:u32)->u32{return 0u;}"
            },
            include_str!("paged_material.wgsl")
        );
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("continuous whole-ray optical integration across source pages"),
            source: wgpu::ShaderSource::Wgsl(text.into()),
        });
        let layouts = [
            Some(pool.binding_layout()),
            Some(&layout),
            use_ranges.map(|r| r.layout()),
        ];
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &layouts[..if use_ranges.is_some() { 3 } else { 2 }],
            immediate_size: 0,
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: None,
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        let resident_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("fully resident paged optical integration"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("resident"),
            compilation_options: Default::default(),
            cache: None,
        });
        let display = Arc::new(LinearDisplay::new(device, format));
        let invalid = pollster::block_on(validation.pop()).is_some();
        let exhausted = pollster::block_on(memory.pop()).is_some();
        if invalid || exhausted {
            return Err("The GPU could not prepare paged optical rendering");
        }
        if cancelled() {
            return Err("Cancelled");
        }
        Ok(Self {
            pool,
            ranges,
            atlas,
            pipeline,
            resident_pipeline,
            layout,
            display,
            pending: Default::default(),
            _metadata: metadata,
            _gpu: gpu,
        })
    }
    /// Worker-only, strict consecutive batches of at most sixteen rows. A pending
    /// target is private; only the final fully checked image can be presented.
    pub fn render_rows(
        &mut self,
        scene: &PagedMaterialScene,
        slot: usize,
        rows: Range<u32>,
        cancelled: impl Fn() -> bool,
    ) -> Result<Option<Arc<PagedMaterialImage>>, &'static str> {
        if slot >= self.pending.len() {
            return Err("Unsupported paged material slot");
        }
        if cancelled() {
            self.cancel(slot)?;
            return Err("Cancelled");
        }
        if !Arc::ptr_eq(&scene.source, self.pool.source()) {
            return Err("Paged material source no longer matches");
        }
        self.check_material(scene)?;
        if rows.start >= rows.end
            || rows.end > scene.size[1]
            || rows.end - rows.start > MAX_BATCH_ROWS
        {
            return Err("Paged material rows exceed supported bounds");
        }
        if scene.lighting.diffuse > 0.
            && scene.source.layout().stencil() != BrickStencil::CentralSecants
        {
            return Err("Paged lighting requires declared source secants");
        }
        let parameters = crate::material_volume::parameters_for_volume(
            scene.source.source(),
            &scene.navigation,
            &scene.volume,
            &scene.material,
            scene.lighting,
            scene.size,
            [[0, rows.start], [scene.size[0], rows.end - rows.start]],
        )?;
        let mut pending = if rows.start == 0 {
            self.pending[slot].take();
            self.allocate(scene)?
        } else {
            self.pending[slot]
                .take()
                .ok_or("Paged rows need a current pending image")?
        };
        if !pending.image.scene.same(scene) || pending.next_row != rows.start {
            return Err("Paged material view or row order changed");
        }
        self.batch(&mut pending, &parameters, &cancelled)?;
        pending.next_row = rows.end;
        pending.image.statistics.tile_batches += 1;
        if cancelled() {
            return Err("Cancelled");
        }
        if rows.end == scene.size[1] {
            Ok(Some(Arc::new(pending.image)))
        } else {
            self.pending[slot] = Some(pending);
            Ok(None)
        }
    }
    /// Worker-only complete image. A fully resident snapshot uses one dispatch
    /// and status fence; constrained residency uses bounded row batches.
    pub fn render_complete(
        &mut self,
        scene: &PagedMaterialScene,
        slot: usize,
        cancelled: impl Fn() -> bool,
    ) -> Result<Arc<PagedMaterialImage>, &'static str> {
        if slot >= self.pending.len() {
            return Err("Unsupported paged material slot");
        }
        if cancelled() {
            self.cancel(slot)?;
            return Err("Cancelled");
        }
        if !Arc::ptr_eq(&scene.source, self.pool.source()) {
            return Err("Paged material source no longer matches");
        }
        self.check_material(scene)?;
        if scene.lighting.diffuse > 0.
            && scene.source.layout().stencil() != BrickStencil::CentralSecants
        {
            return Err("Paged lighting requires declared source secants");
        }
        let parameters = crate::material_volume::parameters_for_volume(
            scene.source.source(),
            &scene.navigation,
            &scene.volume,
            &scene.material,
            scene.lighting,
            scene.size,
            [[0, 0], scene.size],
        )?;
        self.pending[slot].take();
        let mut pending = self.allocate(scene)?;
        let resident = self.ranges.as_ref().map_or_else(
            || self.atlas.resident_indices().count() == self.pool.source().layout().len(),
            |r| r.required_indices().all(|i| self.atlas.contains(i)),
        );
        if !resident {
            for y in (0..scene.size[1]).step_by(MAX_BATCH_ROWS as usize) {
                let mut tile = parameters;
                tile[28..32].copy_from_slice(&[
                    0.,
                    y as f32,
                    scene.size[0] as f32,
                    (scene.size[1] - y).min(MAX_BATCH_ROWS) as f32,
                ]);
                self.batch(&mut pending, &tile, &cancelled)?;
                pending.image.statistics.tile_batches += 1;
            }
            if cancelled() {
                return Err("Cancelled");
            }
            return Ok(Arc::new(pending.image));
        }
        let source = self.pool.source();
        let device = source.device();
        let queue = source.queue();
        let uniform = allocation::initialized_buffer(
            device,
            "immutable resident material parameters",
            bytemuck::cast_slice(&parameters),
            wgpu::BufferUsages::UNIFORM,
        )?;
        let bindings = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("fully resident optical dispatch"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&pending.target),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: pending.states.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: pending.progress.as_entire_binding(),
                },
            ],
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("fully resident complete material image"),
        });
        encoder.clear_buffer(&pending.progress, 0, None);
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("fully resident optical image"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.resident_pipeline);
            pass.set_bind_group(0, self.atlas.bindings(), &[]);
            pass.set_bind_group(1, &bindings, &[]);
            if let Some(ranges) = self.ranges.as_ref().filter(|r| r.transparent_tiles() != 0) {
                pass.set_bind_group(2, ranges.bindings(), &[]);
            }
            pass.dispatch_workgroups(scene.size[0].div_ceil(8), scene.size[1].div_ceil(8), 1);
        }
        encoder.copy_buffer_to_buffer(
            &pending.progress,
            0,
            &pending.readback,
            0,
            self.status_bytes(),
        );
        queue.submit([encoder.finish()]);
        let status = self.read_status(&pending, &cancelled)?;
        if status.pending != 0 || !status.required.is_empty() {
            return Err("Resident optical image unexpectedly needs source pages");
        }
        if cancelled() {
            return Err("Cancelled");
        }
        pending.image.statistics.tile_batches = 1;
        pending.image.statistics.fenced_waves = 1;
        Ok(Arc::new(pending.image))
    }
    fn allocate(&self, scene: &PagedMaterialScene) -> Result<Pending, &'static str> {
        let source = self.pool.source();
        let device = source.device();
        let queue = source.queue();
        let size = scene.size;
        if size
            .into_iter()
            .any(|v| v > device.limits().max_texture_dimension_2d)
        {
            return Err("Paged material output exceeds this GPU's texture side");
        }
        let gpu =
            allocation::reserve_bytes(u64::from(size[0]) * u64::from(size[1]) * 8, device, queue)?;
        let metadata = MemoryBudget::volumes()
            .reserve((std::mem::size_of::<PagedMaterialImage>() + 16) as u64)
            .ok_or("Not enough memory for the paged material image")?;
        let status_bytes = self.status_bytes();
        let state_bytes = u64::from(size[0]) * u64::from(MAX_BATCH_ROWS) * 16;
        let scratch = allocation::reserve_bytes(
            state_bytes + 2 * status_bytes + 1344 + 65536,
            device,
            queue,
        )?;
        let cpu = MemoryBudget::volumes()
            .reserve(
                status_bytes
                    + (self.pool.capacity() * std::mem::size_of::<usize>()) as u64
                    + std::mem::size_of::<Pending>() as u64,
            )
            .ok_or("Not enough memory for paged row status")?;
        let texture = allocation::texture(
            device,
            &wgpu::TextureDescriptor {
                label: Some("complete paged optical radiance"),
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
        let buffer = |label, size, usage| {
            allocation::buffer(
                device,
                &wgpu::BufferDescriptor {
                    label: Some(label),
                    size,
                    usage,
                    mapped_at_creation: false,
                },
            )
        };
        let states = buffer(
            "bounded float32 ray continuation",
            state_bytes,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        )?;
        let progress = buffer(
            "paged row completion and requests",
            status_bytes,
            wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
        )?;
        let readback = buffer(
            "bounded paged status readback",
            status_bytes,
            wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        )?;
        Ok(Pending {
            image: PagedMaterialImage {
                scene: scene.clone(),
                ranges: self.ranges.clone(),
                texture,
                bindings,
                display: self.display.clone(),
                statistics: Default::default(),
                _metadata: metadata,
                _gpu: gpu,
            },
            target,
            states,
            progress,
            readback,
            next_row: 0,
            _scratch: scratch,
            _cpu: cpu,
        })
    }
    fn status_bytes(&self) -> u64 {
        ((self.pool.source().layout().len().div_ceil(32) + 3) * 4) as u64
    }
    fn check_material(&self, scene: &PagedMaterialScene) -> Result<(), &'static str> {
        if self
            .ranges
            .as_ref()
            .is_some_and(|r| !Arc::ptr_eq(r.material(), &scene.material))
        {
            return Err("Paged classification requires its exact material");
        }
        Ok(())
    }
    fn batch(
        &mut self,
        pending: &mut Pending,
        parameters: &[f32; 336],
        cancelled: &impl Fn() -> bool,
    ) -> Result<(), &'static str> {
        let source = self.pool.source().clone();
        let device = source.device();
        let queue = source.queue();
        let uniform = allocation::initialized_buffer(
            device,
            "immutable paged material row parameters",
            bytemuck::cast_slice(parameters),
            wgpu::BufferUsages::UNIFORM,
        )?;
        let bindings = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("paged optical row dispatch"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&pending.target),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: pending.states.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: pending.progress.as_entire_binding(),
                },
            ],
        });
        let mut stalled = false;
        for wave in 0..MAX_WAVES_PER_BATCH {
            if cancelled() {
                return Err("Cancelled");
            }
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("paged optical continuation wave"),
            });
            if wave == 0 {
                encoder.clear_buffer(&pending.states, 0, None);
            }
            encoder.clear_buffer(&pending.progress, 0, None);
            {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("continuous paged optical rows"),
                    timestamp_writes: None,
                });
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, self.atlas.bindings(), &[]);
                pass.set_bind_group(1, &bindings, &[]);
                if let Some(ranges) = self.ranges.as_ref().filter(|r| r.transparent_tiles() != 0) {
                    pass.set_bind_group(2, ranges.bindings(), &[]);
                }
                pass.dispatch_workgroups(
                    (parameters[30] as u32).div_ceil(8),
                    (parameters[31] as u32).div_ceil(8),
                    1,
                );
            }
            encoder.copy_buffer_to_buffer(
                &pending.progress,
                0,
                &pending.readback,
                0,
                self.status_bytes(),
            );
            queue.submit([encoder.finish()]);
            let status = self.read_status(pending, cancelled)?;
            pending.image.statistics.fenced_waves += 1;
            if status.pending == 0 {
                return Ok(());
            }
            if status.required.is_empty() || (stalled && status.advanced == 0) {
                return Err("Source paging could not advance this view; no image was published");
            }
            stalled = status.advanced == 0;
            let mut wanted = status.required;
            for index in self.atlas.resident_indices() {
                if wanted.len() == self.pool.capacity() {
                    break;
                }
                if !wanted.contains(&index) {
                    wanted.push(index);
                }
            }
            let mut upload_bytes = 0u64;
            for &index in &wanted {
                if !self.atlas.contains(index) {
                    let brick = source
                        .layout()
                        .brick(index)
                        .ok_or("Invalid requested source page")?;
                    upload_bytes += brick.size.into_iter().product::<usize>() as u64
                        * if source.source().has_missing_scalar() {
                            5
                        } else {
                            4
                        };
                }
            }
            if pending.image.statistics.source_bytes_uploaded + upload_bytes
                > MAX_IMAGE_UPLOAD_BYTES
            {
                return Err("Source paging exceeds this image's upload work allowance");
            }
            let copies = self.atlas.load(&wanted, cancelled)?;
            pending.image.statistics.source_pages_uploaded += copies.uploaded_pages;
            pending.image.statistics.source_bytes_uploaded += copies.uploaded_bytes;
            pending.image.statistics.atlas_pages_copied +=
                copies.copied_from_pages + copies.copied_from_prior_atlas;
            pending.image.statistics.atlas_bytes_copied += copies.copied_bytes;
        }
        Err("Source paging exceeds this row batch's work allowance")
    }
    fn read_status(
        &self,
        pending: &Pending,
        cancelled: &impl Fn() -> bool,
    ) -> Result<WaveStatus, &'static str> {
        let (tx, rx) = mpsc::sync_channel(1);
        pending
            .readback
            .map_async(wgpu::MapMode::Read, .., move |r| {
                let _ = tx.send(r);
            });
        if let Err(e) = flush_upload(
            self.pool.source().device(),
            self.pool.source().queue(),
            cancelled,
        ) {
            pending.readback.unmap();
            return Err(e);
        }
        rx.try_recv()
            .map_err(|_| "Paged row status unavailable")?
            .map_err(|_| "Paged row status readback failed")?;
        let data = pending
            .readback
            .get_mapped_range(..)
            .map_err(|_| "Paged row status mapping failed")?;
        let words = data.as_chunks::<4>().0;
        let read = |i: usize| u32::from_le_bytes(words[i]);
        let failed = read(0) != 0;
        let pending_rays = read(1);
        let advanced = read(2);
        let mut required = Vec::new();
        required
            .try_reserve_exact(self.pool.capacity())
            .map_err(|_| "Not enough memory for required source pages")?;
        let mut invalid = false;
        for (word, bytes) in words.iter().enumerate().skip(3) {
            let mut bits = u32::from_le_bytes(*bytes);
            while bits != 0 {
                let bit = bits.trailing_zeros() as usize;
                bits &= bits - 1;
                let index = (word - 3) * 32 + bit;
                if index >= self.pool.source().layout().len() {
                    invalid = true;
                } else if required.len() < self.pool.capacity() {
                    required.push(index);
                }
            }
        }
        drop(data);
        pending.readback.unmap();
        if failed || invalid {
            return Err(
                "Paged optical arithmetic or residency could not resolve this view; no image was published",
            );
        }
        if cancelled() {
            return Err("Cancelled");
        }
        Ok(WaveStatus {
            pending: pending_rays,
            advanced,
            required,
        })
    }
    pub fn cancel(&mut self, slot: usize) -> Result<(), &'static str> {
        self.pending
            .get_mut(slot)
            .ok_or("Unsupported paged material slot")?
            .take();
        Ok(())
    }
}
#[cfg(all(test, feature = "gpu-tests"))]
mod tests;
