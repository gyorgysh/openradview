// SPDX-License-Identifier: GPL-3.0-or-later
//! Immutable atlas snapshots. A new table never retargets an in-flight slot.
use super::*;
mod stream;
mod upload;
pub(crate) use stream::GpuBrickAtlasStream;
pub struct GpuBrickAtlasPool {
    source: Arc<GpuBrickSource>,
    capacity: usize,
    stored: [u32; 3],
    grid: [u32; 3],
    layout: wgpu::BindGroupLayout,
    bytes: u64,
    _metadata: Reservation,
    _gpu: allocation::GpuReservation,
}
impl GpuBrickAtlasPool {
    pub fn new(
        source: Arc<GpuBrickSource>,
        capacity: usize,
        cancelled: impl Fn() -> bool,
    ) -> Result<Arc<Self>, &'static str> {
        if cancelled() {
            return Err("Cancelled");
        }
        if capacity == 0 || capacity > 128 || capacity > source.layout.len() {
            return Err("Source atlas capacity exceeds supported bounds");
        }
        let device = source.device();
        let limits = device.limits();
        let table_bytes = source.layout.len() as u64 * 32;
        if limits.max_bindings_per_bind_group < 4
            || limits.max_storage_buffers_per_shader_stage < 1
            || limits.max_storage_buffer_binding_size < table_bytes
            || limits.max_buffer_size < table_bytes.max(64)
        {
            return Err("This GPU cannot bind the source atlas table");
        }
        let mut stored = [0u32; 3];
        for i in 0..source.layout.len() {
            if i.is_multiple_of(64) && cancelled() {
                return Err("Cancelled");
            }
            let b = source
                .layout
                .brick(i)
                .ok_or("Invalid source atlas layout")?;
            for (a, value) in stored.iter_mut().enumerate() {
                *value = (*value).max(b.size[a] as u32);
            }
        }
        let maximum = stored.map(|s| limits.max_texture_dimension_3d / s);
        let grid = pack(capacity as u32, maximum, stored)
            .ok_or("The source atlas does not fit this GPU's texture side")?;
        let extent: [u32; 3] = std::array::from_fn(|a| stored[a] * grid[a]);
        let bytes = extent.into_iter().map(u64::from).product::<u64>()
            * if source.source.has_missing_scalar() {
                5
            } else {
                4
            }
            + table_bytes
            + 64
            + 1
            + 65536;
        if bytes > MAX_RESIDENT_BRICK_BYTES {
            return Err("The source atlas exceeds its memory allowance");
        }
        let metadata = MemoryBudget::volumes()
            .reserve((std::mem::size_of::<Self>() + 16) as u64)
            .ok_or("Not enough memory for the source atlas owner")?;
        let gpu = allocation::reserve_bytes(65536, device, source.queue())?;
        let memory = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let mut entries = Vec::with_capacity(4);
        for binding in 0..4 {
            entries.push(wgpu::BindGroupLayoutEntry {
                binding,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: match binding {
                    0 | 1 => wgpu::BindingType::Texture {
                        sample_type: if binding == 0 {
                            wgpu::TextureSampleType::Float { filterable: false }
                        } else {
                            wgpu::TextureSampleType::Uint
                        },
                        view_dimension: wgpu::TextureViewDimension::D3,
                        multisampled: false,
                    },
                    _ => wgpu::BindingType::Buffer {
                        ty: if binding == 2 {
                            wgpu::BufferBindingType::Uniform
                        } else {
                            wgpu::BufferBindingType::Storage { read_only: true }
                        },
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(if binding == 2 {
                            64
                        } else {
                            table_bytes
                        }),
                    },
                },
                count: None,
            });
        }
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("immutable scalar atlas and source page table"),
            entries: &entries,
        });
        let invalid = pollster::block_on(validation.pop()).is_some();
        let exhausted = pollster::block_on(memory.pop()).is_some();
        if invalid || exhausted {
            return Err("The GPU could not prepare source atlas bindings");
        }
        if cancelled() {
            return Err("Cancelled");
        }
        Ok(Arc::new(Self {
            source,
            capacity,
            stored,
            grid,
            layout,
            bytes,
            _metadata: metadata,
            _gpu: gpu,
        }))
    }
    pub fn source(&self) -> &Arc<GpuBrickSource> {
        &self.source
    }
    pub fn capacity(&self) -> usize {
        self.capacity
    }
    pub fn binding_layout(&self) -> &wgpu::BindGroupLayout {
        &self.layout
    }
    pub fn texture_size(&self) -> [u32; 3] {
        std::array::from_fn(|a| self.stored[a] * self.grid[a])
    }
    pub fn admission_bytes(&self) -> u64 {
        self.bytes
    }
    fn origin(&self, slot: usize) -> wgpu::Origin3d {
        let slot = slot as u32;
        wgpu::Origin3d {
            x: slot % self.grid[0] * self.stored[0],
            y: slot / self.grid[0] % self.grid[1] * self.stored[1],
            z: slot / (self.grid[0] * self.grid[1]) * self.stored[2],
        }
    }
}
fn pack(capacity: u32, maximum: [u32; 3], stored: [u32; 3]) -> Option<[u32; 3]> {
    let mut best = None;
    let mut score = (u32::MAX, u32::MAX);
    for x in 1..=capacity.min(maximum[0]) {
        for y in 1..=capacity.div_ceil(x).min(maximum[1]) {
            let z = capacity.div_ceil(x * y);
            if z > maximum[2] {
                continue;
            }
            let current = (
                x * y * z,
                (x * stored[0]).max(y * stored[1]).max(z * stored[2]),
            );
            if current < score {
                best = Some([x, y, z]);
                score = current;
            }
        }
    }
    best
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AtlasCopyStatistics {
    pub copied_from_pages: usize,
    pub copied_from_prior_atlas: usize,
    pub copied_bytes: u64,
    pub uploaded_pages: usize,
    pub uploaded_bytes: u64,
}
pub struct GpuBrickAtlas {
    pool: Arc<GpuBrickAtlasPool>,
    slots: Vec<Option<usize>>,
    texture: wgpu::Texture,
    validity: wgpu::Texture,
    directory: wgpu::Buffer,
    bindings: wgpu::BindGroup,
    statistics: AtlasCopyStatistics,
    _metadata: Reservation,
    _gpu: allocation::GpuReservation,
}
impl GpuBrickAtlas {
    /// Worker-only copy and fence. Old snapshots and their slot tables are immutable.
    pub fn prepare(
        pool: Arc<GpuBrickAtlasPool>,
        resident: &Arc<GpuBrickResidency>,
        previous: Option<&Arc<Self>>,
        cancelled: impl Fn() -> bool,
    ) -> Result<Arc<Self>, &'static str> {
        if cancelled() {
            return Err("Cancelled");
        }
        if !Arc::ptr_eq(&pool.source, resident.source())
            || previous.is_some_and(|p| !Arc::ptr_eq(&p.pool, &pool))
        {
            return Err("Source atlas inputs belong to another owner or pool");
        }
        let _indices = MemoryBudget::volumes()
            .reserve((pool.capacity * std::mem::size_of::<usize>()) as u64)
            .ok_or("Not enough memory for requested atlas pages")?;
        let mut requested = Vec::new();
        requested
            .try_reserve_exact(pool.capacity)
            .map_err(|_| "Not enough memory for requested atlas pages")?;
        for i in 0..pool.source.layout.len() {
            if resident.page(i).is_some() {
                if requested.len() == pool.capacity {
                    return Err("Requested source pages exceed atlas capacity");
                }
                requested.push(i);
            }
        }
        Self::snapshot(pool, &requested, Some(resident), previous, cancelled)
    }
    /// Worker-only direct uploads into a new immutable snapshot. One bounded
    /// brick scratch buffer replaces transient per-page GPU textures.
    pub fn load(
        pool: Arc<GpuBrickAtlasPool>,
        requested: &[usize],
        previous: Option<&Arc<Self>>,
        cancelled: impl Fn() -> bool,
    ) -> Result<Arc<Self>, &'static str> {
        Self::snapshot(pool, requested, None, previous, cancelled)
    }
    fn snapshot(
        pool: Arc<GpuBrickAtlasPool>,
        requested: &[usize],
        pages: Option<&Arc<GpuBrickResidency>>,
        previous: Option<&Arc<Self>>,
        cancelled: impl Fn() -> bool,
    ) -> Result<Arc<Self>, &'static str> {
        if cancelled() {
            return Err("Cancelled");
        }
        if previous.is_some_and(|p| !Arc::ptr_eq(&p.pool, &pool)) {
            return Err("Previous atlas belongs to another pool");
        }
        if requested.len() > pool.capacity
            || requested
                .iter()
                .enumerate()
                .any(|(a, i)| *i >= pool.source.layout.len() || requested[..a].contains(i))
        {
            return Err("Requested atlas pages exceed bounds or repeat");
        }
        let count = requested.len();
        if let Some(old) = previous
            && old.slots.iter().flatten().count() == count
            && old.slots.iter().flatten().all(|i| requested.contains(i))
        {
            if cancelled() {
                return Err("Cancelled");
            }
            return Ok(old.clone());
        }
        let source = &pool.source;
        let device = source.device();
        let queue = source.queue();
        let metadata = MemoryBudget::volumes()
            .reserve(
                (pool.capacity * std::mem::size_of::<Option<usize>>()
                    + source.layout.len() * 32
                    + std::mem::size_of::<Self>()
                    + 16) as u64,
            )
            .ok_or("Not enough memory for the source atlas table")?;
        let gpu = allocation::reserve_bytes(pool.bytes, device, queue)?;
        let mut slots = Vec::new();
        slots
            .try_reserve_exact(pool.capacity)
            .map_err(|_| "Not enough memory for source atlas slots")?;
        slots.resize(pool.capacity, None);
        if let Some(old) = previous {
            for (slot, page) in old.slots.iter().enumerate() {
                if page.is_some_and(|i| requested.contains(&i)) {
                    slots[slot] = *page;
                }
            }
        }
        for &i in requested {
            if !slots.contains(&Some(i)) {
                let slot = slots
                    .iter_mut()
                    .find(|v| v.is_none())
                    .ok_or("Source atlas slot accounting failed")?;
                *slot = Some(i);
            }
        }
        let mut table = Vec::<[u32; 8]>::new();
        table
            .try_reserve_exact(source.layout.len())
            .map_err(|_| "Not enough memory for source atlas entries")?;
        table.resize(source.layout.len(), [0; 8]);
        let [w, h, d] = pool.texture_size();
        let extent = wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: d,
        };
        let descriptor = |label, format, size| wgpu::TextureDescriptor {
            label: Some(label),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D3,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        };
        let texture = allocation::texture(
            device,
            &descriptor(
                "immutable scalar atlas snapshot",
                wgpu::TextureFormat::R32Float,
                extent,
            ),
        )?;
        let masked = source.source.has_missing_scalar();
        let validity = allocation::texture(
            device,
            &descriptor(
                "immutable source atlas validity",
                wgpu::TextureFormat::R8Uint,
                if masked {
                    extent
                } else {
                    wgpu::Extent3d {
                        width: 1,
                        height: 1,
                        depth_or_array_layers: 1,
                    }
                },
            ),
        )?;
        if !masked {
            queue.write_texture(
                validity.as_image_copy(),
                &[1],
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(1),
                    rows_per_image: Some(1),
                },
                wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
            );
        }
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("immutable source atlas copies"),
        });
        let mut statistics = AtlasCopyStatistics::default();
        for (slot, index) in slots.iter().enumerate() {
            let Some(index) = *index else { continue };
            if cancelled() {
                return Err("Cancelled");
            }
            let b = source
                .layout
                .brick(index)
                .ok_or("Source atlas page missing")?;
            let origin = pool.origin(slot);
            table[index] = [
                b.origin[0] as u32,
                b.origin[1] as u32,
                b.origin[2] as u32,
                slot as u32 + 1,
                b.size[0] as u32,
                b.size[1] as u32,
                b.size[2] as u32,
                u32::from(source.layout.stencil() == BrickStencil::CentralSecants),
            ];
            let reused = previous.and_then(|p| {
                p.slots
                    .iter()
                    .position(|v| *v == Some(index))
                    .map(|s| (p, s))
            });
            let mut destination = texture.as_image_copy();
            destination.origin = origin;
            let size = wgpu::Extent3d {
                width: b.size[0] as u32,
                height: b.size[1] as u32,
                depth_or_array_layers: b.size[2] as u32,
            };
            if let Some((old, old_slot)) = reused {
                let mut input = old.texture.as_image_copy();
                input.origin = pool.origin(old_slot);
                encoder.copy_texture_to_texture(input, destination, size);
                if masked {
                    let mut input = old.validity.as_image_copy();
                    input.origin = pool.origin(old_slot);
                    let mut destination = validity.as_image_copy();
                    destination.origin = origin;
                    encoder.copy_texture_to_texture(input, destination, size);
                }
                statistics.copied_from_prior_atlas += 1;
            } else if let Some(pages) = pages {
                let page = pages.page(index).ok_or("Source atlas input page missing")?;
                encoder.copy_texture_to_texture(page.texture.as_image_copy(), destination, size);
                if masked {
                    let mut destination = validity.as_image_copy();
                    destination.origin = origin;
                    encoder.copy_texture_to_texture(
                        page.validity.as_image_copy(),
                        destination,
                        size,
                    );
                }
                statistics.copied_from_pages += 1;
            } else {
                continue;
            }
            statistics.copied_bytes +=
                b.size.into_iter().product::<usize>() as u64 * if masked { 5 } else { 4 };
        }
        // Submit copies before queued writes; later upload initialization must
        // not reorder source writes ahead of prior immutable atlas contents.
        queue.submit([encoder.finish()]);
        if pages.is_none() {
            let (count, bytes) = upload::source(
                &pool,
                &slots,
                previous.map_or(&[][..], |p| p.slots.as_slice()),
                &texture,
                &validity,
                &cancelled,
            )?;
            statistics.uploaded_pages = count;
            statistics.uploaded_bytes = bytes;
        }
        let dimensions = source.source.dimensions();
        let cells = source.layout.cell_side()[0];
        let words = [
            [
                dimensions[0] as u32,
                dimensions[1] as u32,
                dimensions[2] as u32,
                u32::from(masked),
            ],
            [
                ((dimensions[0] - 1).div_ceil(cells)) as u32,
                ((dimensions[1] - 1).div_ceil(cells)) as u32,
                ((dimensions[2] - 1).div_ceil(cells)) as u32,
                cells as u32,
            ],
            [pool.grid[0], pool.grid[1], pool.grid[2], 0],
            [
                pool.stored[0],
                pool.stored[1],
                pool.stored[2],
                source.layout.len() as u32,
            ],
        ];
        let parameters = allocation::initialized_buffer(
            device,
            "immutable source atlas dimensions",
            bytemuck::cast_slice(&words),
            wgpu::BufferUsages::UNIFORM,
        )?;
        let directory = allocation::initialized_buffer(
            device,
            "immutable source atlas page table",
            bytemuck::cast_slice(&table),
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        )?;
        let values = texture.create_view(&Default::default());
        let valid = validity.create_view(&Default::default());
        let memory = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let bindings = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("source bound atlas snapshot"),
            layout: &pool.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&values),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&valid),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: parameters.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: directory.as_entire_binding(),
                },
            ],
        });
        let invalid = pollster::block_on(validation.pop()).is_some();
        let exhausted = pollster::block_on(memory.pop()).is_some();
        if invalid || exhausted {
            return Err("The GPU could not bind the source atlas snapshot");
        }
        flush_upload(device, queue, &cancelled)?;
        if cancelled() {
            return Err("Cancelled");
        }
        Ok(Arc::new(Self {
            pool,
            slots,
            texture,
            validity,
            directory,
            bindings,
            statistics,
            _metadata: metadata,
            _gpu: gpu,
        }))
    }
    pub fn pool(&self) -> &Arc<GpuBrickAtlasPool> {
        &self.pool
    }
    pub fn bindings(&self) -> &wgpu::BindGroup {
        &self.bindings
    }
    pub fn statistics(&self) -> AtlasCopyStatistics {
        self.statistics
    }
    pub fn contains(&self, index: usize) -> bool {
        self.slots.contains(&Some(index))
    }
    pub fn resident_indices(&self) -> impl Iterator<Item = usize> + '_ {
        self.slots.iter().flatten().copied()
    }
}
