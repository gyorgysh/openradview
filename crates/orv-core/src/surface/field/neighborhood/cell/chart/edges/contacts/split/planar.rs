// SPDX-License-Identifier: GPL-3.0-or-later
//! Connected planar rotation system over exact chart curves and source cuts.
use super::*;
mod refine;
pub use refine::{
    AdaptiveFacetLimits, AdaptiveFacetMesh, AdaptiveFacetState, AdaptiveFacetTriangle,
    AdaptiveStopReason, BoundedSurfaceFacet, BoundedSurfaceVertex, CellAxisBounds,
    EnclosedSurfaceGraph, PlanarRefinementLimits, PlanarTriangle, PlanarTriangulationLimits,
    RefinedPlanarGraph, SurfaceEnclosureLimits, SurfaceFacetCertificate, TriangulatedPlanarGraph,
};

const MAX_EDGES: usize = 8192;
const MAX_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlanarEdge {
    pub nodes: [u8; 2],
    /// Original chart edge; its exact geometry and field ownership remain there.
    pub chart_edge: Option<u8>,
    /// Shared straight source segment, including coincident chart edges.
    pub segment: Option<u32>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlanarHalfEdge {
    pub edge: u32,
    pub from: u8,
    pub to: u8,
    pub twin: u32,
    pub next: u32,
    /// Region on the left; None is the exterior of the chart square.
    pub region: Option<u32>,
    /// The original chart region on the left, before source cuts.
    pub chart_region: Option<u8>,
    chart_half: Option<u8>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlanarRegion {
    pub first: u32,
    pub edge_count: u32,
    pub chart_region: u8,
    pub signs: [i8; 2],
}
/// Exact boundary walks over one connected chart graph. Isolated constraints
/// remain in the input chart. No surface triangulation or seam is implied.
pub struct ProjectedPlanarGraph {
    input: Arc<SplitProjectedConstraints>,
    edges: Vec<PlanarEdge>,
    halves: Vec<PlanarHalfEdge>,
    regions: Vec<PlanarRegion>,
    bytes: u64,
    _reservation: Reservation,
}
impl ProjectedPlanarGraph {
    pub fn input(&self) -> &Arc<SplitProjectedConstraints> {
        &self.input
    }
    pub fn edges(&self) -> &[PlanarEdge] {
        &self.edges
    }
    pub fn half_edges(&self) -> &[PlanarHalfEdge] {
        &self.halves
    }
    pub fn regions(&self) -> &[PlanarRegion] {
        &self.regions
    }
    /// Includes reserved capacity for the maximum number of boundary walks.
    pub fn referenced_bytes(&self) -> u64 {
        self.bytes
    }
}
fn size(parts: &[(usize, usize)], header: usize) -> Result<u64, SurfaceError> {
    parts
        .iter()
        .try_fold(header, |sum, (count, item)| {
            count
                .checked_mul(*item)
                .and_then(|n| sum.checked_add(n))
                .ok_or(SurfaceError::Limit)
        })
        .and_then(|n| u64::try_from(n).map_err(|_| SurfaceError::Limit))
}
fn chart_half(chart: &JointSubdivision, id: u8, reverse: bool) -> Result<u8, SurfaceError> {
    let half = usize::from(id)
        .checked_mul(2)
        .and_then(|n| n.checked_add(usize::from(reverse)))
        .ok_or(SurfaceError::Limit)?;
    let half = u8::try_from(half).map_err(|_| SurfaceError::Limit)?;
    if chart.half_edge(half).is_none_or(|h| h.edge != id) {
        return Err(SurfaceError::Geometry);
    }
    Ok(half)
}
fn octant(chart: &JointSubdivision, half: &PlanarHalfEdge) -> Result<u8, SurfaceError> {
    if let Some(id) = half.chart_half {
        return chart.half_octant(id).map_err(error);
    }
    let a = chart
        .node(half.from)
        .ok_or(SurfaceError::Geometry)?
        .coordinates();
    let b = chart
        .node(half.to)
        .ok_or(SurfaceError::Geometry)?
        .coordinates();
    match (cmp(a[0], b[0])?, cmp(a[1], b[1])?) {
        (Ordering::Less, Ordering::Equal) => Ok(0),
        (Ordering::Equal, Ordering::Less) => Ok(2),
        (Ordering::Greater, Ordering::Equal) => Ok(4),
        (Ordering::Equal, Ordering::Greater) => Ok(6),
        _ => Err(SurfaceError::Geometry),
    }
}
fn angle(
    chart: &JointSubdivision,
    a: &PlanarHalfEdge,
    b: &PlanarHalfEdge,
) -> Result<Ordering, SurfaceError> {
    if a.from != b.from {
        return Err(SurfaceError::Geometry);
    }
    if let (Some(a), Some(b)) = (a.chart_half, b.chart_half) {
        return chart.angle_compare(a, b).map_err(error);
    }
    let order = octant(chart, a)?.cmp(&octant(chart, b)?);
    // Equal axis directions must have been split and merged before this stage.
    if order == Ordering::Equal {
        Err(SurfaceError::Geometry)
    } else {
        Ok(order)
    }
}
fn root(parents: &mut [u8; NODES], mut node: u8) -> u8 {
    while parents[usize::from(node)] != node {
        let next = parents[usize::from(node)];
        parents[usize::from(node)] = parents[usize::from(next)];
        node = next;
    }
    node
}
fn connected(
    degrees: &[usize; NODES],
    parents: &mut [u8; NODES],
    cancel: &impl Fn() -> bool,
) -> Result<(), SurfaceError> {
    let mut component = None;
    for (node, degree) in degrees.iter().enumerate() {
        check(cancel)?;
        if *degree == 0 {
            continue;
        }
        let r = root(parents, node as u8);
        if component.is_some_and(|old| old != r) {
            return Err(SurfaceError::Geometry);
        }
        component = Some(r);
    }
    if component.is_none() {
        return Err(SurfaceError::Geometry);
    }
    Ok(())
}
impl SplitProjectedConstraints {
    /// Build source-bound face walks using exact chart tangents and axis rays.
    /// A disconnected edge graph is refused because a face could have multiple
    /// boundary components; isolated chart nodes are retained by the input.
    pub fn planar_graph(
        self: &Arc<Self>,
        cancel: impl Fn() -> bool,
    ) -> Result<ProjectedPlanarGraph, SurfaceError> {
        self.planar_graph_with_budget(MemoryBudget::volumes(), cancel)
    }
    pub(in crate::surface) fn planar_graph_with_budget(
        self: &Arc<Self>,
        budget: Arc<MemoryBudget>,
        cancel: impl Fn() -> bool,
    ) -> Result<ProjectedPlanarGraph, SurfaceError> {
        check(&cancel)?;
        let chart = self.edges.chart.subdivision();
        let chart_count = chart.edges().count();
        let extra = self
            .segments
            .iter()
            .filter(|s| s.chart_edge.is_none())
            .count();
        let edge_count = chart_count.checked_add(extra).ok_or(SurfaceError::Limit)?;
        if edge_count == 0 || edge_count > MAX_EDGES {
            return Err(SurfaceError::Limit);
        }
        let half_count = edge_count.checked_mul(2).ok_or(SurfaceError::Limit)?;
        let output = size(
            &[
                (edge_count, std::mem::size_of::<PlanarEdge>()),
                (half_count, std::mem::size_of::<PlanarHalfEdge>()),
                (half_count, std::mem::size_of::<PlanarRegion>()),
            ],
            std::mem::size_of::<ProjectedPlanarGraph>(),
        )?;
        if output > MAX_BYTES {
            return Err(SurfaceError::Limit);
        }
        let scratch = size(
            &[
                (half_count, std::mem::size_of::<u32>()),
                (half_count, std::mem::size_of::<u8>()),
            ],
            std::mem::size_of::<[usize; NODES * 3 + 1]>()
                + std::mem::size_of::<[u8; NODES]>()
                + std::mem::size_of::<Vec<u32>>()
                + std::mem::size_of::<Vec<u8>>(),
        )?;
        let _scratch = budget.reserve(scratch).ok_or(SurfaceError::Memory)?;
        let reservation = budget.reserve(output).ok_or(SurfaceError::Memory)?;
        let mut edges = exact(edge_count)?;
        let mut halves = exact(half_count)?;
        let mut regions = exact(half_count)?;
        let mut outgoing = exact(half_count)?;
        outgoing.resize(half_count, u32::MAX);
        let mut visited = exact(half_count)?;
        visited.resize(half_count, 0u8);
        for (id, e) in chart.edges() {
            check(&cancel)?;
            if usize::from(id) != edges.len() {
                return Err(SurfaceError::Geometry);
            }
            edges.push(PlanarEdge {
                nodes: [e.from, e.to],
                chart_edge: Some(id),
                segment: None,
            });
        }
        for (id, s) in self.segments.iter().enumerate() {
            check(&cancel)?;
            let id = u32::try_from(id).map_err(|_| SurfaceError::Limit)?;
            if let Some(chart_id) = s.chart_edge {
                let e = edges
                    .get_mut(usize::from(chart_id))
                    .ok_or(SurfaceError::Geometry)?;
                if e.nodes != s.nodes || e.segment.replace(id).is_some() {
                    return Err(SurfaceError::Geometry);
                }
            } else {
                if edges.len() >= edge_count {
                    return Err(SurfaceError::Geometry);
                }
                edges.push(PlanarEdge {
                    nodes: s.nodes,
                    chart_edge: None,
                    segment: Some(id),
                });
            }
        }
        if edges.len() != edge_count {
            return Err(SurfaceError::Geometry);
        }
        let mut degrees = [0usize; NODES];
        let mut parents = std::array::from_fn::<_, NODES, _>(|i| i as u8);
        for (id, e) in edges.iter().enumerate() {
            check(&cancel)?;
            if e.nodes[0] == e.nodes[1]
                || e.nodes
                    .iter()
                    .any(|n| usize::from(*n) >= NODES || chart.node(*n).is_none())
            {
                return Err(SurfaceError::Geometry);
            }
            let [a, b] = e.nodes.map(|n| root(&mut parents, n));
            if a != b {
                parents[usize::from(b)] = a;
            }
            for reverse in [false, true] {
                let [from, to] = if reverse {
                    [e.nodes[1], e.nodes[0]]
                } else {
                    e.nodes
                };
                degrees[usize::from(from)] += 1;
                let chart_half = e
                    .chart_edge
                    .map(|n| chart_half(chart, n, reverse))
                    .transpose()?;
                let chart_region =
                    chart_half.and_then(|n| chart.half_edge(n).and_then(|h| h.region));
                halves.push(PlanarHalfEdge {
                    edge: id as u32,
                    from,
                    to,
                    twin: (2 * id + usize::from(!reverse)) as u32,
                    next: u32::MAX,
                    region: None,
                    chart_region,
                    chart_half,
                });
            }
        }
        connected(&degrees, &mut parents, &cancel)?;
        let mut offsets = [0usize; NODES + 1];
        for i in 0..NODES {
            offsets[i + 1] = offsets[i]
                .checked_add(degrees[i])
                .ok_or(SurfaceError::Limit)?;
        }
        if offsets[NODES] != half_count {
            return Err(SurfaceError::Geometry);
        }
        let mut cursor = [0usize; NODES];
        for (id, h) in halves.iter().enumerate() {
            let node = usize::from(h.from);
            let at = offsets[node] + cursor[node];
            *outgoing.get_mut(at).ok_or(SurfaceError::Geometry)? = id as u32;
            cursor[node] += 1;
        }
        for node in 0..NODES {
            let start = offsets[node];
            let end = offsets[node + 1];
            for i in start + usize::from(start < end)..end {
                let mut j = i;
                while j > start {
                    check(&cancel)?;
                    let a = halves
                        .get(outgoing[j - 1] as usize)
                        .ok_or(SurfaceError::Geometry)?;
                    let b = halves
                        .get(outgoing[j] as usize)
                        .ok_or(SurfaceError::Geometry)?;
                    match angle(chart, a, b)? {
                        Ordering::Less => break,
                        Ordering::Greater => outgoing.swap(j - 1, j),
                        Ordering::Equal => return Err(SurfaceError::Geometry),
                    }
                    j -= 1;
                }
            }
        }
        for id in 0..half_count {
            check(&cancel)?;
            let h = halves[id];
            let node = usize::from(h.to);
            let ring = &outgoing[offsets[node]..offsets[node + 1]];
            let at = ring
                .iter()
                .position(|v| *v == h.twin)
                .ok_or(SurfaceError::Geometry)?;
            if ring.is_empty() {
                return Err(SurfaceError::Geometry);
            }
            halves[id].next = ring[(at + ring.len() - 1) % ring.len()];
            if h.chart_half.is_none() {
                let node = usize::from(h.from);
                let ring = &outgoing[offsets[node]..offsets[node + 1]];
                let at = ring
                    .iter()
                    .position(|v| *v == id as u32)
                    .ok_or(SurfaceError::Geometry)?;
                let mut parent = None;
                for step in 1..=ring.len() {
                    let prior = halves
                        .get(ring[(at + ring.len() - step) % ring.len()] as usize)
                        .ok_or(SurfaceError::Geometry)?;
                    if prior.chart_half.is_some() {
                        parent = Some(prior.chart_region);
                        break;
                    }
                }
                halves[id].chart_region = match parent {
                    Some(value) => value,
                    None => Some(
                        chart
                            .interior_region(h.from)
                            .ok_or(SurfaceError::Geometry)?,
                    ),
                };
            }
        }
        let mut outside = 0usize;
        for start in 0..half_count {
            check(&cancel)?;
            if visited[start] != 0 {
                continue;
            }
            let parent = halves[start].chart_region;
            let region = if parent.is_some() {
                Some(regions.len() as u32)
            } else {
                outside += 1;
                None
            };
            let mut at = start;
            let mut count = 0usize;
            loop {
                check(&cancel)?;
                if at >= half_count || visited[at] != 0 || halves[at].chart_region != parent {
                    return Err(SurfaceError::Geometry);
                }
                visited[at] = 1;
                halves[at].region = region;
                count += 1;
                at = halves[at].next as usize;
                if at == start {
                    break;
                }
                if count >= half_count {
                    return Err(SurfaceError::Geometry);
                }
            }
            if let Some(parent) = parent {
                let signs = chart
                    .regions()
                    .find(|(id, _)| *id == parent)
                    .ok_or(SurfaceError::Geometry)?
                    .1
                    .signs;
                if regions.len() >= half_count {
                    return Err(SurfaceError::Geometry);
                }
                regions.push(PlanarRegion {
                    first: start as u32,
                    edge_count: count as u32,
                    chart_region: parent,
                    signs,
                });
            }
        }
        if outside != 1 {
            return Err(SurfaceError::Geometry);
        }
        check(&cancel)?;
        Ok(ProjectedPlanarGraph {
            input: Arc::clone(self),
            edges,
            halves,
            regions,
            bytes: output,
            _reservation: reservation,
        })
    }
}

#[cfg(test)]
mod tests;
