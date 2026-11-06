// SPDX-License-Identifier: GPL-3.0-or-later
//! Source-bound candidate construction over an explicitly bounded cell box.
use super::*;
use crate::{
    resources::{MemoryBudget, Reservation},
    surface::SurfaceError,
};

const MAX_CELLS: usize = 256;
const MAX_DIRECTORY: u64 = 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CandidateGridLimits {
    pub max_cells: usize,
    /// Retained directory and temporary candidate pointer array; intermediate
    /// source objects, seam generations and stage scratch are admitted separately.
    pub max_directory_bytes: u64,
    pub graph: CellGraphLimits,
    pub planar_refinement: PlanarRefinementLimits,
    pub triangulation: PlanarTriangulationLimits,
    pub enclosure: SurfaceEnclosureLimits,
    pub surface: PhysicalConformingLimits,
    pub seams: CellSeamLimits,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PipelineStage {
    Graph,
    Chart,
    Edges,
    Contacts,
    Split,
    Planar,
    PlanarRefinement,
    Triangulation,
    Enclosure,
    SurfaceRefinement,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CandidateCellState {
    /// Index into `CandidateGrid::meshes`; this is not complete surface coverage.
    Candidate { mesh: u16 },
    /// No-interior, unavailable and family cases retain their exact disposition.
    Graph(CellGraphState),
    Unresolved {
        stage: PipelineStage,
        reason: SurfaceError,
    },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CandidateCell {
    pub origin: [usize; 3],
    pub state: CandidateCellState,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GridCoordination {
    /// No candidate mesh was produced; this does not prove an empty surface.
    NoCandidates,
    /// Face coordination ran; each cell and face must still be inspected.
    Coordinated,
    Unresolved {
        reason: SurfaceError,
    },
}
enum Meshes {
    Empty,
    Coordinated(CellSeamBatch),
    Uncoordinated {
        meshes: Vec<Arc<ConformingSurfaceMesh>>,
        reason: SurfaceError,
    },
}
/// The listed source cells only. Missing strata, outer neighbors, end faces and
/// unproved candidates are not silently converted into a complete surface.
pub struct CandidateGrid {
    field: MaskField,
    origin: [usize; 3],
    size: [usize; 3],
    axis: u8,
    cells: Vec<CandidateCell>,
    meshes: Meshes,
    bytes: u64,
    _reservation: Reservation,
}
impl CandidateGrid {
    pub fn field(&self) -> &MaskField {
        &self.field
    }
    pub fn origin(&self) -> [usize; 3] {
        self.origin
    }
    pub fn size(&self) -> [usize; 3] {
        self.size
    }
    pub fn projection_axis(&self) -> u8 {
        self.axis
    }
    pub fn cells(&self) -> &[CandidateCell] {
        &self.cells
    }
    pub fn meshes(&self) -> &[Arc<ConformingSurfaceMesh>] {
        match &self.meshes {
            Meshes::Empty => &[],
            Meshes::Coordinated(batch) => batch.cells(),
            Meshes::Uncoordinated { meshes, .. } => meshes,
        }
    }
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
            Meshes::Uncoordinated { reason, .. } => {
                GridCoordination::Unresolved { reason: *reason }
            }
        }
    }
    /// Directory charge only; nested source objects and seam generations retain
    /// their own independent reservations.
    pub fn referenced_bytes(&self) -> u64 {
        self.bytes
    }
}
enum Built {
    Candidate(Arc<ConformingSurfaceMesh>),
    Graph(CellGraphState),
}
fn one(
    field: &MaskField,
    origin: [usize; 3],
    axis: usize,
    limits: CandidateGridLimits,
    budget: &Arc<MemoryBudget>,
    cancel: &impl Fn() -> bool,
) -> Result<Built, (PipelineStage, SurfaceError)> {
    macro_rules! stage {
        ($name:ident, $call:expr) => {
            $call.map_err(|e| (PipelineStage::$name, e))?
        };
    }
    let graph = Arc::new(stage!(
        Graph,
        field.cell_graph_with_budget(origin, limits.graph, budget.clone(), cancel)
    ));
    if *graph.state() != CellGraphState::Finite {
        return Ok(Built::Graph(*graph.state()));
    }
    let chart = Arc::new(stage!(
        Chart,
        graph.projection_chart_with_budget(axis, budget.clone(), cancel)
    ));
    let edges = Arc::new(stage!(
        Edges,
        chart.project_edges_with_budget(budget.clone(), cancel)
    ));
    let contacts = Arc::new(stage!(
        Contacts,
        edges.contacts_with_budget(budget.clone(), cancel)
    ));
    let split = Arc::new(stage!(
        Split,
        contacts.split_with_budget(budget.clone(), cancel)
    ));
    let planar = Arc::new(stage!(
        Planar,
        split.planar_graph_with_budget(budget.clone(), cancel)
    ));
    let refined = Arc::new(stage!(
        PlanarRefinement,
        planar.refine_with_budget(limits.planar_refinement, budget.clone(), cancel)
    ));
    let triangles = Arc::new(stage!(
        Triangulation,
        refined.triangulate_with_budget(limits.triangulation, budget.clone(), cancel)
    ));
    let enclosed = Arc::new(stage!(
        Enclosure,
        triangles.enclose_surface_with_budget(limits.enclosure, budget.clone(), cancel)
    ));
    let mesh = Arc::new(stage!(
        SurfaceRefinement,
        enclosed.refine_conforming_physical_with_budget(limits.surface, budget.clone(), cancel)
    ));
    Ok(Built::Candidate(mesh))
}
impl MaskField {
    /// Worker-only source-cell construction followed by shared-face coordination.
    /// Cell and face dispositions are explicit; the returned candidates are not a
    /// whole-grid surface or an authority for edits.
    pub fn candidate_grid(
        &self,
        origin: [usize; 3],
        size: [usize; 3],
        axis: usize,
        limits: CandidateGridLimits,
        cancel: impl Fn() -> bool,
    ) -> Result<CandidateGrid, SurfaceError> {
        self.candidate_grid_with_budget(origin, size, axis, limits, MemoryBudget::volumes(), cancel)
    }
    pub(in crate::surface) fn candidate_grid_with_budget(
        &self,
        origin: [usize; 3],
        size: [usize; 3],
        axis: usize,
        limits: CandidateGridLimits,
        budget: Arc<MemoryBudget>,
        cancel: impl Fn() -> bool,
    ) -> Result<CandidateGrid, SurfaceError> {
        if cancel() {
            return Err(SurfaceError::Cancelled);
        }
        if axis >= 3 || !limits.surface.max_error_mm.is_finite() || limits.surface.max_error_mm < 0.
        {
            return Err(SurfaceError::Geometry);
        }
        if size.contains(&0)
            || limits.max_cells > MAX_CELLS
            || limits.max_directory_bytes > MAX_DIRECTORY
        {
            return Err(SurfaceError::Limit);
        }
        let count = size
            .into_iter()
            .try_fold(1usize, |n, v| n.checked_mul(v))
            .ok_or(SurfaceError::Limit)?;
        if count > limits.max_cells {
            return Err(SurfaceError::Limit);
        }
        let dimensions = self.source().dimensions();
        for i in 0..3 {
            if origin[i]
                .checked_add(size[i])
                .is_none_or(|end| end >= dimensions[i])
            {
                return Err(SurfaceError::Geometry);
            }
        }
        let bytes = count
            .checked_mul(
                std::mem::size_of::<CandidateCell>()
                    + std::mem::size_of::<Arc<ConformingSurfaceMesh>>(),
            )
            .and_then(|n| n.checked_add(std::mem::size_of::<CandidateGrid>()))
            .and_then(|n| u64::try_from(n).ok())
            .ok_or(SurfaceError::Limit)?;
        if bytes > limits.max_directory_bytes {
            return Err(SurfaceError::Limit);
        }
        let reservation = budget.reserve(bytes).ok_or(SurfaceError::Memory)?;
        let mut cells = Vec::new();
        let mut meshes = Vec::new();
        cells
            .try_reserve_exact(count)
            .map_err(|_| SurfaceError::Memory)?;
        meshes
            .try_reserve_exact(count)
            .map_err(|_| SurfaceError::Memory)?;
        if cells.capacity() != count || meshes.capacity() != count {
            return Err(SurfaceError::Memory);
        }
        for index in 0..count {
            if cancel() {
                return Err(SurfaceError::Cancelled);
            }
            let offset = [
                index % size[0],
                index / size[0] % size[1],
                index / size[0] / size[1],
            ];
            let position = std::array::from_fn(|i| origin[i] + offset[i]);
            let state = match one(self, position, axis, limits, &budget, &cancel) {
                Ok(Built::Candidate(mesh)) => {
                    let id = u16::try_from(meshes.len()).map_err(|_| SurfaceError::Limit)?;
                    meshes.push(mesh);
                    CandidateCellState::Candidate { mesh: id }
                }
                Ok(Built::Graph(state)) => CandidateCellState::Graph(state),
                Err((_, SurfaceError::Cancelled)) => return Err(SurfaceError::Cancelled),
                Err((_, SurfaceError::Memory)) => return Err(SurfaceError::Memory),
                Err((_, SurfaceError::Selection)) => return Err(SurfaceError::Selection),
                Err((stage, reason)) => CandidateCellState::Unresolved { stage, reason },
            };
            cells.push(CandidateCell {
                origin: position,
                state,
            });
        }
        let meshes = if meshes.is_empty() {
            Meshes::Empty
        } else {
            match CellSeamBatch::coordinate_with_budget(&meshes, limits.seams, budget, &cancel) {
                Ok(batch) => Meshes::Coordinated(batch),
                Err(SurfaceError::Cancelled) => return Err(SurfaceError::Cancelled),
                Err(SurfaceError::Memory) => return Err(SurfaceError::Memory),
                Err(SurfaceError::Selection) => return Err(SurfaceError::Selection),
                Err(reason) => Meshes::Uncoordinated { meshes, reason },
            }
        };
        if cancel() {
            return Err(SurfaceError::Cancelled);
        }
        Ok(CandidateGrid {
            field: self.clone(),
            origin,
            size,
            axis: axis as u8,
            cells,
            meshes,
            bytes,
            _reservation: reservation,
        })
    }
}
