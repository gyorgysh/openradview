// SPDX-License-Identifier: GPL-3.0-or-later
//! Exact projected source-edge directory and end-face chart links.
use super::*;
mod contacts;
pub use contacts::{
    AdaptiveFacetLimits, AdaptiveFacetMesh, AdaptiveFacetState, AdaptiveFacetTriangle,
    AdaptiveStopReason, AppliedFaceCell, AppliedFaceLimits, AppliedFacePair, AppliedFaceSpan,
    AppliedFaceTriangle, AppliedFaceVertex, AppliedTriangleState, BoundedSurfaceFacet,
    BoundedSurfaceVertex, CellAxisBounds, CellFaceReport, CellFaceState, CellSeamBatch,
    CellSeamLimits, CellSeamStop, ConformingFacetState, ConformingStopReason,
    ConformingSurfaceLimits, ConformingSurfaceMesh, ConformingTriangle, ConformingVertex,
    EnclosedSurfaceGraph, FacePartitionLimits, FacePartitionPoint, FacePartitionSpan,
    FacePlacement, FacePromotionLimits, FaceSpanCoverage, PhysicalConformingLimits, PlanarEdge,
    PlanarHalfEdge, PlanarRefinementLimits, PlanarRegion, PlanarTriangle,
    PlanarTriangulationLimits, ProjectedContact, ProjectedContactGeometry, ProjectedContactOwner,
    ProjectedContacts, ProjectedPlanarGraph, RefinedPlanarGraph, SharedFacePartition, SplitPath,
    SplitProjectedConstraints, SplitSegment, SplitStep, SurfaceEnclosureLimits,
    SurfaceFacetCertificate, TriangulatedPlanarGraph,
};

