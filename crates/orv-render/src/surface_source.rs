// SPDX-License-Identifier: GPL-3.0-or-later
//! Immutable source-bound quad buffers; allocation, packing and fences are worker work.
pub use crate::label_source::UploadKind;
use crate::{
    volume::{allocation, flush_upload},
    volume_source::GpuVolumeSource,
};
use orv_core::{
    resources::{MemoryBudget, Reservation},
    surface::{CellChunk, CellFace, CellSurface, FaceBoundary},
};
use std::sync::Arc;

pub const DESCRIPTOR_BYTES: u64 = 16;
const STAGING_LIMIT: u64 = 16 * 1024 * 1024;
const BUFFER_ADMISSION_ALIGNMENT: u64 = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UploadStatistics {
    pub kind: UploadKind,
    pub uploaded_chunks: usize,
    pub reused_chunks: usize,
    /// Previously nonempty coordinates that are now empty; replacements are uploads.
    pub removed_chunks: usize,
    pub uploaded_bytes: u64,
    /// New targets, rounded conservatively per buffer; not driver/VRAM measurement.
    pub gpu_admission_bytes: u64,
    pub staging_admission_bytes: u64,
    pub cpu_metadata_admission_bytes: u64,
    /// Data upload fences, excluding source work and retirement submissions.
    pub fenced_batches: usize,
}
/// Integer instance stream: (cell x, y, z, side | grid-truncation << 8),
/// with side values 0..5 in X-, X+, Y-, Y+, Z-, Z+ order. Other bits are zero.
/// It contains no expanded vertices, normals, segment names or source identifiers.
pub struct GpuCellChunk {
    source: Arc<GpuVolumeSource>,
    cpu: Arc<CellChunk>,
    buffer: wgpu::Buffer,
    data_bytes: u64,
    admitted_bytes: u64,
    _gpu: allocation::GpuReservation,
    _cpu_metadata: Reservation,
}
impl GpuCellChunk {
    pub fn source(&self) -> &Arc<GpuVolumeSource> {
        &self.source
    }
    /// May retain an earlier equal geometry allocation. The parent surface,
    /// rather than this descriptor owner, carries the current map revision.
    pub fn cpu_chunk(&self) -> &Arc<CellChunk> {
        &self.cpu
    }
    /// Immutable VERTEX | COPY_SRC data. Retain this chunk/parent Arc until every
    /// command using it has been submitted; a cloned raw buffer is not admission ownership.
    pub fn buffer(&self) -> &wgpu::Buffer {
        &self.buffer
    }
    pub fn data_bytes(&self) -> u64 {
        self.data_bytes
    }
    pub fn admitted_bytes(&self) -> u64 {
        self.admitted_bytes
    }
}
pub struct GpuCellSurface {
    source: Arc<GpuVolumeSource>,
    surface: Arc<CellSurface>,
    chunks: Vec<Option<Arc<GpuCellChunk>>>,
    _cpu_directory: Reservation,
}
fn index(coordinate: [usize; 3], grid: [usize; 3]) -> Option<usize> {
    coordinate
        .iter()
        .zip(grid)
        .all(|(a, b)| *a < b)
        .then(|| (coordinate[2] * grid[1] + coordinate[1]) * grid[0] + coordinate[0])
}
fn compatible(
    old: Option<&Arc<GpuCellSurface>>,
    cpu: &Arc<CellChunk>,
) -> Option<Arc<GpuCellChunk>> {
    old.and_then(|old| old.chunk(cpu.coordinate()))
        .filter(|old| Arc::ptr_eq(&old.cpu, cpu) || old.cpu.faces() == cpu.faces())
        .cloned()
}
fn words(face: &CellFace) -> [u32; 4] {
    let [x, y, z] = face.cell();
    [
        x,
        y,
        z,
        face.side() as u32 | (u32::from(face.boundary() == FaceBoundary::Grid) << 8),
    ]
}
impl GpuCellSurface {
    pub fn source(&self) -> &Arc<GpuVolumeSource> {
        &self.source
    }
    pub fn surface(&self) -> &Arc<CellSurface> {
        &self.surface
    }
    pub fn chunks(&self) -> impl Iterator<Item = &Arc<GpuCellChunk>> {
        self.chunks.iter().flatten()
    }
    pub fn chunk(&self, coordinate: [usize; 3]) -> Option<&Arc<GpuCellChunk>> {
        self.chunks
            .get(index(coordinate, self.surface.chunk_grid())?)?
            .as_ref()
    }
    pub fn referenced_data_bytes(&self) -> u64 {
        self.chunks().map(|c| c.data_bytes).sum()
    }
    pub fn referenced_gpu_admission_bytes(&self) -> u64 {
        self.chunks().map(|c| c.admitted_bytes).sum()
    }
    /// Upload complete immutable chunk buffers on the source's device/queue.
    /// Related revisions reuse equal geometry, even when CPU halo rebuilding
    /// produced another equal chunk allocation. No old GPU buffer is overwritten.
    pub fn upload(
        source: Arc<GpuVolumeSource>,
        surface: Arc<CellSurface>,
        previous: Option<Arc<Self>>,
        cancelled: impl Fn() -> bool,
    ) -> Result<(Arc<Self>, UploadStatistics), &'static str> {
        if !Arc::ptr_eq(surface.source(), source.volume()) {
            return Err("Cell surface and GPU source differ");
        }
        if previous
            .as_ref()
            .is_some_and(|old| !Arc::ptr_eq(&old.source, &source))
        {
            return Err("Previous surface upload belongs to another GPU source");
        }
        if cancelled() {
            return Err("Cancelled");
        }
        let related = previous
            .as_ref()
            .filter(|old| old.surface.key() == surface.key());
        let mut stats = UploadStatistics {
            kind: if related.is_some() {
                UploadKind::Delta
            } else {
                UploadKind::Full
            },
            uploaded_chunks: 0,
            reused_chunks: 0,
            removed_chunks: 0,
            uploaded_bytes: 0,
            gpu_admission_bytes: 0,
            staging_admission_bytes: 0,
            cpu_metadata_admission_bytes: 0,
            fenced_batches: 0,
        };
        if let Some(old) = related
            && old.surface.revision() == surface.revision()
        {
            stats.kind = UploadKind::Reuse;
            stats.reused_chunks = old.chunks().count();
            return Ok((old.clone(), stats));
        }
        let device = source.device();
        let queue = source.queue();
        let grid = surface.chunk_grid();
        let count = grid
            .into_iter()
            .try_fold(1usize, usize::checked_mul)
            .filter(|n| *n <= orv_core::surface::MAX_CHUNKS)
            .ok_or("Surface directory exceeds its limit")?;
        let mut largest_staging = 0;
        for cpu in surface.chunks() {
            if cancelled() {
                return Err("Cancelled");
            }
            if compatible(related, cpu).is_some() {
                stats.reused_chunks += 1;
                continue;
            }
            let bytes = (cpu.faces().len() as u64)
                .checked_mul(DESCRIPTOR_BYTES)
                .filter(|n| *n > 0 && *n <= device.limits().max_buffer_size)
                .ok_or("Cell geometry exceeds this GPU's buffer limit")?;
            let admitted = bytes.next_multiple_of(BUFFER_ADMISSION_ALIGNMENT);
            largest_staging = largest_staging.max(admitted);
            stats.uploaded_chunks += 1;
            stats.uploaded_bytes += bytes;
            stats.gpu_admission_bytes += admitted;
        }
        if let Some(old) = related {
            stats.removed_chunks = old
                .chunks()
                .filter(|c| surface.chunk(c.cpu.coordinate()).is_none())
                .count();
        }
        stats.staging_admission_bytes = stats
            .gpu_admission_bytes
            .min(STAGING_LIMIT + largest_staging);
        // Mapped-at-creation buffers may retain driver upload staging until the
        // next submission. Fence bounded batches, rather than assuming unmap frees it.
        let _staging = if stats.staging_admission_bytes == 0 {
            None
        } else {
            Some(allocation::reserve_bytes(
                stats.staging_admission_bytes,
                device,
                queue,
            )?)
        };
        let directory_bytes = (count * std::mem::size_of::<Option<Arc<GpuCellChunk>>>()
            + std::mem::size_of::<Self>()
            + 2 * std::mem::size_of::<usize>()) as u64;
        let cpu_budget = MemoryBudget::volumes();
        let directory = cpu_budget
            .reserve(directory_bytes)
            .ok_or("Not enough memory for GPU geometry ownership")?;
        stats.cpu_metadata_admission_bytes = directory_bytes;
        let mut chunks = Vec::new();
        chunks
            .try_reserve_exact(count)
            .map_err(|_| "Not enough memory for GPU geometry ownership")?;
        chunks.resize_with(count, || None);
        let mut pending = 0;
        for cpu in surface.chunks() {
            if cancelled() {
                return Err("Cancelled");
            }
            let slot =
                index(cpu.coordinate(), grid).ok_or("Surface chunk is outside its source")?;
            if let Some(old) = compatible(related, cpu) {
                chunks[slot] = Some(old);
                continue;
            }
            let bytes = cpu.faces().len() as u64 * DESCRIPTOR_BYTES;
            let admitted = bytes.next_multiple_of(BUFFER_ADMISSION_ALIGNMENT);
            let gpu = allocation::reserve_bytes(admitted, device, queue)?;
            let metadata_bytes =
                (std::mem::size_of::<GpuCellChunk>() + 2 * std::mem::size_of::<usize>()) as u64;
            let metadata = cpu_budget
                .reserve(metadata_bytes)
                .ok_or("Not enough memory for GPU chunk ownership")?;
            let buffer = allocation::buffer(
                device,
                &wgpu::BufferDescriptor {
                    label: Some("immutable cell quad descriptors"),
                    size: bytes,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_SRC,
                    mapped_at_creation: true,
                },
            )?;
            let packed = pack(&buffer, cpu, &cancelled);
            buffer.unmap();
            packed?;
            chunks[slot] = Some(Arc::new(GpuCellChunk {
                source: source.clone(),
                cpu: cpu.clone(),
                buffer,
                data_bytes: bytes,
                admitted_bytes: admitted,
                _gpu: gpu,
                _cpu_metadata: metadata,
            }));
            stats.cpu_metadata_admission_bytes += metadata_bytes;
            pending += admitted;
            if pending >= STAGING_LIMIT {
                flush_upload(device, queue, &cancelled)?;
                stats.fenced_batches += 1;
                pending = 0;
            }
        }
        if pending != 0 {
            flush_upload(device, queue, &cancelled)?;
            stats.fenced_batches += 1;
        }
        if cancelled() {
            return Err("Cancelled");
        }
        Ok((
            Arc::new(Self {
                source,
                surface,
                chunks,
                _cpu_directory: directory,
            }),
            stats,
        ))
    }
}
fn pack(
    buffer: &wgpu::Buffer,
    cpu: &CellChunk,
    cancelled: &impl Fn() -> bool,
) -> Result<(), &'static str> {
    let mut mapped = buffer
        .get_mapped_range_mut(..)
        .map_err(|_| "Could not map geometry upload")?;
    if mapped.len() != cpu.faces().len() * DESCRIPTOR_BYTES as usize {
        return Err("Geometry mapping size differs from its descriptors");
    }
    for (i, face) in cpu.faces().iter().enumerate() {
        if i.is_multiple_of(256) && cancelled() {
            return Err("Cancelled");
        }
        // The pinned API exposes write-only mapped memory. Construct the small
        // value on the stack instead of treating that mapping as a readable slice.
        let encoded = words(face).map(u32::to_le_bytes);
        let start = i * DESCRIPTOR_BYTES as usize;
        mapped
            .slice(start..start + DESCRIPTOR_BYTES as usize)
            .copy_from_slice(encoded.as_flattened());
    }
    Ok(())
}

#[cfg(all(test, feature = "gpu-tests"))]
mod tests;
