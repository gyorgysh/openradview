// SPDX-License-Identifier: GPL-3.0-or-later
//! Immutable integer triangle inputs for bounded, source-bound candidates.
use crate::{
    label_source::UploadKind,
    volume::{allocation, flush_upload},
    volume_source::GpuVolumeSource,
};
use orv_core::{
    resources::{MemoryBudget, Reservation},
    surface::field::{
        CandidateAssembly, CandidateCellState, ConformingFacetState, ConformingSurfaceMesh,
    },
};
use std::sync::Arc;
mod affine;
use affine::{AffineGeometry, quantize};

/// Cell origin xyz, three exact subcell xyz numerators and four packed pairs of
/// signed source-field samples. Coordinates use the mesh position denominator.
pub const TRIANGLE_BYTES: u64 = 64;
pub const MAX_TRIANGLES: usize = 1_000_000;
pub const MAX_BYTES: u64 = 128 * 1024 * 1024;
const ALIGNMENT: u64 = 4096;
const STAGING_LIMIT: u64 = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CandidateUploadLimits {
    /// Applies to the complete represented output, including reused chunks.
    pub max_triangles: usize,
    pub max_bytes: u64,
}
impl Default for CandidateUploadLimits {
    fn default() -> Self {
        Self {
            max_triangles: MAX_TRIANGLES,
            max_bytes: MAX_BYTES,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CandidateUploadStatistics {
    pub kind: UploadKind,
    pub represented_triangles: usize,
    /// Unresolved candidate triangles are not uploaded as certified geometry.
    pub unresolved_triangles: usize,
    pub noncandidate_cells: usize,
    /// Complete local affine polygons included in the partial assembly.
    pub affine_cells: usize,
    /// Affine polygons refused for precision, degeneracy or topology.
    pub unresolved_affine_cells: usize,
    /// Conservative source-space quantization bound for represented affine cells.
    pub affine_error_upper_mm: f64,
    pub uploaded_chunks: usize,
    pub reused_chunks: usize,
    pub removed_chunks: usize,
    pub uploaded_bytes: u64,
    pub gpu_admission_bytes: u64,
    pub staging_admission_bytes: u64,
    pub cpu_metadata_admission_bytes: u64,
    pub fenced_batches: usize,
}

/// One source-cell triangle stream. Reused buffers may retain an older equal
/// geometry object; the parent assembly supplies the current revision.
pub struct GpuCandidateChunk {
    source: Arc<GpuVolumeSource>,
    geometry: ChunkGeometry,
    origin: [u32; 3],
    buffer: wgpu::Buffer,
    triangles: usize,
    bytes: u64,
    admitted: u64,
    _gpu: allocation::GpuReservation,
    _metadata: Reservation,
}
pub(crate) enum ChunkGeometry {
    Mesh(Arc<ConformingSurfaceMesh>),
    Affine(AffineGeometry),
}
impl GpuCandidateChunk {
    pub(crate) fn geometry(&self) -> &ChunkGeometry {
        &self.geometry
    }
    pub fn source(&self) -> &Arc<GpuVolumeSource> {
        &self.source
    }
    pub fn origin(&self) -> [u32; 3] {
        self.origin
    }
    pub fn buffer(&self) -> &wgpu::Buffer {
        &self.buffer
    }
    pub fn triangle_count(&self) -> usize {
        self.triangles
    }
    pub fn data_bytes(&self) -> u64 {
        self.bytes
    }
    pub fn admitted_bytes(&self) -> u64 {
        self.admitted
    }
}

/// A partial candidate input, not a complete surface or edit authority. Retain
/// this owner and its chunks until command buffers using them have been submitted.
pub struct GpuCandidateSurface {
    source: Arc<GpuVolumeSource>,
    assembly: Arc<CandidateAssembly>,
    chunks: Vec<Arc<GpuCandidateChunk>>,
    affine_error_upper_mm: f64,
    unresolved_affine_cells: usize,
    _directory: Reservation,
}
impl GpuCandidateSurface {
    pub fn source(&self) -> &Arc<GpuVolumeSource> {
        &self.source
    }
    pub fn assembly(&self) -> &Arc<CandidateAssembly> {
        &self.assembly
    }
    pub fn chunks(&self) -> &[Arc<GpuCandidateChunk>] {
        &self.chunks
    }
    /// Source-space quantization bound; excludes projection and f32 GPU error.
    pub fn affine_error_upper_mm(&self) -> f64 {
        self.affine_error_upper_mm
    }
    /// Exact affine planes not admitted because their integer representation
    /// exceeds the requested tolerance or would change local topology.
    pub fn unresolved_affine_cells(&self) -> usize {
        self.unresolved_affine_cells
    }
    pub fn chunk(&self, origin: [u32; 3]) -> Option<&Arc<GpuCandidateChunk>> {
        self.chunks.iter().find(|chunk| chunk.origin == origin)
    }
    pub fn referenced_data_bytes(&self) -> u64 {
        self.chunks.iter().map(|chunk| chunk.bytes).sum()
    }

    /// Worker-only upload. Only within-tolerance triangles are represented;
    /// all other candidate facets and assembly gaps remain explicitly unproved.
    pub fn upload(
        source: Arc<GpuVolumeSource>,
        assembly: Arc<CandidateAssembly>,
        previous: Option<Arc<Self>>,
        limits: CandidateUploadLimits,
        cancelled: impl Fn() -> bool,
    ) -> Result<(Arc<Self>, CandidateUploadStatistics), &'static str> {
        if cancelled() {
            return Err("Cancelled");
        }
        if limits.max_triangles > MAX_TRIANGLES || limits.max_bytes > MAX_BYTES {
            return Err("Candidate geometry limits exceed the supported budget");
        }
        if !Arc::ptr_eq(assembly.field().source(), source.volume()) {
            return Err("Candidate geometry and GPU source differ");
        }
        if previous
            .as_ref()
            .is_some_and(|old| !Arc::ptr_eq(&old.source, &source))
        {
            return Err("Previous candidate upload belongs to another GPU source");
        }
        let related = previous
            .as_ref()
            .filter(|old| old.assembly.field().key() == assembly.field().key());
        let mut stats = CandidateUploadStatistics {
            kind: if related.is_some() {
                UploadKind::Delta
            } else {
                UploadKind::Full
            },
            represented_triangles: 0,
            unresolved_triangles: 0,
            noncandidate_cells: 0,
            affine_cells: 0,
            unresolved_affine_cells: 0,
            affine_error_upper_mm: 0.,
            uploaded_chunks: 0,
            reused_chunks: 0,
            removed_chunks: 0,
            uploaded_bytes: 0,
            gpu_admission_bytes: 0,
            staging_admission_bytes: 0,
            cpu_metadata_admission_bytes: 0,
            fenced_batches: 0,
        };
        let device = source.device();
        let queue = source.queue();
        let mut total_bytes = 0u64;
        let mut largest_upload = 0u64;
        let mut retained_origins = 0usize;
        for (index, cell) in assembly.cells().iter().enumerate() {
            if cancelled() {
                return Err("Cancelled");
            }
            let (geometry, refused) = select(&assembly, index, &source)?;
            stats.unresolved_affine_cells += usize::from(refused);
            let Some(geometry) = geometry else {
                stats.noncandidate_cells += 1;
                continue;
            };
            if let InputGeometry::Affine(affine) = &geometry {
                stats.affine_cells += 1;
                stats.affine_error_upper_mm = stats.affine_error_upper_mm.max(affine.error_mm);
            }
            let origin = checked_origin(cell.origin, source.volume().dimensions())?;
            let (represented, unresolved) = geometry.inspect(&cancelled)?;
            stats.represented_triangles = stats
                .represented_triangles
                .checked_add(represented)
                .ok_or("Candidate triangle count overflow")?;
            stats.unresolved_triangles = stats
                .unresolved_triangles
                .checked_add(unresolved)
                .ok_or("Candidate triangle count overflow")?;
            let bytes = triangle_bytes(represented)?;
            total_bytes = total_bytes
                .checked_add(bytes)
                .ok_or("Candidate geometry size overflow")?;
            if stats.represented_triangles > limits.max_triangles || total_bytes > limits.max_bytes
            {
                return Err("Candidate geometry exceeds its requested budget");
            }
            if represented == 0 {
                continue;
            }
            retained_origins += usize::from(related.is_some_and(|old| old.chunk(origin).is_some()));
            if bytes > device.limits().max_buffer_size {
                return Err("Candidate geometry exceeds this GPU's buffer limit");
            }
            if compatible(related, origin, &geometry).is_some() {
                stats.reused_chunks += 1;
            } else {
                let admitted = aligned(bytes)?;
                stats.uploaded_chunks += 1;
                stats.uploaded_bytes = stats
                    .uploaded_bytes
                    .checked_add(bytes)
                    .ok_or("Candidate geometry size overflow")?;
                stats.gpu_admission_bytes = stats
                    .gpu_admission_bytes
                    .checked_add(admitted)
                    .ok_or("Candidate geometry size overflow")?;
                largest_upload = largest_upload.max(admitted);
            }
        }
        if let Some(old) = related {
            stats.removed_chunks = old
                .chunks
                .len()
                .checked_sub(retained_origins)
                .ok_or("Candidate geometry contains duplicate cells")?;
            if Arc::ptr_eq(&old.assembly, &assembly) {
                if cancelled() {
                    return Err("Cancelled");
                }
                stats.kind = UploadKind::Reuse;
                return Ok((old.clone(), stats));
            }
        }
        stats.staging_admission_bytes = stats
            .gpu_admission_bytes
            .min(STAGING_LIMIT + largest_upload);
        let _staging = if stats.staging_admission_bytes == 0 {
            None
        } else {
            Some(allocation::reserve_bytes(
                stats.staging_admission_bytes,
                device,
                queue,
            )?)
        };
        let count = stats.uploaded_chunks + stats.reused_chunks;
        let directory_bytes = count
            .checked_mul(std::mem::size_of::<Arc<GpuCandidateChunk>>())
            .and_then(|v| {
                v.checked_add(std::mem::size_of::<Self>() + 2 * std::mem::size_of::<usize>())
            })
            .and_then(|v| u64::try_from(v).ok())
            .ok_or("Candidate directory size overflow")?;
        let cpu_budget = MemoryBudget::volumes();
        let directory = cpu_budget
            .reserve(directory_bytes)
            .ok_or("Not enough memory for candidate ownership")?;
        stats.cpu_metadata_admission_bytes = directory_bytes;
        let mut chunks = Vec::new();
        chunks
            .try_reserve_exact(count)
            .map_err(|_| "Not enough memory for candidate ownership")?;
        if chunks.capacity() != count {
            return Err("Candidate directory allocation exceeded its budget");
        }
        let mut pending = 0u64;
        for (index, cell) in assembly.cells().iter().enumerate() {
            if cancelled() {
                return Err("Cancelled");
            }
            let (geometry, _) = select(&assembly, index, &source)?;
            let Some(geometry) = geometry else {
                continue;
            };
            let origin = checked_origin(cell.origin, source.volume().dimensions())?;
            let (represented, _) = geometry.inspect(&cancelled)?;
            if represented == 0 {
                continue;
            }
            if let Some(old) = compatible(related, origin, &geometry) {
                chunks.push(old);
                continue;
            }
            let bytes = triangle_bytes(represented)?;
            let admitted = aligned(bytes)?;
            let gpu = allocation::reserve_bytes(admitted, device, queue)?;
            let metadata_bytes = (std::mem::size_of::<GpuCandidateChunk>()
                + 2 * std::mem::size_of::<usize>()) as u64;
            let metadata = cpu_budget
                .reserve(metadata_bytes)
                .ok_or("Not enough memory for candidate ownership")?;
            let buffer = allocation::buffer(
                device,
                &wgpu::BufferDescriptor {
                    label: Some("immutable candidate triangle vertices"),
                    size: bytes,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_SRC,
                    mapped_at_creation: true,
                },
            )?;
            let result = geometry.pack(&buffer, origin, represented, &cancelled);
            buffer.unmap();
            result?;
            chunks.push(Arc::new(GpuCandidateChunk {
                source: source.clone(),
                geometry: geometry.owned(),
                origin,
                buffer,
                triangles: represented,
                bytes,
                admitted,
                _gpu: gpu,
                _metadata: metadata,
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
                assembly,
                chunks,
                affine_error_upper_mm: stats.affine_error_upper_mm,
                unresolved_affine_cells: stats.unresolved_affine_cells,
                _directory: directory,
            }),
            stats,
        ))
    }
}

enum InputGeometry<'a> {
    Mesh(&'a Arc<ConformingSurfaceMesh>),
    Affine(AffineGeometry),
}
impl InputGeometry<'_> {
    fn inspect(&self, cancelled: &impl Fn() -> bool) -> Result<(usize, usize), &'static str> {
        match self {
            Self::Mesh(mesh) => inspect(mesh, cancelled),
            Self::Affine(geometry) => Ok((geometry.triangle_count(), 0)),
        }
    }
    fn pack(
        &self,
        buffer: &wgpu::Buffer,
        origin: [u32; 3],
        count: usize,
        cancel: &impl Fn() -> bool,
    ) -> Result<(), &'static str> {
        match self {
            Self::Mesh(mesh) => pack(buffer, mesh, origin, count, cancel),
            Self::Affine(geometry) => pack_affine(buffer, geometry, origin, count, cancel),
        }
    }
    fn owned(self) -> ChunkGeometry {
        match self {
            Self::Mesh(mesh) => ChunkGeometry::Mesh(mesh.clone()),
            Self::Affine(geometry) => ChunkGeometry::Affine(geometry),
        }
    }
}
fn select<'a>(
    assembly: &'a CandidateAssembly,
    index: usize,
    source: &GpuVolumeSource,
) -> Result<(Option<InputGeometry<'a>>, bool), &'static str> {
    let cell = assembly
        .cells()
        .get(index)
        .ok_or("Candidate cell index is invalid")?;
    let affine = assembly
        .affine_cells()
        .get(index)
        .ok_or("Affine candidate index is invalid")?;
    let is_plane = affine.disposition() == orv_core::surface::field::AffineDisposition::Plane;
    if let Some(geometry) = quantize(affine, source.volume(), assembly.max_error_mm())? {
        return Ok((Some(InputGeometry::Affine(geometry)), false));
    }
    if let CandidateCellState::Candidate { mesh } = cell.state {
        let mesh = assembly
            .meshes()
            .get(usize::from(mesh))
            .ok_or("Candidate mesh index is invalid")?;
        return Ok((Some(InputGeometry::Mesh(mesh)), is_plane));
    }
    Ok((None, is_plane))
}
fn compatible(
    previous: Option<&Arc<GpuCandidateSurface>>,
    origin: [u32; 3],
    geometry: &InputGeometry<'_>,
) -> Option<Arc<GpuCandidateChunk>> {
    previous
        .and_then(|old| old.chunk(origin))
        .filter(|old| match (&old.geometry, geometry) {
            (ChunkGeometry::Affine(a), InputGeometry::Affine(b)) => a == b,
            (ChunkGeometry::Mesh(a), InputGeometry::Mesh(b)) => {
                Arc::ptr_eq(a, b)
                    || (same_triangle_layout(a, b)
                        && matches!((field_samples(a),field_samples(b)),(Ok(a),Ok(b)) if a == b))
            }
            _ => false,
        })
        .cloned()
}
fn same_triangle_layout(a: &ConformingSurfaceMesh, b: &ConformingSurfaceMesh) -> bool {
    a.vertices() == b.vertices()
        && a.triangles().len() == b.triangles().len()
        && a.triangles().iter().zip(b.triangles()).all(|(a, b)| {
            a.vertices == b.vertices
                && matches!(a.state, ConformingFacetState::WithinTolerance { .. })
                    == matches!(b.state, ConformingFacetState::WithinTolerance { .. })
        })
}
fn checked_origin(origin: [usize; 3], dimensions: [usize; 3]) -> Result<[u32; 3], &'static str> {
    let mut result = [0; 3];
    for i in 0..3 {
        if origin[i]
            .checked_add(1)
            .is_none_or(|end| end >= dimensions[i])
        {
            return Err("Candidate cell is outside its source");
        }
        result[i] =
            u32::try_from(origin[i]).map_err(|_| "Candidate origin cannot be represented")?;
    }
    Ok(result)
}
fn triangle_bytes(triangles: usize) -> Result<u64, &'static str> {
    u64::try_from(triangles)
        .ok()
        .and_then(|n| n.checked_mul(TRIANGLE_BYTES))
        .ok_or("Candidate geometry size overflow")
}
fn aligned(bytes: u64) -> Result<u64, &'static str> {
    bytes
        .checked_add(ALIGNMENT - 1)
        .map(|n| n / ALIGNMENT * ALIGNMENT)
        .ok_or("Candidate geometry size overflow")
}
fn inspect(
    mesh: &ConformingSurfaceMesh,
    cancelled: &impl Fn() -> bool,
) -> Result<(usize, usize), &'static str> {
    field_samples(mesh)?;
    let (mut represented, mut unresolved) = (0usize, 0usize);
    for (i, triangle) in mesh.triangles().iter().enumerate() {
        if i.is_multiple_of(256) && cancelled() {
            return Err("Cancelled");
        }
        if matches!(triangle.state, ConformingFacetState::WithinTolerance { .. }) {
            for id in triangle.vertices {
                let position = mesh
                    .vertices()
                    .get(id as usize)
                    .and_then(|v| v.position())
                    .ok_or("Candidate vertex is unavailable")?;
                if position
                    .into_iter()
                    .any(|v| v > ConformingSurfaceMesh::POSITION_DENOMINATOR)
                {
                    return Err("Candidate vertex is outside its source cell");
                }
            }
            represented += 1;
        } else {
            unresolved += 1;
        }
    }
    Ok((represented, unresolved))
}
fn field_samples(mesh: &ConformingSurfaceMesh) -> Result<[i16; 8], &'static str> {
    let chart = mesh.input().input().input().input().input().edges().chart();
    let mut values = [0i16; 8];
    for (out, sample) in values.iter_mut().zip(chart.graph().cell().corners()) {
        if !sample.source_valid {
            return Err("Candidate field includes an invalid source sample");
        }
        *out = sample
            .value
            .ok_or("Candidate field includes an unknown sample")?;
    }
    Ok(values)
}
fn packed_samples(values: [i16; 8]) -> [u32; 4] {
    std::array::from_fn(|i| {
        u32::from(values[2 * i] as u16) | (u32::from(values[2 * i + 1] as u16) << 16)
    })
}
fn pack_affine(
    buffer: &wgpu::Buffer,
    geometry: &AffineGeometry,
    origin: [u32; 3],
    triangles: usize,
    cancelled: &impl Fn() -> bool,
) -> Result<(), &'static str> {
    let mut mapped = buffer
        .get_mapped_range_mut(..)
        .map_err(|_| "Could not map affine geometry upload")?;
    if mapped.len() as u64 != triangle_bytes(triangles)? || triangles != geometry.triangle_count() {
        return Err("Affine mapping size differs from its geometry");
    }
    for (index, vertices) in geometry.triangles().enumerate() {
        if cancelled() {
            return Err("Cancelled");
        }
        let mut words = [0u32; 16];
        words[..3].copy_from_slice(&origin);
        for (vertex, position) in vertices.into_iter().enumerate() {
            words[3 + 3 * vertex..6 + 3 * vertex].copy_from_slice(&position);
        }
        words[12..].copy_from_slice(&packed_samples(geometry.samples));
        let encoded = words.map(u32::to_le_bytes);
        let start = index * TRIANGLE_BYTES as usize;
        mapped
            .slice(start..start + TRIANGLE_BYTES as usize)
            .copy_from_slice(encoded.as_flattened());
    }
    Ok(())
}
fn pack(
    buffer: &wgpu::Buffer,
    mesh: &ConformingSurfaceMesh,
    origin: [u32; 3],
    triangles: usize,
    cancelled: &impl Fn() -> bool,
) -> Result<(), &'static str> {
    let mut mapped = buffer
        .get_mapped_range_mut(..)
        .map_err(|_| "Could not map candidate geometry upload")?;
    if mapped.len() as u64 != triangle_bytes(triangles)? {
        return Err("Candidate mapping size differs from its geometry");
    }
    let mut index = 0usize;
    let samples = packed_samples(field_samples(mesh)?);
    for (i, triangle) in mesh.triangles().iter().enumerate() {
        if i.is_multiple_of(256) && cancelled() {
            return Err("Cancelled");
        }
        if !matches!(triangle.state, ConformingFacetState::WithinTolerance { .. }) {
            continue;
        }
        let mut words = [0u32; 16];
        words[..3].copy_from_slice(&origin);
        for (vertex, id) in triangle.vertices.into_iter().enumerate() {
            let position = mesh
                .vertices()
                .get(id as usize)
                .and_then(|v| v.position())
                .ok_or("Candidate vertex is unavailable")?;
            words[3 + 3 * vertex..6 + 3 * vertex].copy_from_slice(&position);
        }
        words[12..].copy_from_slice(&samples);
        let encoded = words.map(u32::to_le_bytes);
        let start = index * TRIANGLE_BYTES as usize;
        mapped
            .slice(start..start + TRIANGLE_BYTES as usize)
            .copy_from_slice(encoded.as_flattened());
        index += 1;
    }
    if index != triangles {
        return Err("Candidate triangle count changed during upload");
    }
    Ok(())
}

#[cfg(all(test, feature = "gpu-tests"))]
mod tests;
