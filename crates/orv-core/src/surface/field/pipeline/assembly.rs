// SPDX-License-Identifier: GPL-3.0-or-later
//! Bounded cross-box coordination of independent source-cell candidates.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CandidateAssemblyLimits {
    pub max_cells: usize,
    /// Retained directory only; input meshes, seam generations and scratch are separate.
    pub max_directory_bytes: u64,
    pub seams: CellSeamLimits,
}

/// Only explicitly supplied cells and candidate faces are represented. Exterior
/// boundaries, unlisted cells and noncandidate strata remain unproved.
pub struct CandidateAssembly {
    field: MaskField,
    cells: Vec<CandidateCell>,
    base_meshes: Vec<Arc<ConformingSurfaceMesh>>,
    meshes: Meshes,
    affine: Vec<AffineCell>,
    affine_faces: Vec<AffineFace>,
    max_error_mm: f64,
    bytes: u64,
    _reservation: Reservation,
}

impl CandidateAssembly {
    pub fn field(&self) -> &MaskField {
        &self.field
    }
    /// Source-cell order is z, then y, then x, independent of input box order.
    pub fn cells(&self) -> &[CandidateCell] {
        &self.cells
    }
    pub fn meshes(&self) -> &[Arc<ConformingSurfaceMesh>] {
        match &self.meshes {
            Meshes::Empty => &[],
            Meshes::Coordinated(batch) => batch.cells(),
            Meshes::Uncoordinated { .. } => &self.base_meshes,
        }
    }
    /// Face cell indices refer to `meshes`, not the full cell directory.
    pub fn faces(&self) -> &[CellFaceReport] {
        match &self.meshes {
            Meshes::Coordinated(batch) => batch.faces(),
            _ => &[],
        }
    }
    /// One source-bound affine classification per cell, in `cells()` order.
    pub fn affine_cells(&self) -> &[AffineCell] {
        &self.affine
    }
    /// Adjacent listed pairs involving an affine plane. Unlisted neighbors,
    /// mixed-cell seams and finite-grid closure remain unresolved.
    pub fn affine_faces(&self) -> &[AffineFace] {
        &self.affine_faces
    }
    pub fn max_error_mm(&self) -> f64 {
        self.max_error_mm
    }
    /// Coordination of the general candidate meshes only. Affine traces are
    /// reported separately by `affine_faces()`.
    pub fn coordination(&self) -> GridCoordination {
        match &self.meshes {
            Meshes::Empty => GridCoordination::NoCandidates,
            Meshes::Coordinated(_) => GridCoordination::Coordinated,
            Meshes::Uncoordinated { reason } => GridCoordination::Unresolved { reason: *reason },
        }
    }
    pub fn referenced_bytes(&self) -> u64 {
        self.bytes
    }

    /// Worker-only assembly of disjoint boxes from the same source revision and
    /// build settings. Each box contributes its independent, pre-coordination
    /// candidates. The aggregate re-coordinates all listed candidate faces.
    pub fn build(
        grids: &[&CandidateGrid],
        limits: CandidateAssemblyLimits,
        cancel: impl Fn() -> bool,
    ) -> Result<Self, SurfaceError> {
        Self::build_with_budget(grids, limits, MemoryBudget::volumes(), cancel)
    }

