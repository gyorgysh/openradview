// SPDX-License-Identifier: GPL-3.0-or-later
//! Coordinated curve and straight-cut paths over one source-owned planar graph.
use super::*;
mod triangulate;
pub use triangulate::{
    AdaptiveFacetLimits, AdaptiveFacetMesh, AdaptiveFacetState, AdaptiveFacetTriangle,
    AdaptiveStopReason, AppliedFaceCell, AppliedFaceLimits, AppliedFacePair, AppliedFaceSpan,
    AppliedFaceTriangle, AppliedFaceVertex, AppliedTriangleState, BoundedSurfaceFacet,
    BoundedSurfaceVertex, CellAxisBounds, CellFaceReport, CellFaceState, CellSeamBatch,
    CellSeamLimits, CellSeamStop, ConformingFacetState, ConformingStopReason,
    ConformingSurfaceLimits, ConformingSurfaceMesh, ConformingTriangle, ConformingVertex,
    EnclosedSurfaceGraph, FacePartitionLimits, FacePartitionPoint, FacePartitionSpan,
    FacePlacement, FacePromotionLimits, FaceSpanCoverage, PhysicalConformingLimits, PlanarTriangle,
    PlanarTriangulationLimits, SharedFacePartition, SurfaceEnclosureLimits,
    SurfaceFacetCertificate, TriangulatedPlanarGraph,
};

