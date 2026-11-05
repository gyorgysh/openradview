// SPDX-License-Identifier: GPL-3.0-or-later
//! Immutable, source-bound GPU pages. Nonresident data is separate from missing samples.
use crate::volume::{allocation, flush_upload};
use orv_core::{
    bricks::{Brick, BrickLayout, BrickStencil},
    resources::{MemoryBudget, Reservation},
    volume::Volume,
};
use std::sync::Arc;
mod page;
pub use page::GpuBrickPage;
#[cfg(all(test, feature = "gpu-tests"))]
mod tests;

pub const MAX_RESIDENT_BRICK_BYTES: u64 = 256 * 1024 * 1024;
pub const BRICK_SAMPLING_SHADER: &str = include_str!("brick_source/sampling.wgsl");
#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrickSampleStatus {
    Known = 1,
    Missing = 2,
    NotOwned = 3,
    OutsideSource = 4,
    StencilUnavailable = 5,
}

pub struct GpuBrickSource {
    source: Arc<Volume>,
    layout: BrickLayout,
    bindings: wgpu::BindGroupLayout,
    device: wgpu::Device,
    queue: wgpu::Queue,
    _metadata: Reservation,
    _gpu: allocation::GpuReservation,
}
impl GpuBrickSource {
    /// Worker-only. This does not allocate a dense GPU copy or read scalar planes.
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        source: Arc<Volume>,
        cell_side: usize,
        stencil: BrickStencil,
        cancelled: impl Fn() -> bool,
    ) -> Result<Arc<Self>, &'static str> {
        if cancelled() {
            return Err("Cancelled");
        }
        if source.is_color() {
            return Err("Source bricks require scalar images");
        }
        let layout = BrickLayout::with_stencil(source.dimensions(), cell_side, stencil)
            .map_err(|_| "Unsupported source brick layout")?;
        let limits = device.limits();
        if limits.max_bind_groups < 1
            || limits.max_bindings_per_bind_group < 3
            || limits.max_sampled_textures_per_shader_stage < 2
            || limits.max_uniform_buffers_per_shader_stage < 1
            || limits.max_uniform_buffer_binding_size < 80
            || limits.max_buffer_size < 80
        {
            return Err("This GPU cannot bind source brick inputs");
        }
        for i in 0..layout.len() {
            if i.is_multiple_of(64) && cancelled() {
                return Err("Cancelled");
            }
            let b = layout.brick(i).ok_or("Invalid source brick layout")?;
            if b.size
                .into_iter()
                .any(|v| v > limits.max_texture_dimension_3d as usize)
            {
                return Err("The source brick stencil exceeds this GPU's texture side");
            }
        }
        let metadata = MemoryBudget::volumes()
            .reserve((std::mem::size_of::<Self>() + 2 * std::mem::size_of::<usize>()) as u64)
            .ok_or("Not enough memory for the source brick owner")?;
        let gpu = allocation::reserve_bytes(65536, device, queue)?;
        let memory = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let bindings = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("immutable source brick values validity and bounds"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D3,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Uint,
                        view_dimension: wgpu::TextureViewDimension::D3,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(80),
                    },
                    count: None,
                },
            ],
        });
        let invalid = pollster::block_on(validation.pop()).is_some();
        let exhausted = pollster::block_on(memory.pop()).is_some();
        if invalid || exhausted {
            return Err("The GPU could not prepare source brick bindings");
        }
        if cancelled() {
            return Err("Cancelled");
        }
        Ok(Arc::new(Self {
            source,
            layout,
            bindings,
            device: device.clone(),
            queue: queue.clone(),
            _metadata: metadata,
            _gpu: gpu,
        }))
    }
    pub fn source(&self) -> &Arc<Volume> {
        &self.source
    }
    pub fn layout(&self) -> &BrickLayout {
        &self.layout
    }
    pub fn binding_layout(&self) -> &wgpu::BindGroupLayout {
        &self.bindings
    }
    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }
    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }
    pub fn wait_submitted(&self, cancelled: impl Fn() -> bool) -> Result<(), &'static str> {
        flush_upload(&self.device, &self.queue, &cancelled)
    }
    fn resident_bytes(&self, index: usize) -> Result<u64, &'static str> {
        let b = self
            .layout
            .brick(index)
            .ok_or("Source brick index is outside the layout")?;
        let n = b.size.into_iter().product::<usize>() as u64;
        Ok(n * if self.source.has_missing_scalar() {
            5
        } else {
            4
        } + 1
            + 80
            + 65536)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BrickUploadStatistics {
    pub reused_pages: usize,
    pub uploaded_pages: usize,
    /// Original scalar/validity bytes only, excluding padding and uniforms.
    pub uploaded_bytes: u64,
    /// Complete generation's referenced page admission, including shared pages.
    pub referenced_bytes: u64,
}
pub enum BrickLookup<'a> {
    OutsideSource,
    Nonresident {
        index: usize,
    },
    Resident {
        index: usize,
        page: &'a Arc<GpuBrickPage>,
    },
}
pub struct GpuBrickResidency {
    source: Arc<GpuBrickSource>,
    pages: Vec<Option<Arc<GpuBrickPage>>>,
    statistics: BrickUploadStatistics,
    _metadata: Reservation,
}
impl GpuBrickResidency {
    /// Worker-only immutable generation. Requested pages alone are resident;
    /// absence never means transparent/known source data. The renderer must prove
    /// completeness separately before publishing an image.
    pub fn prepare(
        source: Arc<GpuBrickSource>,
        requested: &[usize],
        previous: Option<&Arc<Self>>,
        limit: u64,
        cancelled: impl Fn() -> bool,
    ) -> Result<Arc<Self>, &'static str> {
        if cancelled() {
            return Err("Cancelled");
        }
        if previous.is_some_and(|p| !Arc::ptr_eq(&p.source, &source)) {
            return Err("Previous source brick residency belongs to another owner or device");
        }
        if limit > MAX_RESIDENT_BRICK_BYTES || requested.len() > source.layout.len() {
            return Err("Source brick residency exceeds its bound");
        }
        let bytes = (source.layout.len() * std::mem::size_of::<Option<Arc<GpuBrickPage>>>()
            + std::mem::size_of_val(requested)
            + std::mem::size_of::<Self>()
            + 2 * std::mem::size_of::<usize>()) as u64;
        let metadata = MemoryBudget::volumes()
            .reserve(bytes)
            .ok_or("Not enough memory for source brick residency")?;
        let mut indices = Vec::new();
        indices
            .try_reserve_exact(requested.len())
            .map_err(|_| "Not enough memory for requested source bricks")?;
        indices.extend_from_slice(requested);
        indices.sort_unstable();
        if indices.windows(2).any(|w| w[0] == w[1]) {
            return Err("Requested source brick indices must be distinct");
        }
        let mut statistics = BrickUploadStatistics::default();
        for &i in &indices {
            if cancelled() {
                return Err("Cancelled");
            }
            statistics.referenced_bytes = statistics
                .referenced_bytes
                .checked_add(source.resident_bytes(i)?)
                .filter(|n| *n <= limit)
                .ok_or("Requested source bricks exceed the resident allowance")?;
        }
        let mut pages = Vec::new();
        pages
            .try_reserve_exact(source.layout.len())
            .map_err(|_| "Not enough memory for source brick residency")?;
        pages.resize_with(source.layout.len(), || None);
        for index in indices {
            if cancelled() {
                return Err("Cancelled");
            }
            if let Some(page) = previous.and_then(|p| p.pages[index].as_ref()) {
                pages[index] = Some(page.clone());
                statistics.reused_pages += 1;
            } else {
                let page = GpuBrickPage::upload(source.clone(), index, &cancelled)?;
                statistics.uploaded_bytes += page.uploaded_bytes();
                statistics.uploaded_pages += 1;
                pages[index] = Some(page);
            }
        }
        if cancelled() {
            return Err("Cancelled");
        }
        Ok(Arc::new(Self {
            source,
            pages,
            statistics,
            _metadata: metadata,
        }))
    }
    pub fn source(&self) -> &Arc<GpuBrickSource> {
        &self.source
    }
    pub fn page(&self, index: usize) -> Option<&Arc<GpuBrickPage>> {
        self.pages.get(index)?.as_ref()
    }
    pub fn statistics(&self) -> BrickUploadStatistics {
        self.statistics
    }
    pub fn lookup(&self, voxel: [f64; 3]) -> BrickLookup<'_> {
        let Some(index) = self.source.layout.owner(voxel) else {
            return BrickLookup::OutsideSource;
        };
        self.page(index)
            .map_or(BrickLookup::Nonresident { index }, |page| {
                BrickLookup::Resident { index, page }
            })
    }
}
