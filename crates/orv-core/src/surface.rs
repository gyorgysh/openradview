// SPDX-License-Identifier: GPL-3.0-or-later
//! Bounded exact source-cell boundaries; independent of a smoothed interpolant.
//! Unknown coverage and missing samples leave open boundaries. Grid truncation
//! faces are marked separately. These derived faces do not authorize an edit.
mod face;
pub mod field;
mod layout;
mod ray;
mod walk;
use crate::{
    labels::LABEL_TILE_SIDE,
    mask::{MaskKey, MaskOwner, MaskRevision},
    resources::{MemoryBudget, Reservation},
    volume::Volume,
};
pub use face::{CellFace, FaceBoundary, FaceGeometry, FaceSide};
use layout::Layout;
pub use ray::{InterfaceHit, InterfaceRay, InterfaceRayError};
use std::sync::Arc;

pub const CHUNK_SIDE: usize = LABEL_TILE_SIDE;
pub const MAX_CHUNKS: usize = 65536;
pub const MAX_FACES: usize = 2 * 1024 * 1024;
pub const MAX_BYTES: u64 = 128 * 1024 * 1024;

// Arc stores two counters before the value, with padding to the value's alignment.
// Some constructors are used both by value and inside Arc; charging the header
// conservatively keeps a later shared owner admitted after its caller is dropped.
fn shared_header_bytes<T>() -> usize {
    let header = 2 * std::mem::size_of::<usize>();
    let align = std::mem::align_of::<T>();
    let remainder = header % align;
    if remainder == 0 {
        header
    } else {
        header + align - remainder
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SurfaceError {
    Selection,
    Limit,
    Memory,
    Cancelled,
    Geometry,
}
impl std::fmt::Display for SurfaceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Selection => "Choose an existing nonzero mask ID",
            Self::Limit => "Cell surface exceeds its geometry limit; keep the mask-ray view",
            Self::Memory => "Not enough memory for the cell surface; keep the mask-ray view",
            Self::Cancelled => "Cancelled",
            Self::Geometry => "Cell surface geometry cannot be represented",
        })
    }
}
impl std::error::Error for SurfaceError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SurfaceLimits {
    pub max_faces: usize,
    /// Complete generation's directory and referenced chunk storage, even when shared.
    pub max_bytes: u64,
}
impl Default for SurfaceLimits {
    fn default() -> Self {
        Self {
            max_faces: MAX_FACES,
            max_bytes: MAX_BYTES,
        }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SurfaceUpdate {
    pub rebuilt_chunks: usize,
    pub reused_chunks: usize,
    /// Counts cell visits separately in the counting and emitting passes; excludes halo reads.
    pub counted_cells: usize,
    pub emitted_cells: usize,
    /// New directory/chunks only. Old/shared chunks stay charged once to their owners.
    pub allocated_bytes: u64,
}
pub struct CellChunk {
    coordinate: [usize; 3],
    faces: Vec<CellFace>,
    bytes: u64,
    _reservation: Reservation,
}
impl CellChunk {
    pub fn coordinate(&self) -> [usize; 3] {
        self.coordinate
    }
    pub fn faces(&self) -> &[CellFace] {
        &self.faces
    }
    pub fn storage_bytes(&self) -> u64 {
        self.bytes
    }
}
pub struct CellSurface {
    owner: MaskOwner,
    source: Arc<Volume>,
    key: MaskKey,
    revision: MaskRevision,
    layout: Layout,
    chunks: Vec<Option<Arc<CellChunk>>>,
    faces: usize,
    bytes: u64,
    budget: Arc<MemoryBudget>,
    _directory: Reservation,
}
fn allocated<T>(count: usize) -> Result<Vec<T>, SurfaceError> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(count)
        .map_err(|_| SurfaceError::Memory)?;
    Ok(values)
}
// Callers reserve the retained storage before copying. Chunking keeps large
// immutable geometry copies cancellable without admitting an unbounded buffer.
fn copy_retained<T: Copy>(input: &[T], cancel: &impl Fn() -> bool) -> Result<Vec<T>, SurfaceError> {
    let mut output = allocated(input.len())?;
    if output.capacity() != input.len() {
        return Err(SurfaceError::Memory);
    }
    for chunk in input.chunks(1024) {
        if cancel() {
            return Err(SurfaceError::Cancelled);
        }
        output.extend_from_slice(chunk);
    }
    Ok(output)
}
fn chunk_bytes(faces: usize) -> u64 {
    if faces == 0 {
        0
    } else {
        (faces * std::mem::size_of::<CellFace>()
            + std::mem::size_of::<CellChunk>()
            + 2 * std::mem::size_of::<usize>()) as u64
    }
}
impl CellSurface {
    pub fn source(&self) -> &Arc<Volume> {
        &self.source
    }
    pub fn key(&self) -> &MaskKey {
        &self.key
    }
    pub fn revision(&self) -> &MaskRevision {
        &self.revision
    }
    pub fn owner(&self) -> &MaskOwner {
        &self.owner
    }
    pub fn face_count(&self) -> usize {
        self.faces
    }
    /// Typed storage admission, excluding source/map storage and allocator/process overhead.
    pub fn referenced_bytes(&self) -> u64 {
        self.bytes
    }
    pub fn chunk_grid(&self) -> [usize; 3] {
        self.layout.grid
    }
    pub fn chunk(&self, coordinate: [usize; 3]) -> Option<&Arc<CellChunk>> {
        self.chunks.get(self.layout.index(coordinate)?)?.as_ref()
    }
    pub fn chunks(&self) -> impl Iterator<Item = &Arc<CellChunk>> {
        self.chunks.iter().flatten()
    }
    pub fn physical_face(&self, face: &CellFace) -> Result<FaceGeometry, SurfaceError> {
        face.physical(&self.source)
    }
    /// Worker-only. Count the entire prospective generation before allocating its
    /// faces. An error/cancellation leaves every earlier generation unchanged.
    pub fn build(
        owner: MaskOwner,
        number: u16,
        previous: Option<&Arc<Self>>,
        limits: SurfaceLimits,
        cancelled: impl Fn() -> bool,
    ) -> Result<(Arc<Self>, SurfaceUpdate), SurfaceError> {
        Self::with_budget(
            owner,
            number,
            previous,
            limits,
            MemoryBudget::volumes(),
            cancelled,
        )
    }
    fn with_budget(
        owner: MaskOwner,
        number: u16,
        previous: Option<&Arc<Self>>,
        limits: SurfaceLimits,
        budget: Arc<MemoryBudget>,
        cancelled: impl Fn() -> bool,
    ) -> Result<(Arc<Self>, SurfaceUpdate), SurfaceError> {
        if cancelled() {
            return Err(SurfaceError::Cancelled);
        }
        if limits.max_faces > MAX_FACES || limits.max_bytes > MAX_BYTES {
            return Err(SurfaceError::Limit);
        }
        let source = owner.source();
        let view = owner.view(number).map_err(|_| SurfaceError::Selection)?;
        let key = view.key();
        let revision = view.revision().clone();
        let layout = Layout::new(source.dimensions())?;
        let related = previous.filter(|old| {
            Arc::ptr_eq(&old.source, &source) && old.key == key && Arc::ptr_eq(&old.budget, &budget)
        });
        if let Some(old) = related
            && old.revision == revision
        {
            if old.faces > limits.max_faces || old.bytes > limits.max_bytes {
                return Err(SurfaceError::Limit);
            }
            return Ok((
                old.clone(),
                SurfaceUpdate {
                    reused_chunks: layout.len,
                    ..Default::default()
                },
            ));
        }
        let scratch_bytes =
            (layout.len * (std::mem::size_of::<usize>() + 1) + walk::HALO_CELLS) as u64;
        let _scratch = budget.reserve(scratch_bytes).ok_or(SurfaceError::Memory)?;
        let mut counts = allocated(layout.len)?;
        counts.resize(layout.len, 0usize);
        let mut dirty = allocated(layout.len)?;
        dirty.resize(layout.len, true);
        let mut halo = allocated(walk::HALO_CELLS)?;
        if let Some(old) = related
            && let (MaskOwner::Manual(current), MaskOwner::Manual(prior)) = (&owner, &old.owner)
        {
            dirty.fill(false);
            for tile in current
                .changed_tiles_since(prior)
                .map_err(|_| SurfaceError::Selection)?
            {
                if cancelled() {
                    return Err(SurfaceError::Cancelled);
                }
                layout.mark_neighbors(tile.coordinate, &mut dirty)?;
            }
        }
        let directory_bytes = (layout.len * std::mem::size_of::<Option<Arc<CellChunk>>>()
            + std::mem::size_of::<Self>()
            + 2 * std::mem::size_of::<usize>()) as u64;
        let mut bytes = directory_bytes;
        let mut faces = 0usize;
        let mut update = SurfaceUpdate::default();
        for index in 0..layout.len {
            if cancelled() {
                return Err(SurfaceError::Cancelled);
            }
            if dirty[index] {
                update.rebuilt_chunks += 1;
                update.counted_cells +=
                    walk::faces(view, layout.bounds(index), &mut halo, &cancelled, |_| {
                        faces += 1;
                        counts[index] += 1;
                        if faces > limits.max_faces {
                            Err(SurfaceError::Limit)
                        } else {
                            Ok(())
                        }
                    })?;
            } else if let Some(old) = related {
                update.reused_chunks += 1;
                counts[index] = old.chunks[index].as_ref().map_or(0, |v| v.faces.len());
                faces += counts[index];
            }
            bytes += chunk_bytes(counts[index]);
            if faces > limits.max_faces || bytes > limits.max_bytes {
                return Err(SurfaceError::Limit);
            }
        }
        let directory = budget
            .reserve(directory_bytes)
            .ok_or(SurfaceError::Memory)?;
        let mut chunks = allocated(layout.len)?;
        update.allocated_bytes = directory_bytes;
        for index in 0..layout.len {
            if cancelled() {
                return Err(SurfaceError::Cancelled);
            }
            if !dirty[index] {
                chunks.push(related.and_then(|old| old.chunks[index].clone()));
                continue;
            }
            if counts[index] == 0 {
                chunks.push(None);
                continue;
            }
            let bytes = chunk_bytes(counts[index]);
            let reservation = budget.reserve(bytes).ok_or(SurfaceError::Memory)?;
            let mut output = allocated(counts[index])?;
            update.emitted_cells +=
                walk::faces(view, layout.bounds(index), &mut halo, &cancelled, |face| {
                    if output.len() >= counts[index] {
                        return Err(SurfaceError::Geometry);
                    }
                    output.push(face);
                    Ok(())
                })?;
            if output.len() != counts[index] {
                return Err(SurfaceError::Geometry);
            }
            update.allocated_bytes += bytes;
            chunks.push(Some(Arc::new(CellChunk {
                coordinate: layout.coordinate(index),
                faces: output,
                bytes,
                _reservation: reservation,
            })));
        }
        if cancelled() {
            return Err(SurfaceError::Cancelled);
        }
        Ok((
            Arc::new(Self {
                owner,
                source,
                key,
                revision,
                layout,
                chunks,
                faces,
                bytes,
                budget,
                _directory: directory,
            }),
            update,
        ))
    }
}

#[cfg(test)]
mod tests;
