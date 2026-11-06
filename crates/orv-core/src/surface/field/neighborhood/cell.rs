// SPDX-License-Identifier: GPL-3.0-or-later
//! Source-owned finite cell contour/anchor graph; not an admitted triangle mesh.
use super::*;
use crate::{
    resources::{MemoryBudget, Reservation},
    surface::{SurfaceError, allocated},
};
use std::cmp::Ordering;
mod chart;
mod collect;
mod link;
mod refine;
pub use chart::{
    CellFiber, CellProjectionChart, ChartEdgeStep, PlanarEdge, PlanarHalfEdge, PlanarRegion,
    ProjectedCellEdges, ProjectedContact, ProjectedContactGeometry, ProjectedContactOwner,
    ProjectedContacts, ProjectedEdge, ProjectedEdgeKind, ProjectedPlanarGraph, SplitPath,
    SplitProjectedConstraints, SplitSegment, SplitStep,
};
pub use refine::{CellPathLimits, CellPathOrigin, CellPathVertex, RefinedCellGraph};

const NODES: usize = 6 * FaceArrangement::MAX_NODES + CriticalGraph::MAX_NODES;
const EDGES: usize =
    6 * FaceArrangement::MAX_SEGMENTS + CriticalGraph::MAX_EDGES + 18 * FaceArrangement::MAX_NODES;
