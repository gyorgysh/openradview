// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
/// Private renderer cache. Completed images retain their own radiance and CPU
/// source identity, never these slots. A load fences all submitted work first;
/// its caller must keep no encoded-but-unsubmitted use of the cache bindings.
pub(crate) struct GpuBrickAtlasStream {
    data: GpuBrickAtlas,
    table: Vec<[u32; 8]>,
    _metadata: Reservation,
}
impl GpuBrickAtlasStream {
    pub(crate) fn new(
        pool: Arc<GpuBrickAtlasPool>,
        cancelled: &impl Fn() -> bool,
    ) -> Result<Self, &'static str> {
        let metadata = MemoryBudget::volumes()
            .reserve(
                (std::mem::size_of::<Self>() - std::mem::size_of::<GpuBrickAtlas>() + 16) as u64,
            )
            .ok_or("Not enough memory for private source cache metadata")?;
        let count = pool.source.layout.len();
        let data = Arc::try_unwrap(GpuBrickAtlas::load(pool, &[], None, cancelled)?)
            .map_err(|_| "The private source cache was shared during construction")?;
        // The atlas owner already admits this full directory's CPU storage.
        let mut table = Vec::new();
        table
            .try_reserve_exact(count)
            .map_err(|_| "Not enough memory for the private source directory")?;
        table.resize(count, [0; 8]);
        Ok(Self {
            data,
            table,
            _metadata: metadata,
        })
    }
    pub(crate) fn load(
        &mut self,
        requested: &[usize],
        cancelled: &impl Fn() -> bool,
    ) -> Result<AtlasCopyStatistics, &'static str> {
        if cancelled() {
            return Err("Cancelled");
        }
        let pool = self.data.pool.clone();
        let source = &pool.source;
        let device = source.device();
        let queue = source.queue();
        if requested.len() > pool.capacity
            || requested
                .iter()
                .enumerate()
                .any(|(a, i)| *i >= source.layout.len() || requested[..a].contains(i))
        {
            return Err("Requested source cache pages exceed bounds or repeat");
        }
        if requested.len() == self.data.slots.iter().flatten().count()
            && requested.iter().all(|i| self.contains(*i))
        {
            return Ok(AtlasCopyStatistics::default());
        }
        let _temporary = MemoryBudget::volumes()
            .reserve((pool.capacity * (2 * std::mem::size_of::<Option<usize>>() + 32) + 96) as u64)
            .ok_or("Not enough memory for source cache replacement metadata")?;
        let _staging =
            allocation::reserve_bytes((self.table.len() * 32 * 2) as u64, device, queue)?;
        let mut old = Vec::new();
        old.try_reserve_exact(pool.capacity)
            .map_err(|_| "Not enough memory for prior source slots")?;
        old.extend_from_slice(&self.data.slots);
        let mut next = Vec::new();
        next.try_reserve_exact(pool.capacity)
            .map_err(|_| "Not enough memory for replacement source slots")?;
        next.resize(pool.capacity, None);
        for (slot, index) in old.iter().enumerate() {
            if index.is_some_and(|i| requested.contains(&i)) {
                next[slot] = *index;
            }
        }
        for &index in requested {
            if !next.contains(&Some(index)) {
                *next
                    .iter_mut()
                    .find(|i| i.is_none())
                    .ok_or("Source cache slot accounting failed")? = Some(index);
            }
        }
        let mut entries = Vec::new();
        entries
            .try_reserve_exact(pool.capacity)
            .map_err(|_| "Not enough memory for replacement source entries")?;
        entries.resize(pool.capacity, [0; 8]);
        for (slot, index) in next.iter().enumerate() {
            if let Some(index) = *index {
                let b = source
                    .layout
                    .brick(index)
                    .ok_or("Invalid replacement source page")?;
                entries[slot] = [
                    b.origin[0] as u32,
                    b.origin[1] as u32,
                    b.origin[2] as u32,
                    slot as u32 + 1,
                    b.size[0] as u32,
                    b.size[1] as u32,
                    b.size[2] as u32,
                    u32::from(source.layout.stencil() == BrickStencil::CentralSecants),
                ];
            }
        }
        // No old GPU reader may survive slot reuse. The renderer encodes and
        // submits each wave immediately, so this also excludes unsubmitted uses.
        flush_upload(device, queue, cancelled)?;
        let mut invalidated = false;
        for page in &mut self.data.slots {
            if page.is_some_and(|i| !requested.contains(&i)) {
                let index = page.take().ok_or("Invalid source cache removal")?;
                self.table[index] = [0; 8];
                invalidated = true;
            }
        }
        if invalidated {
            queue.write_buffer(&self.data.directory, 0, bytemuck::cast_slice(&self.table));
            queue.submit([]);
        }
        // On failure, overwritten slots remain unmapped. Reused slots still
        // contain their exact source pages, and the old completed image is separate.
        let (count, bytes) = upload::source(
            &pool,
            &next,
            &old,
            &self.data.texture,
            &self.data.validity,
            cancelled,
        )?;
        if cancelled() {
            return Err("Cancelled");
        }
        for (slot, index) in next.iter().enumerate() {
            if let Some(index) = *index {
                self.table[index] = entries[slot];
            }
        }
        self.data.slots = next;
        queue.write_buffer(&self.data.directory, 0, bytemuck::cast_slice(&self.table));
        queue.submit([]);
        flush_upload(device, queue, cancelled)?;
        Ok(AtlasCopyStatistics {
            uploaded_pages: count,
            uploaded_bytes: bytes,
            ..Default::default()
        })
    }
    pub(crate) fn contains(&self, index: usize) -> bool {
        self.data.contains(index)
    }
    pub(crate) fn resident_indices(&self) -> impl Iterator<Item = usize> + '_ {
        self.data.resident_indices()
    }
    pub(crate) fn bindings(&self) -> &wgpu::BindGroup {
        self.data.bindings()
    }
}
