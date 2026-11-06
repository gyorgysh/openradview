// SPDX-License-Identifier: GPL-3.0-or-later
//! Shared exact nodes and split straight constraints over a rebuilt chart.
use super::*;
mod planar;
pub use planar::{
    BoundedSurfaceFacet, BoundedSurfaceVertex, CellAxisBounds, EnclosedSurfaceGraph, PlanarEdge,
    PlanarHalfEdge, PlanarRefinementLimits, PlanarRegion, PlanarTriangle,
    PlanarTriangulationLimits, ProjectedPlanarGraph, RefinedPlanarGraph, SurfaceEnclosureLimits,
    SurfaceFacetCertificate, TriangulatedPlanarGraph,
};

const NODES: usize = JointSubdivision::MAX_NODES;
const MAX_STEPS: usize = 262_144;
const MAX_BYTES: u64 = 16 * 1024 * 1024;

/// One undirected projected interval. A matching straight chart edge, when
/// present, is the same geometry; curved chart edges are never merged by endpoints.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SplitSegment {
    pub nodes: [u8; 2],
    pub chart_edge: Option<u8>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SplitStep {
    pub segment: u32,
    pub forward: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplitPath {
    /// The complete original source edge projects to this one node; its original
    /// endpoints and heights remain in the retained source graph.
    Collapsed {
        node: u8,
    },
    /// See `ProjectedCellEdges::chart_steps` on the rebuilt edge directory.
    EndFace,
    Segment {
        start: u32,
        len: u32,
    },
}
/// Split projected constraints, retaining contact and source ownership. This is
/// not yet a face-connected planar subdivision or a 3D surface mesh.
pub struct SplitProjectedConstraints {
    contacts: Arc<ProjectedContacts>,
    edges: Arc<ProjectedCellEdges>,
    segments: Vec<SplitSegment>,
    paths: Vec<SplitPath>,
    steps: Vec<SplitStep>,
    bytes: u64,
    _reservation: Reservation,
}
impl SplitProjectedConstraints {
    pub fn contacts(&self) -> &Arc<ProjectedContacts> {
        &self.contacts
    }
    pub fn edges(&self) -> &Arc<ProjectedCellEdges> {
        &self.edges
    }
    pub fn segments(&self) -> &[SplitSegment] {
        &self.segments
    }
    pub fn path(&self, edge: u16) -> Option<SplitPath> {
        self.paths.get(usize::from(edge)).copied()
    }
    /// Segment steps follow the original source edge's node order.
    pub fn steps(&self, edge: u16) -> Option<&[SplitStep]> {
        let SplitPath::Segment { start, len } = self.path(edge)? else {
            return None;
        };
        self.steps
            .get(start as usize..(start as usize).checked_add(len as usize)?)
    }
    pub fn referenced_bytes(&self) -> u64 {
        self.bytes
    }
}
fn includes(
    chart: &CellProjectionChart,
    point: [ExactCoordinate; 2],
    cancel: &impl Fn() -> bool,
) -> Result<bool, SurfaceError> {
    for (_, node) in chart.subdivision.nodes() {
        check(cancel)?;
        if same(point, *node.coordinates())? {
            return Ok(true);
        }
    }
    Ok(false)
}
fn add_insertion(
    chart: &CellProjectionChart,
    points: &mut [[ExactCoordinate; 2]; JointSubdivision::MAX_INSERTIONS],
    count: &mut usize,
    point: [ExactCoordinate; 2],
    cancel: &impl Fn() -> bool,
) -> Result<(), SurfaceError> {
    check(cancel)?;
    if includes(chart, point, cancel)? {
        return Ok(());
    }
    for p in &points[..*count] {
        check(cancel)?;
        if same(*p, point)? {
            return Ok(());
        }
    }
    let limit = JointSubdivision::MAX_INSERTIONS
        .checked_sub(chart.graph.nodes.len())
        .ok_or(SurfaceError::Limit)?;
    if *count >= limit {
        return Err(SurfaceError::Limit);
    }
    points[*count] = point;
    *count += 1;
    Ok(())
}
fn inserted_chart(
    contacts: &ProjectedContacts,
    budget: &Arc<MemoryBudget>,
    cancel: &impl Fn() -> bool,
) -> Result<Arc<CellProjectionChart>, SurfaceError> {
    let old = &contacts.edges.chart;
    let graph = old.graph();
    let axis = usize::from(old.axis());
    let baseline = graph.projection_chart_with_budget(axis, budget.clone(), cancel)?;
    let zero = endpoint(false)?;
    let mut points = [[zero; 2]; JointSubdivision::MAX_INSERTIONS];
    let mut count = 0;
    for (_, node) in old.subdivision().nodes() {
        add_insertion(
            &baseline,
            &mut points,
            &mut count,
            *node.coordinates(),
            cancel,
        )?;
    }
    for contact in &contacts.contacts {
        check(cancel)?;
        match contact.geometry {
            ProjectedContactGeometry::Point(point) => {
                add_insertion(&baseline, &mut points, &mut count, point, cancel)?
            }
            ProjectedContactGeometry::Overlap([a, b]) => {
                add_insertion(&baseline, &mut points, &mut count, a, cancel)?;
                add_insertion(&baseline, &mut points, &mut count, b, cancel)?;
            }
        }
    }
    drop(baseline);
    Ok(Arc::new(
        graph.projection_chart_with_insertions_and_budget(
            axis,
            &points[..count],
            budget.clone(),
            cancel,
        )?,
    ))
}
fn sequence(
    edges: &ProjectedCellEdges,
    id: usize,
    cancel: &impl Fn() -> bool,
    nodes: &mut [u8; NODES],
) -> Result<usize, SurfaceError> {
    let span = source_span(edges, id)?.ok_or(SurfaceError::Geometry)?;
    let axis = span.axis.ok_or(SurfaceError::Geometry)?;
    let mut count = 0usize;
    for (node, value) in edges.chart.subdivision.nodes() {
        check(cancel)?;
        if !span.contains(*value.coordinates())? {
            continue;
        }
        if count == NODES {
            return Err(SurfaceError::Limit);
        }
        let c = value.coordinates()[axis];
        let mut at = count;
        while at > 0 {
            check(cancel)?;
            let previous = edges
                .chart
                .subdivision
                .node(nodes[at - 1])
                .ok_or(SurfaceError::Geometry)?
                .coordinates()[axis];
            match cmp(previous, c)? {
                Ordering::Less => break,
                Ordering::Equal => return Err(SurfaceError::Geometry),
                Ordering::Greater => {
                    nodes[at] = nodes[at - 1];
                    at -= 1;
                }
            }
        }
        nodes[at] = node;
        count += 1;
    }
    if count < 2 {
        return Err(SurfaceError::Geometry);
    }
    let first = *edges
        .chart
        .subdivision
        .node(nodes[0])
        .ok_or(SurfaceError::Geometry)?
        .coordinates();
    let last = *edges
        .chart
        .subdivision
        .node(nodes[count - 1])
        .ok_or(SurfaceError::Geometry)?
        .coordinates();
    if !same(first, span.points[0])? || !same(last, span.points[1])? {
        return Err(SurfaceError::Geometry);
    }
    let source = edges.edges.get(id).ok_or(SurfaceError::Geometry)?;
    if !same(projected_node(&edges.chart, source.source_nodes[0])?, first)? {
        nodes[..count].reverse();
    }
    Ok(count)
}
fn chart_edge(
    chart: &CellProjectionChart,
    pair: [u8; 2],
    cancel: &impl Fn() -> bool,
) -> Result<Option<u8>, SurfaceError> {
    let mut found = None;
    for (id, edge) in chart.subdivision.edges() {
        check(cancel)?;
        if [edge.from.min(edge.to), edge.from.max(edge.to)] != pair
            || !matches!(edge.geometry, CommonZero::Axis { .. })
        {
            continue;
        }
        if found.replace(id).is_some() {
            return Err(SurfaceError::Geometry);
        }
    }
    Ok(found)
}
fn intern(
    directory: &mut [u32; NODES * NODES],
    count: &mut usize,
    pair: [u8; 2],
) -> Result<(SplitStep, bool), SurfaceError> {
    if pair[0] == pair[1] || pair.into_iter().any(|id| usize::from(id) >= NODES) {
        return Err(SurfaceError::Geometry);
    }
    let [a, b] = [pair[0].min(pair[1]), pair[0].max(pair[1])];
    let slot = &mut directory[usize::from(a) * NODES + usize::from(b)];
    let fresh = *slot == u32::MAX;
    if fresh {
        let id = u32::try_from(*count).map_err(|_| SurfaceError::Limit)?;
        if id == u32::MAX {
            return Err(SurfaceError::Limit);
        }
        *count = count.checked_add(1).ok_or(SurfaceError::Limit)?;
        *slot = id;
    }
    Ok((
        SplitStep {
            segment: *slot,
            forward: pair[0] == a,
        },
        fresh,
    ))
}
fn bytes(edges: usize, segments: usize, steps: usize) -> Result<u64, SurfaceError> {
    [
        (edges, std::mem::size_of::<SplitPath>()),
        (segments, std::mem::size_of::<SplitSegment>()),
        (steps, std::mem::size_of::<SplitStep>()),
    ]
    .into_iter()
    .try_fold(
        std::mem::size_of::<SplitProjectedConstraints>(),
        |sum, (n, size)| {
            n.checked_mul(size)
                .and_then(|n| sum.checked_add(n))
                .ok_or(SurfaceError::Limit)
        },
    )
    .and_then(|n| u64::try_from(n).map_err(|_| SurfaceError::Limit))
}
impl ProjectedContacts {
    /// Rebuild the chart with every contact and split the projected straight
    /// constraints at all chart nodes. Coincident pieces share a segment ID;
    /// source-edge paths preserve each owner and direction. Limits and numerical
    /// refusal never publish partial output.
    pub fn split(
        self: &Arc<Self>,
        cancel: impl Fn() -> bool,
    ) -> Result<SplitProjectedConstraints, SurfaceError> {
        self.split_with_budget(MemoryBudget::volumes(), cancel)
    }
    pub(in crate::surface) fn split_with_budget(
        self: &Arc<Self>,
        budget: Arc<MemoryBudget>,
        cancel: impl Fn() -> bool,
    ) -> Result<SplitProjectedConstraints, SurfaceError> {
        check(&cancel)?;
        let scratch_bytes =
            std::mem::size_of::<[[ExactCoordinate; 2]; JointSubdivision::MAX_INSERTIONS]>()
                .checked_add(std::mem::size_of::<[u32; NODES * NODES]>())
                .and_then(|n| n.checked_add(std::mem::size_of::<[u8; NODES]>()))
                .ok_or(SurfaceError::Limit)?;
        let _scratch = budget
            .reserve(scratch_bytes as u64)
            .ok_or(SurfaceError::Memory)?;
        let chart = inserted_chart(self, &budget, &cancel)?;
        let edges = Arc::new(chart.project_edges_with_budget(budget.clone(), &cancel)?);
        let mut directory = [u32::MAX; NODES * NODES];
        let mut nodes = [0u8; NODES];
        let (mut segment_count, mut step_count) = (0usize, 0usize);
        for (id, edge) in edges.edges.iter().enumerate() {
            check(&cancel)?;
            if edge.kind != ProjectedEdgeKind::Segment {
                continue;
            }
            let count = sequence(&edges, id, &cancel, &mut nodes)?;
            step_count = step_count
                .checked_add(count - 1)
                .ok_or(SurfaceError::Limit)?;
            if step_count > MAX_STEPS {
                return Err(SurfaceError::Limit);
            }
            for pair in nodes[..count].windows(2) {
                intern(&mut directory, &mut segment_count, [pair[0], pair[1]])?;
            }
        }
        let retained = bytes(edges.edges.len(), segment_count, step_count)?;
        if retained > MAX_BYTES {
            return Err(SurfaceError::Limit);
        }
        let reservation = budget.reserve(retained).ok_or(SurfaceError::Memory)?;
        let mut segments = exact(segment_count)?;
        let mut paths = exact(edges.edges.len())?;
        let mut steps = exact(step_count)?;
        directory.fill(u32::MAX);
        let mut actual_count = 0usize;
        for (id, edge) in edges.edges.iter().enumerate() {
            check(&cancel)?;
            let path = match edge.kind {
                ProjectedEdgeKind::Collapsed => {
                    let point = projected_node(&edges.chart, edge.source_nodes[0])?;
                    SplitPath::Collapsed {
                        node: chart_node(&edges.chart, point, &cancel)?,
                    }
                }
                ProjectedEdgeKind::EndFace { .. } => SplitPath::EndFace,
                ProjectedEdgeKind::Segment => {
                    let start = steps.len();
                    let count = sequence(&edges, id, &cancel, &mut nodes)?;
                    for pair in nodes[..count].windows(2) {
                        check(&cancel)?;
                        let (step, fresh) =
                            intern(&mut directory, &mut actual_count, [pair[0], pair[1]])?;
                        if fresh {
                            if segments.len() >= segment_count
                                || step.segment as usize != segments.len()
                            {
                                return Err(SurfaceError::Geometry);
                            }
                            let nodes = [pair[0].min(pair[1]), pair[0].max(pair[1])];
                            segments.push(SplitSegment {
                                nodes,
                                chart_edge: chart_edge(&edges.chart, nodes, &cancel)?,
                            });
                        }
                        if steps.len() >= step_count {
                            return Err(SurfaceError::Geometry);
                        }
                        steps.push(step);
                    }
                    SplitPath::Segment {
                        start: start as u32,
                        len: (steps.len() - start) as u32,
                    }
                }
            };
            paths.push(path);
        }
        check(&cancel)?;
        if segments.len() != segment_count
            || actual_count != segment_count
            || steps.len() != step_count
            || paths.len() != edges.edges.len()
        {
            return Err(SurfaceError::Geometry);
        }
        Ok(SplitProjectedConstraints {
            contacts: Arc::clone(self),
            edges,
            segments,
            paths,
            steps,
            bytes: retained,
            _reservation: reservation,
        })
    }
}

#[cfg(test)]
mod tests;