const MAX_LINKS: usize = 262_144;
const MAX_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProjectedEdgeKind {
    /// A whole source edge projects to one point; both heights remain in the graph.
    Collapsed,
    /// Straight interval. Interior cuts still need intersection and arrangement.
    Segment,
    /// A source end-face curve already represented by the chart graph.
    EndFace { upper: bool },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProjectedEdge {
    pub source_nodes: [u16; 2],
    pub kind: ProjectedEdgeKind,
    start: u32,
    len: u32,
}
/// A directed step in chart-graph order along a source edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChartEdgeStep {
    pub edge: u8,
    pub forward: bool,
}
/// Retains the source-owned chart and each original graph edge. This directory
/// does not add 2D intersections or new 3D vertices.
pub struct ProjectedCellEdges {
    chart: Arc<CellProjectionChart>,
    edges: Vec<ProjectedEdge>,
    links: Vec<ChartEdgeStep>,
    bytes: u64,
    _reservation: Reservation,
}
impl ProjectedCellEdges {
    pub fn chart(&self) -> &Arc<CellProjectionChart> {
        &self.chart
    }
    pub fn edges(&self) -> &[ProjectedEdge] {
        &self.edges
    }
    /// End-face steps cover the complete edge in source-node order. Other edge
    /// kinds have no steps and remain explicit source-owned constraints.
    pub fn chart_steps(&self, id: u16) -> Option<&[ChartEdgeStep]> {
        let edge = self.edges.get(usize::from(id))?;
        self.links
            .get(edge.start as usize..(edge.start as usize).checked_add(edge.len as usize)?)
    }
    pub fn referenced_bytes(&self) -> u64 {
        self.bytes
    }
}
fn projected_node(
    chart: &CellProjectionChart,
    id: u16,
) -> Result<[ExactCoordinate; 2], SurfaceError> {
    let p = chart
        .graph
        .nodes
        .get(usize::from(id))
        .ok_or(SurfaceError::Geometry)?
        .coordinates;
    let [u, v] = chart.transverse_axes();
    Ok([p[u], p[v]])
}
fn same(a: [ExactCoordinate; 2], b: [ExactCoordinate; 2]) -> Result<bool, SurfaceError> {
    Ok(a[0].compare(b[0]).map_err(error)? == Ordering::Equal
        && a[1].compare(b[1]).map_err(error)? == Ordering::Equal)
}
fn chart_node(
    chart: &CellProjectionChart,
    point: [ExactCoordinate; 2],
    cancel: &impl Fn() -> bool,
) -> Result<u8, SurfaceError> {
    for (id, node) in chart.subdivision.nodes() {
        check(cancel)?;
        if same(point, *node.coordinates())? {
            return Ok(id);
        }
    }
    Err(SurfaceError::Geometry)
}
fn curve_steps(
    chart: &CellProjectionChart,
    edge: CellGraphEdge,
    upper: bool,
    cancel: &impl Fn() -> bool,
    mut emit: impl FnMut(ChartEdgeStep) -> Result<(), SurfaceError>,
) -> Result<usize, SurfaceError> {
    let a = projected_node(chart, edge.nodes[0])?;
    let b = projected_node(chart, edge.nodes[1])?;
    let direction = [0, 1]
        .into_iter()
        .find_map(|axis| match a[axis].compare(b[axis]) {
            Ok(Ordering::Less) => Some(Ok((axis, Ordering::Less))),
            Ok(Ordering::Greater) => Some(Ok((axis, Ordering::Greater))),
            Ok(Ordering::Equal) => None,
            Err(e) => Some(Err(error(e))),
        })
        .ok_or(SurfaceError::Geometry)??;
    let (axis, order) = direction;
    let end = chart_node(chart, b, cancel)?;
    let mut current = chart_node(chart, a, cancel)?;
    let mut count = 0;
    while current != end {
        check(cancel)?;
        if count >= JointSubdivision::MAX_NODES {
            return Err(SurfaceError::Geometry);
        }
        let current_coordinate = chart
            .subdivision
            .node(current)
            .ok_or(SurfaceError::Geometry)?
            .coordinates()[axis];
        let mut next: Option<(u8, u8, ExactCoordinate, bool)> = None;
        for (id, candidate) in chart.subdivision.edges() {
            check(cancel)?;
            if candidate.curves & (1 << usize::from(upper)) == 0 {
                continue;
            }
            let (other, forward) = if candidate.from == current {
                (candidate.to, true)
            } else if candidate.to == current {
                (candidate.from, false)
            } else {
                continue;
            };
            let coordinate = chart
                .subdivision
                .node(other)
                .ok_or(SurfaceError::Geometry)?
                .coordinates()[axis];
            if current_coordinate.compare(coordinate).map_err(error)? != order {
                continue;
            }
            let to_end = coordinate.compare(b[axis]).map_err(error)?;
            if to_end != order && to_end != Ordering::Equal {
                continue;
            }
            if let Some((_, _, old, _)) = next {
                match coordinate.compare(old).map_err(error)? {
                    Ordering::Equal => return Err(SurfaceError::Geometry),
                    value if value != order => continue,
                    _ => {}
                }
            }
            next = Some((id, other, coordinate, forward));
        }
        let (id, other, _, forward) = next.ok_or(SurfaceError::Geometry)?;
        emit(ChartEdgeStep { edge: id, forward })?;
        current = other;
        count += 1;
    }
    if count == 0 {
        return Err(SurfaceError::Geometry);
    }
    Ok(count)
}
fn kind(
    chart: &CellProjectionChart,
    edge: CellGraphEdge,
) -> Result<ProjectedEdgeKind, SurfaceError> {
    let axis = usize::from(chart.axis);
    let a = projected_node(chart, edge.nodes[0])?;
    let b = projected_node(chart, edge.nodes[1])?;
    if same(a, b)? {
        return if edge.geometry == (CellGraphGeometry::Axis { axis: chart.axis }) {
            Ok(ProjectedEdgeKind::Collapsed)
        } else {
            Err(SurfaceError::Geometry)
        };
    }
    let face_bits = (edge.faces >> (2 * axis)) & 3;
    match face_bits {
        1 => Ok(ProjectedEdgeKind::EndFace { upper: false }),
        2 => Ok(ProjectedEdgeKind::EndFace { upper: true }),
        0 => {
            if a[0].compare(b[0]).map_err(error)? != Ordering::Equal
                && a[1].compare(b[1]).map_err(error)? != Ordering::Equal
            {
                return Err(SurfaceError::Geometry);
            }
            Ok(ProjectedEdgeKind::Segment)
        }
        _ => Err(SurfaceError::Geometry),
    }
}
fn exact<T>(count: usize) -> Result<Vec<T>, SurfaceError> {
    let values = allocated(count)?;
    if values.capacity() != count {
        return Err(SurfaceError::Memory);
    }
    Ok(values)
}
fn retained_bytes(edges: usize, links: usize) -> Result<u64, SurfaceError> {
    edges
        .checked_mul(std::mem::size_of::<ProjectedEdge>())
        .and_then(|n| {
            links
                .checked_mul(std::mem::size_of::<ChartEdgeStep>())
                .and_then(|m| n.checked_add(m))
        })
        .and_then(|n| {
            n.checked_add(
                std::mem::size_of::<ProjectedCellEdges>()
                    + crate::surface::shared_header_bytes::<ProjectedCellEdges>(),
            )
        })
        .and_then(|n| u64::try_from(n).ok())
        .ok_or(SurfaceError::Limit)
}
impl CellProjectionChart {
    pub fn project_edges(
        self: &Arc<Self>,
        cancel: impl Fn() -> bool,
    ) -> Result<ProjectedCellEdges, SurfaceError> {
        self.project_edges_with_budget(MemoryBudget::volumes(), cancel)
    }
    pub(in crate::surface) fn project_edges_with_budget(
        self: &Arc<Self>,
        budget: Arc<MemoryBudget>,
        cancel: impl Fn() -> bool,
    ) -> Result<ProjectedCellEdges, SurfaceError> {
        check(&cancel)?;
        let mut total = 0usize;
        for edge in self.graph.edges.iter().copied() {
            check(&cancel)?;
            if let ProjectedEdgeKind::EndFace { upper } = kind(self, edge)? {
                total = total
                    .checked_add(curve_steps(self, edge, upper, &cancel, |_| Ok(()))?)
                    .ok_or(SurfaceError::Limit)?;
                if total > MAX_LINKS {
                    return Err(SurfaceError::Limit);
                }
            }
        }
        let bytes = retained_bytes(self.graph.edges.len(), total)?;
        if bytes > MAX_BYTES {
            return Err(SurfaceError::Limit);
        }
        let reservation = budget.reserve(bytes).ok_or(SurfaceError::Memory)?;
        let mut edges = exact(self.graph.edges.len())?;
        let mut links = exact(total)?;
        for edge in self.graph.edges.iter().copied() {
            check(&cancel)?;
            let start = links.len();
            let kind = kind(self, edge)?;
            if let ProjectedEdgeKind::EndFace { upper } = kind {
                curve_steps(self, edge, upper, &cancel, |step| {
                    if links.len() >= total {
                        return Err(SurfaceError::Geometry);
                    }
                    links.push(step);
                    Ok(())
                })?;
            }
            edges.push(ProjectedEdge {
                source_nodes: edge.nodes,
                kind,
                start: start as u32,
                len: (links.len() - start) as u32,
            });
        }
        check(&cancel)?;
        if links.len() != total {
            return Err(SurfaceError::Geometry);
        }
        Ok(ProjectedCellEdges {
            chart: Arc::clone(self),
            edges,
            links,
            bytes,
            _reservation: reservation,
        })
    }
}