const HALVES: usize = 2 * EDGES;
const MAX_BYTES: u64 = 4 * 1024 * 1024;
const _: () = assert!(NODES < u16::MAX as usize);
const _: () = assert!(HALVES < u16::MAX as usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellGraphLimits {
    pub max_nodes: usize,
    pub max_edges: usize,
    /// Complete retained typed storage; source/mask buffers and scratch excluded.
    pub max_bytes: u64,
}
impl Default for CellGraphLimits {
    fn default() -> Self {
        Self {
            max_nodes: NODES,
            max_edges: EDGES,
            max_bytes: MAX_BYTES,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CellGraphState {
    Unavailable {
        invalid_corners: u8,
        unknown_corners: u8,
    },
    /// The inclusive selected phase has no interface in the strict cell interior.
    /// Boundary strata and incident-cell/grid closure remain separate.
    NoInteriorInterface,
    Family {
        projection_axes: u8,
        line_axes: u8,
        whole_sections: [Option<WholeSection>; 6],
    },
    Finite,
}
#[derive(Clone, Copy, Debug)]
pub struct CellGraphNode {
    coordinates: [ExactCoordinate; 3],
    boundary: u8,
    faces: u8,
    original: Option<u8>,
    critical_axes: u8,
    gradient: [i8; 3],
    surface_degree: u8,
}
impl CellGraphNode {
    pub fn coordinates(&self) -> &[ExactCoordinate; 3] {
        &self.coordinates
    }
    pub fn boundary(&self) -> u8 {
        self.boundary
    }
    pub fn source_faces(&self) -> u8 {
        self.faces
    }
    pub fn original_node(&self) -> Option<u8> {
        self.original
    }
    pub fn critical_axes(&self) -> u8 {
        self.critical_axes
    }
    pub fn gradient_signs(&self) -> [i8; 3] {
        self.gradient
    }
    pub fn surface_degree(&self) -> u8 {
        self.surface_degree
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CellGraphGeometry {
    Axis { axis: u8 },
    Face { face: u8 },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellGraphEdge {
    pub nodes: [u16; 2],
    pub geometry: CellGraphGeometry,
    /// Local face bits2*axis+upper; original anchor-line bits use the finite graph.
    pub faces: u8,
    pub anchors: u8,
    /// Left-side eligibility in canonical node order, then reverse order.
    pub eligible: [bool; 2],
    pub first_half: Option<u16>,
    face_forward: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellGraphHalfEdge {
    pub edge: u16,
    pub from: u16,
    pub to: u16,
    pub twin: u16,
    pub next: u16,
    pub eligible: bool,
    pub region: Option<u16>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellGraphRegion {
    pub first: u16,
    pub edge_count: u16,
}
pub struct SourceCellGraph {
    field: MaskField,
    cell: FieldCell,
    factorization: Option<CellFactorization>,
    state: CellGraphState,
    nodes: Vec<CellGraphNode>,
    edges: Vec<CellGraphEdge>,
    halves: Vec<CellGraphHalfEdge>,
    regions: Vec<CellGraphRegion>,
    bytes: u64,
    _metadata: Reservation,
    _storage: Option<Reservation>,
}
impl SourceCellGraph {
    pub fn field(&self) -> &MaskField {
        &self.field
    }
    pub fn cell(&self) -> &FieldCell {
        &self.cell
    }
    pub fn state(&self) -> &CellGraphState {
        &self.state
    }
    pub fn factorization(&self) -> Option<CellFactorization> {
        self.factorization
    }
    pub fn phase(&self) -> Option<CellPhase> {
        self.cell.interpolant().map(Trilinear::phase)
    }
    pub fn zero_boundary(&self) -> Option<ZeroBoundary> {
        self.cell.interpolant().map(Trilinear::zero_boundary)
    }
    pub fn nodes(&self) -> &[CellGraphNode] {
        &self.nodes
    }
    pub fn edges(&self) -> &[CellGraphEdge] {
        &self.edges
    }
    pub fn half_edges(&self) -> &[CellGraphHalfEdge] {
        &self.halves
    }
    pub fn regions(&self) -> &[CellGraphRegion] {
        &self.regions
    }
    /// Retained typed storage. Shared source/mask buffers and scratch are excluded.
    pub fn referenced_bytes(&self) -> u64 {
        self.bytes
    }
}
struct Scratch {
    nodes: Vec<CellGraphNode>,
    edges: Vec<CellGraphEdge>,
    halves: Vec<CellGraphHalfEdge>,
    regions: Vec<CellGraphRegion>,
}
fn check(cancel: &impl Fn() -> bool) -> Result<(), SurfaceError> {
    if cancel() {
        Err(SurfaceError::Cancelled)
    } else {
        Ok(())
    }
}
fn error(e: FieldError) -> SurfaceError {
    match e {
        FieldError::Cancelled => SurfaceError::Cancelled,
        FieldError::Capacity => SurfaceError::Limit,
        FieldError::Selection => SurfaceError::Selection,
        _ => SurfaceError::Geometry,
    }
}
fn endpoint(upper: bool) -> Result<ExactCoordinate, SurfaceError> {
    ExactCoordinate::from_rational(Rational {
        numerator: i64::from(upper),
        denominator: 1,
    })
    .map_err(error)?
    .ok_or(SurfaceError::Geometry)
}
fn array_bytes(counts: [usize; 4]) -> Result<u64, SurfaceError> {
    counts
        .into_iter()
        .zip([
            std::mem::size_of::<CellGraphNode>(),
            std::mem::size_of::<CellGraphEdge>(),
            std::mem::size_of::<CellGraphHalfEdge>(),
            std::mem::size_of::<CellGraphRegion>(),
        ])
        .try_fold(0usize, |sum, (n, size)| {
            n.checked_mul(size)
                .and_then(|v| sum.checked_add(v))
                .ok_or(SurfaceError::Limit)
        })
        .and_then(|v| u64::try_from(v).map_err(|_| SurfaceError::Limit))
}
impl MaskField {
    /// Worker-side original-source graph. Finite regions are graph cycles only;
    /// general patch approximation, families and finite-grid closure are separate.
    pub fn cell_graph(
        &self,
        lower: [usize; 3],
        limits: CellGraphLimits,
        cancel: impl Fn() -> bool,
    ) -> Result<SourceCellGraph, SurfaceError> {
        self.cell_graph_with_budget(lower, limits, MemoryBudget::volumes(), cancel)
    }
    pub(in crate::surface) fn cell_graph_with_budget(
        &self,
        lower: [usize; 3],
        limits: CellGraphLimits,
        budget: Arc<MemoryBudget>,
        cancel: impl Fn() -> bool,
    ) -> Result<SourceCellGraph, SurfaceError> {
        check(&cancel)?;
        if limits.max_nodes > NODES || limits.max_edges > EDGES || limits.max_bytes > MAX_BYTES {
            return Err(SurfaceError::Limit);
        }
        let cell = self.cell(lower).map_err(error)?;
        let metadata = std::mem::size_of::<SourceCellGraph>() as u64;
        if metadata > limits.max_bytes {
            return Err(SurfaceError::Limit);
        }
        let reservation = budget.reserve(metadata).ok_or(SurfaceError::Memory)?;
        let mut result = SourceCellGraph {
            field: self.clone(),
            cell,
            factorization: None,
            state: CellGraphState::Unavailable {
                invalid_corners: 0,
                unknown_corners: 0,
            },
            nodes: Vec::new(),
            edges: Vec::new(),
            halves: Vec::new(),
            regions: Vec::new(),
            bytes: metadata,
            _metadata: reservation,
            _storage: None,
        };
        let Some(field) = result.cell.interpolant() else {
            let mut invalid_corners = 0;
            let mut unknown_corners = 0;
            for (i, s) in result.cell.corners().iter().enumerate() {
                invalid_corners |= u8::from(!s.source_valid) << i;
                unknown_corners |= u8::from(s.value.is_none()) << i;
            }
            result.state = CellGraphState::Unavailable {
                invalid_corners,
                unknown_corners,
            };
            check(&cancel)?;
            return Ok(result);
        };
        let prep = budget
            .reserve(
                (std::mem::size_of::<CriticalGraph>() + std::mem::size_of::<CellFactorization>())
                    as u64,
            )
            .ok_or(SurfaceError::Memory)?;
        let factors = field.factorization(&cancel).map_err(error)?;
        result.factorization = Some(factors);
        if field.phase() != CellPhase::Crossing {
            result.state = CellGraphState::NoInteriorInterface;
            check(&cancel)?;
            return Ok(result);
        }
        let graph = field.critical_graph(&cancel).map_err(error)?;
        if factors.residual().kind() != ResidualKind::Trilinear {
            let mut sections = [None; 6];
            for (i, section) in graph.whole_sections().enumerate() {
                *sections.get_mut(i).ok_or(SurfaceError::Geometry)? = Some(*section);
            }
            result.state = CellGraphState::Family {
                projection_axes: graph.unresolved_projection_axes(),
                line_axes: graph.unresolved_line_axes(),
                whole_sections: sections,
            };
            check(&cancel)?;
            return Ok(result);
        }
        if graph.requires_family_resolution() {
            return Err(SurfaceError::Geometry);
        }
        let scratch_arrays = array_bytes([
            limits.max_nodes,
            limits.max_edges,
            2 * limits.max_edges,
            2 * limits.max_edges,
        ])?;
        let scratch_metadata = (std::mem::size_of::<Scratch>()
            + std::mem::size_of::<SourceFace>()
            + std::mem::size_of::<[u16; NODES]>()
            + std::mem::size_of::<[u16; FaceArrangement::MAX_NODES]>()
            + std::mem::size_of::<AnchorStar>()
            + std::mem::size_of::<[[u16; 12]; 2]>()
            + std::mem::size_of::<[usize; 2]>()
            + std::mem::size_of::<Option<TangentRay>>()) as u64;
        let scratch_bytes = scratch_arrays
            .checked_add(scratch_metadata)
            .ok_or(SurfaceError::Limit)?;
        let scratch_reservation = budget.reserve(scratch_bytes).ok_or(SurfaceError::Memory)?;
        let mut scratch = Scratch {
            nodes: allocated(limits.max_nodes)?,
            edges: allocated(limits.max_edges)?,
            halves: allocated(2 * limits.max_edges)?,
            regions: allocated(2 * limits.max_edges)?,
        };
        scratch.collect(self, &result.cell, &graph, limits, &cancel)?;
        scratch.link(field, &graph, &cancel)?;
        let storage = array_bytes([
            scratch.nodes.len(),
            scratch.edges.len(),
            scratch.halves.len(),
            scratch.regions.len(),
        ])?;
        result.bytes = metadata.checked_add(storage).ok_or(SurfaceError::Limit)?;
        if result.bytes > limits.max_bytes {
            return Err(SurfaceError::Limit);
        }
        result._storage = Some(budget.reserve(storage).ok_or(SurfaceError::Memory)?);
        result.nodes = allocated(scratch.nodes.len())?;
        result.edges = allocated(scratch.edges.len())?;
        result.halves = allocated(scratch.halves.len())?;
        result.regions = allocated(scratch.regions.len())?;
        for v in &scratch.nodes {
            check(&cancel)?;
            result.nodes.push(*v);
        }
        for v in &scratch.edges {
            check(&cancel)?;
            result.edges.push(*v);
        }
        for v in &scratch.halves {
            check(&cancel)?;
            result.halves.push(*v);
        }
        for v in &scratch.regions {
            check(&cancel)?;
            result.regions.push(*v);
        }
        drop(scratch);
        drop(scratch_reservation);
        drop(prep);
        result.state = CellGraphState::Finite;
        check(&cancel)?;
        Ok(result)
    }
}