const MAX_BYTES: u64 = 16 * 1024 * 1024;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlanarRefinementLimits {
    /// Curve refinement has its own independently admitted storage and limits.
    pub curves: RefinementLimits,
    /// Total chart and added straight-cut vertices.
    pub max_vertices: usize,
    /// Additional typed storage, excluding the chart curve refinement and input.
    pub max_bytes: u64,
}
impl Default for PlanarRefinementLimits {
    fn default() -> Self {
        Self {
            curves: RefinementLimits::default(),
            max_vertices: RefinementLimits::MAX_VERTICES,
            max_bytes: MAX_BYTES,
        }
    }
}
#[derive(Clone, Copy)]
struct Path {
    start: u32,
    len: u32,
}
/// Curve and straight-cut paths share one slice partition and source graph.
/// Boundary approximation is not an interior or physical-space error bound.
pub struct RefinedPlanarGraph {
    input: Arc<ProjectedPlanarGraph>,
    curves: Arc<RefinedJointSubdivision>,
    added: Vec<[ExactCoordinate; 2]>,
    indices: Vec<u32>,
    paths: Vec<Path>,
    bytes: u64,
    _reservation: Reservation,
}
impl RefinedPlanarGraph {
    pub(in crate::surface) fn rebind_candidate(
        &self,
        proof: &crate::surface::field::CellReuseProof<'_>,
        budget: &Arc<MemoryBudget>,
        cancel: &impl Fn() -> bool,
    ) -> Result<Self, SurfaceError> {
        if cancel() {
            return Err(SurfaceError::Cancelled);
        }
        let input = Arc::new(self.input.rebind_candidate(proof, budget, cancel)?);
        let reservation = budget.reserve(self.bytes).ok_or(SurfaceError::Memory)?;
        // The subdivision contains only numeric fields and curves; it has no mask owner.
        Ok(Self {
            input,
            curves: self.curves.clone(),
            added: crate::surface::copy_retained(&self.added, cancel)?,
            indices: crate::surface::copy_retained(&self.indices, cancel)?,
            paths: crate::surface::copy_retained(&self.paths, cancel)?,
            bytes: self.bytes,
            _reservation: reservation,
        })
    }
    pub fn input(&self) -> &Arc<ProjectedPlanarGraph> {
        &self.input
    }
    pub fn curves(&self) -> &Arc<RefinedJointSubdivision> {
        &self.curves
    }
    pub fn vertex_count(&self) -> usize {
        self.curves.vertices().len() + self.added.len()
    }
    pub fn vertex(&self, id: u32) -> Option<&[ExactCoordinate; 2]> {
        let id = id as usize;
        if id < self.curves.vertices().len() {
            self.curves.vertices().get(id)
        } else {
            self.added.get(id - self.curves.vertices().len())
        }
    }
    /// Vertices follow the corresponding planar edge's stored node order.
    pub fn edge_vertices(&self, edge: u32) -> Option<&[u32]> {
        let p = self.paths.get(edge as usize)?;
        self.indices
            .get(p.start as usize..(p.start as usize).checked_add(p.len as usize)?)
    }
    pub fn referenced_bytes(&self) -> u64 {
        self.bytes
    }
    /// Walk a region without repeating the closing endpoint. Bridges and
    /// contacts retain repeated visits; no topology is inferred from floats.
    pub fn region_boundary(
        &self,
        region: u32,
        cancel: impl Fn() -> bool,
        mut emit: impl FnMut(u32) -> Result<(), SurfaceError>,
    ) -> Result<(), SurfaceError> {
        check(&cancel)?;
        let r = self
            .input
            .regions
            .get(region as usize)
            .ok_or(SurfaceError::Geometry)?;
        let mut at = r.first;
        for _ in 0..r.edge_count {
            check(&cancel)?;
            let h = self
                .input
                .halves
                .get(at as usize)
                .ok_or(SurfaceError::Geometry)?;
            let e = self
                .input
                .edges
                .get(h.edge as usize)
                .ok_or(SurfaceError::Geometry)?;
            let path = self.edge_vertices(h.edge).ok_or(SurfaceError::Geometry)?;
            if h.region != Some(region)
                || path.len() < 2
                || path[0] != u32::from(e.nodes[0])
                || path[path.len() - 1] != u32::from(e.nodes[1])
            {
                return Err(SurfaceError::Geometry);
            }
            let forward = if [h.from, h.to] == e.nodes {
                true
            } else if [h.to, h.from] == e.nodes {
                false
            } else {
                return Err(SurfaceError::Geometry);
            };
            for i in 0..path.len() - 1 {
                check(&cancel)?;
                let index = if forward { i } else { path.len() - 1 - i };
                let vertex = path[index];
                if self.vertex(vertex).is_none() {
                    return Err(SurfaceError::Geometry);
                }
                emit(vertex)?;
            }
            at = h.next;
        }
        if at != r.first {
            return Err(SurfaceError::Geometry);
        }
        check(&cancel)
    }
}
fn bytes(vertices: usize, indices: usize, edges: usize) -> Result<u64, SurfaceError> {
    size(
        &[
            (vertices, std::mem::size_of::<[ExactCoordinate; 2]>()),
            (indices, std::mem::size_of::<u32>()),
            (edges, std::mem::size_of::<Path>()),
        ],
        std::mem::size_of::<RefinedPlanarGraph>()
            + crate::surface::shared_header_bytes::<RefinedPlanarGraph>(),
    )
}
fn node(chart: &JointSubdivision, id: u8) -> Result<[ExactCoordinate; 2], SurfaceError> {
    chart
        .node(id)
        .map(|n| *n.coordinates())
        .ok_or(SurfaceError::Geometry)
}
fn between(
    value: ExactCoordinate,
    a: ExactCoordinate,
    b: ExactCoordinate,
) -> Result<bool, SurfaceError> {
    let (lo, hi) = if cmp(a, b)? == Ordering::Greater {
        (b, a)
    } else {
        (a, b)
    };
    Ok(cmp(lo, value)? == Ordering::Less && cmp(value, hi)? == Ordering::Less)
}
fn straight_inner(
    chart: &JointSubdivision,
    curves: &RefinedJointSubdivision,
    edge: PlanarEdge,
    cancel: &impl Fn() -> bool,
    mut emit: impl FnMut([ExactCoordinate; 2]) -> Result<(), SurfaceError>,
) -> Result<usize, SurfaceError> {
    let a = node(chart, edge.nodes[0])?;
    let b = node(chart, edge.nodes[1])?;
    if cmp(a[0], b[0])? == Ordering::Equal {
        if cmp(a[1], b[1])? == Ordering::Equal {
            return Err(SurfaceError::Geometry);
        }
        return Ok(0);
    }
    if cmp(a[1], b[1])? != Ordering::Equal {
        return Err(SurfaceError::Geometry);
    }
    let reverse = cmp(a[0], b[0])? == Ordering::Greater;
    let cuts = curves.slice_coordinates();
    let mut count = 0usize;
    for i in 0..cuts.len() {
        check(cancel)?;
        let u = cuts[if reverse { cuts.len() - 1 - i } else { i }];
        if !between(u, a[0], b[0])? {
            continue;
        }
        let point = [u, a[1]];
        // An unsplit cut cannot cross either exact field curve or a square
        // boundary. Such a contact should already be a chart node.
        if cmp(point[1], endpoint(false)?)? == Ordering::Equal
            || cmp(point[1], endpoint(true)?)? == Ordering::Equal
        {
            return Err(SurfaceError::Geometry);
        }
        for field in chart.fields() {
            if field.sign_at(point).map_err(error)? == 0 {
                return Err(SurfaceError::Geometry);
            }
        }
        emit(point)?;
        count = count.checked_add(1).ok_or(SurfaceError::Limit)?;
    }
    Ok(count)
}
impl ProjectedPlanarGraph {
    /// Coordinate all boundary paths with the chart refiner's shared slices.
    /// Unsupported intersections, limits or cancellation yield no partial graph.
    pub fn refine(
        self: &Arc<Self>,
        limits: PlanarRefinementLimits,
        cancel: impl Fn() -> bool,
    ) -> Result<RefinedPlanarGraph, SurfaceError> {
        self.refine_with_budget(limits, MemoryBudget::volumes(), cancel)
    }
    pub(in crate::surface) fn refine_with_budget(
        self: &Arc<Self>,
        limits: PlanarRefinementLimits,
        budget: Arc<MemoryBudget>,
        cancel: impl Fn() -> bool,
    ) -> Result<RefinedPlanarGraph, SurfaceError> {
        check(&cancel)?;
        if limits.max_vertices > RefinementLimits::MAX_VERTICES || limits.max_bytes > MAX_BYTES {
            return Err(SurfaceError::Limit);
        }
        let chart = self.input.edges.chart.subdivision();
        let curves = Arc::new(chart.refine_with_budget(limits.curves, budget.clone(), &cancel)?);
        let original = curves.vertices().len();
        if original > limits.max_vertices {
            return Err(SurfaceError::Limit);
        }
        let mut additions = 0usize;
        let mut index_count = 0usize;
        for e in self.edges.iter().copied() {
            check(&cancel)?;
            let len = if let Some(id) = e.chart_edge {
                curves
                    .edge_vertices(id)
                    .ok_or(SurfaceError::Geometry)?
                    .len()
            } else {
                let inner = straight_inner(chart, &curves, e, &cancel, |_| Ok(()))?;
                additions = additions.checked_add(inner).ok_or(SurfaceError::Limit)?;
                2usize.checked_add(inner).ok_or(SurfaceError::Limit)?
            };
            index_count = index_count.checked_add(len).ok_or(SurfaceError::Limit)?;
            if original
                .checked_add(additions)
                .is_none_or(|v| v > limits.max_vertices)
                || bytes(additions, index_count, self.edges.len())? > limits.max_bytes
            {
                return Err(SurfaceError::Limit);
            }
        }
        let retained = bytes(additions, index_count, self.edges.len())?;
        let reservation = budget.reserve(retained).ok_or(SurfaceError::Memory)?;
        let mut added = exact(additions)?;
        let mut indices = exact(index_count)?;
        let mut paths = exact(self.edges.len())?;
        for e in self.edges.iter().copied() {
            check(&cancel)?;
            let start = indices.len();
            if let Some(id) = e.chart_edge {
                let path = curves.edge_vertices(id).ok_or(SurfaceError::Geometry)?;
                if path.first() != Some(&u32::from(e.nodes[0]))
                    || path.last() != Some(&u32::from(e.nodes[1]))
                {
                    return Err(SurfaceError::Geometry);
                }
                if indices
                    .len()
                    .checked_add(path.len())
                    .is_none_or(|n| n > index_count)
                {
                    return Err(SurfaceError::Geometry);
                }
                indices.extend_from_slice(path);
            } else {
                if indices.len() >= index_count {
                    return Err(SurfaceError::Geometry);
                }
                indices.push(u32::from(e.nodes[0]));
                straight_inner(chart, &curves, e, &cancel, |point| {
                    if added.len() >= additions || indices.len() >= index_count {
                        return Err(SurfaceError::Geometry);
                    }
                    let id =
                        u32::try_from(original + added.len()).map_err(|_| SurfaceError::Limit)?;
                    added.push(point);
                    indices.push(id);
                    Ok(())
                })?;
                if indices.len() >= index_count {
                    return Err(SurfaceError::Geometry);
                }
                indices.push(u32::from(e.nodes[1]));
            }
            let len = indices.len() - start;
            paths.push(Path {
                start: u32::try_from(start).map_err(|_| SurfaceError::Limit)?,
                len: u32::try_from(len).map_err(|_| SurfaceError::Limit)?,
            });
        }
        check(&cancel)?;
        if added.len() != additions || indices.len() != index_count {
            return Err(SurfaceError::Geometry);
        }
        Ok(RefinedPlanarGraph {
            input: Arc::clone(self),
            curves,
            added,
            indices,
            paths,
            bytes: retained,
            _reservation: reservation,
        })
    }
}
