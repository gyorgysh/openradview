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
                    + std::mem::size_of::<Arc<ConformingSurfaceMesh>>(),
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
        if cancel() {
            return Err(SurfaceError::Cancelled);
        }
        Ok(Self {
            field: first.field.clone(),
            cells,
            base_meshes,
            meshes,
            bytes,
            _reservation: reservation,
        })
    }
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