    pub(in crate::surface) fn build_with_budget(
        grids: &[&CandidateGrid],
        limits: CandidateAssemblyLimits,
        budget: Arc<MemoryBudget>,
        cancel: impl Fn() -> bool,
    ) -> Result<Self, SurfaceError> {
        if cancel() {
            return Err(SurfaceError::Cancelled);
        }
        if grids.is_empty()
            || grids.len() > MAX_CELLS
            || limits.max_cells > MAX_CELLS
            || limits.max_directory_bytes > MAX_DIRECTORY
        {
            return Err(SurfaceError::Limit);
        }
        let first = grids[0];
        let mut count = 0usize;
        for grid in grids {
            if cancel() {
                return Err(SurfaceError::Cancelled);
            }
            if grid.field.key() != first.field.key()
                || grid.field.revision() != first.field.revision()
                || !Arc::ptr_eq(grid.field.source(), first.field.source())
                || !Arc::ptr_eq(&grid.budget, &budget)
                || grid.axis != first.axis
                || grid.limits != first.limits
            {
                return Err(SurfaceError::Geometry);
            }
            count = count
                .checked_add(grid.cells.len())
                .ok_or(SurfaceError::Limit)?;
            if count > limits.max_cells {
                return Err(SurfaceError::Limit);
            }
        }
        let bytes = count
            .checked_mul(
                std::mem::size_of::<CandidateCell>()
                    + std::mem::size_of::<Arc<ConformingSurfaceMesh>>()
                    + std::mem::size_of::<AffineCell>()
                    + 3 * std::mem::size_of::<AffineFace>(),
            )
            .and_then(|n| n.checked_add(std::mem::size_of::<Self>()))
            .and_then(|n| u64::try_from(n).ok())
            .ok_or(SurfaceError::Limit)?;
        if bytes > limits.max_directory_bytes {
            return Err(SurfaceError::Limit);
        }
        let scratch_bytes = count
            .checked_mul(std::mem::size_of::<Record>())
            .and_then(|n| u64::try_from(n).ok())
            .ok_or(SurfaceError::Limit)?;
        let reservation = budget.reserve(bytes).ok_or(SurfaceError::Memory)?;
        let _scratch = budget.reserve(scratch_bytes).ok_or(SurfaceError::Memory)?;
        let mut records = exact(count)?;
        for grid in grids {
            for cell in &grid.cells {
                if cancel() {
                    return Err(SurfaceError::Cancelled);
                }
                let mesh = match cell.state {
                    CandidateCellState::Candidate { mesh } => Some(Arc::clone(
                        grid.base_meshes
                            .get(usize::from(mesh))
                            .ok_or(SurfaceError::Geometry)?,
                    )),
                    _ => None,
                };
                records.push(Record { cell: *cell, mesh });
            }
        }
        records.sort_unstable_by_key(|record| {
            let [x, y, z] = record.cell.origin;
            (z, y, x)
        });
        if records
            .windows(2)
            .any(|pair| pair[0].cell.origin == pair[1].cell.origin)
        {
            return Err(SurfaceError::Geometry);
        }
        let mut cells = exact(count)?;
        let mut base_meshes = exact(count)?;
        for record in records {
            if cancel() {
                return Err(SurfaceError::Cancelled);
            }
            let state = if let Some(mesh) = record.mesh {
                let id = u16::try_from(base_meshes.len()).map_err(|_| SurfaceError::Limit)?;
                base_meshes.push(mesh);
                CandidateCellState::Candidate { mesh: id }
            } else {
                record.cell.state
            };
            cells.push(CandidateCell {
                origin: record.cell.origin,
                state,
            });
        }
        let meshes = if base_meshes.is_empty() {
            Meshes::Empty
        } else {
            match CellSeamBatch::coordinate_with_budget(&base_meshes, limits.seams, budget, &cancel)
            {
                Ok(batch) => Meshes::Coordinated(batch),
                Err(SurfaceError::Cancelled) => return Err(SurfaceError::Cancelled),
                Err(SurfaceError::Memory) => return Err(SurfaceError::Memory),
                Err(SurfaceError::Selection) => return Err(SurfaceError::Selection),
                Err(reason) => Meshes::Uncoordinated { reason },
            }
        };
        let mut affine = exact(count)?;
        for cell in &cells {
            if cancel() {
                return Err(SurfaceError::Cancelled);
            }
            affine.push(first.field.affine_cell(cell.origin, &cancel).map_err(
                |reason| match reason {
                    FieldError::Cancelled => SurfaceError::Cancelled,
                    FieldError::Selection => SurfaceError::Selection,
                    _ => SurfaceError::Geometry,
                },
            )?);
        }
        let mut face_count = 0usize;
        for (index, cell) in cells.iter().enumerate() {
            if cancel() {
                return Err(SurfaceError::Cancelled);
            }
            for axis in 0..3 {
                if let Some(other) = neighbor(&cells, cell.origin, axis)
                    && (affine[index].disposition() == AffineDisposition::Plane
                        || affine[other].disposition() == AffineDisposition::Plane)
                {
                    face_count += 1;
                }
            }
        }
        let mut affine_faces = exact(face_count)?;
        for (index, cell) in cells.iter().enumerate() {
            if cancel() {
                return Err(SurfaceError::Cancelled);
            }
            for axis in 0..3 {
                if let Some(other) = neighbor(&cells, cell.origin, axis)
                    && (affine[index].disposition() == AffineDisposition::Plane
                        || affine[other].disposition() == AffineDisposition::Plane)
                {
                    affine_faces.push(
                        affine[index]
                            .shared_face(&affine[other], axis, &cancel)
                            .map_err(|reason| match reason {
                                FieldError::Cancelled => SurfaceError::Cancelled,
                                FieldError::Selection => SurfaceError::Selection,
                                _ => SurfaceError::Geometry,
                            })?,
                    );
                }
            }
        }
        if cancel() {
            return Err(SurfaceError::Cancelled);
        }
        Ok(Self {
            field: first.field.clone(),
            cells,
            base_meshes,
            meshes,
            affine,
            affine_faces,
            max_error_mm: first.limits.surface.max_error_mm,
            bytes,
            _reservation: reservation,
        })
    }
}

fn neighbor(cells: &[CandidateCell], mut origin: [usize; 3], axis: usize) -> Option<usize> {
    origin[axis] = origin[axis].checked_add(1)?;
    cells
        .binary_search_by_key(&(origin[2], origin[1], origin[0]), |cell| {
            (cell.origin[2], cell.origin[1], cell.origin[0])
        })
        .ok()
}

struct Record {
    cell: CandidateCell,
    mesh: Option<Arc<ConformingSurfaceMesh>>,
}

fn exact<T>(count: usize) -> Result<Vec<T>, SurfaceError> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(count)
        .map_err(|_| SurfaceError::Memory)?;
    if values.capacity() != count {
        return Err(SurfaceError::Memory);
    }
    Ok(values)
}
