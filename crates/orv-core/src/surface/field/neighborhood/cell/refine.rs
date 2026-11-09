// SPDX-License-Identifier: GPL-3.0-or-later
//! Canonical certified 3D boundary polylines over a completed source-cell graph.
use super::*;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellPathLimits {
    /// Unit-cell max-coordinate chord error 2^-bits. This does not bound surface,
    /// physical-coordinate or GPU rounding error.
    pub precision_bits: u8,
    pub max_vertices: usize,
    /// Additional retained typed storage. The input graph keeps its own charge.
    pub max_bytes: u64,
}
impl Default for CellPathLimits {
    fn default() -> Self {
        Self {
            precision_bits: 8,
            max_vertices: RefinementLimits::MAX_VERTICES,
            max_bytes: RefinementLimits::MAX_BYTES,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CellPathOrigin {
    Node(u16),
    Edge(u16),
}
#[derive(Clone, Copy, Debug)]
pub struct CellPathVertex {
    coordinates: [ExactCoordinate; 3],
    origin: CellPathOrigin,
}
impl CellPathVertex {
    pub fn coordinates(&self) -> &[ExactCoordinate; 3] {
        &self.coordinates
    }
    pub fn origin(&self) -> CellPathOrigin {
        self.origin
    }
}
#[derive(Clone, Copy)]
struct Path {
    start: u32,
    len: u32,
}
/// Completed edge paths, not a triangulated or editable source surface. Original
/// graph nodes remain the vertex prefix, including isolated/inactive constraints.
pub struct RefinedCellGraph {
    input: Arc<SourceCellGraph>,
    vertices: Vec<CellPathVertex>,
    indices: Vec<u32>,
    paths: Vec<Path>,
    bits: u8,
    bytes: u64,
    _reservation: Reservation,
}
impl RefinedCellGraph {
    pub fn graph(&self) -> &Arc<SourceCellGraph> {
        &self.input
    }
    pub fn vertices(&self) -> &[CellPathVertex] {
        &self.vertices
    }
    pub fn vertex(&self, id: u32) -> Option<&CellPathVertex> {
        self.vertices.get(id as usize)
    }
    /// Canonical input-edge node order. Incident regions use this same path,
    /// reversing traversal for the opposite half-edge.
    pub fn edge_vertices(&self, id: u16) -> Option<&[u32]> {
        let p = self.paths.get(usize::from(id))?;
        self.indices
            .get(p.start as usize..(p.start as usize).checked_add(p.len as usize)?)
    }
    pub fn maximum_coordinate_error(&self) -> Rational {
        Rational {
            numerator: 1,
            denominator: 1u64 << self.bits,
        }
    }
    /// Additional retained typed storage, excluding the separately owned input.
    pub fn referenced_bytes(&self) -> u64 {
        self.bytes
    }
    /// Walk a completed region without repeating the closing endpoint. Repeated
    /// original vertices at contacts/cuts remain distinct visits, never welded.
    pub fn region_boundary(
        &self,
        id: u16,
        cancel: impl Fn() -> bool,
        mut emit: impl FnMut(u32) -> Result<(), SurfaceError>,
    ) -> Result<(), SurfaceError> {
        check(&cancel)?;
        let region = self
            .input
            .regions
            .get(usize::from(id))
            .ok_or(SurfaceError::Geometry)?;
        let mut current = region.first;
        for _ in 0..region.edge_count {
            check(&cancel)?;
            let h = self
                .input
                .halves
                .get(usize::from(current))
                .ok_or(SurfaceError::Geometry)?;
            if !h.eligible || h.region != Some(id) {
                return Err(SurfaceError::Geometry);
            }
            let e = self
                .input
                .edges
                .get(usize::from(h.edge))
                .ok_or(SurfaceError::Geometry)?;
            let path = self.edge_vertices(h.edge).ok_or(SurfaceError::Geometry)?;
            if path.len() < 2
                || path[0] != u32::from(e.nodes[0])
                || path[path.len() - 1] != u32::from(e.nodes[1])
            {
                return Err(SurfaceError::Geometry);
            }
            let forward = h.from == e.nodes[0];
            if [h.from, h.to]
                != if forward {
                    e.nodes
                } else {
                    [e.nodes[1], e.nodes[0]]
                }
            {
                return Err(SurfaceError::Geometry);
            }
            for i in 0..path.len() - 1 {
                check(&cancel)?;
                let index = if forward { i } else { path.len() - 1 - i };
                let vertex = path[index];
                let v = self.vertex(vertex).ok_or(SurfaceError::Geometry)?;
                let expected = if index == 0 {
                    CellPathOrigin::Node(e.nodes[0])
                } else if index + 1 == path.len() {
                    CellPathOrigin::Node(e.nodes[1])
                } else {
                    CellPathOrigin::Edge(h.edge)
                };
                if v.origin != expected {
                    return Err(SurfaceError::Geometry);
                }
                emit(vertex)?;
            }
            current = h.next;
        }
        if current != region.first {
            return Err(SurfaceError::Geometry);
        }
        check(&cancel)
    }
}
fn exact<T>(count: usize) -> Result<Vec<T>, SurfaceError> {
    let v = allocated(count)?;
    if v.capacity() != count {
        return Err(SurfaceError::Memory);
    }
    Ok(v)
}
fn bytes(vertices: usize, indices: usize, edges: usize) -> Result<u64, SurfaceError> {
    [
        (vertices, std::mem::size_of::<CellPathVertex>()),
        (indices, std::mem::size_of::<u32>()),
        (edges, std::mem::size_of::<Path>()),
    ]
    .into_iter()
    .try_fold(std::mem::size_of::<RefinedCellGraph>(), |sum, (n, size)| {
        n.checked_mul(size)
            .and_then(|v| sum.checked_add(v))
            .ok_or(SurfaceError::Limit)
    })
    .and_then(|v| u64::try_from(v).map_err(|_| SurfaceError::Limit))
}
struct ArcSpec {
    field: Bilinear,
    points: [[ExactCoordinate; 2]; 2],
    face: u8,
    first: u16,
    last: u16,
    reverse: bool,
}
impl SourceCellGraph {
    fn arc_spec(&self, e: CellGraphEdge) -> Result<ArcSpec, SurfaceError> {
        let CellGraphGeometry::Face { face } = e.geometry else {
            return Err(SurfaceError::Geometry);
        };
        let axis = usize::from(face / 2);
        let [u, v] = transverse(axis);
        let mut nodes = e.nodes;
        let point = |id: u16| {
            self.nodes
                .get(usize::from(id))
                .map(|n| [n.coordinates[u], n.coordinates[v]])
                .ok_or(SurfaceError::Geometry)
        };
        let reverse = match point(nodes[0])?[0]
            .compare(point(nodes[1])?[0])
            .map_err(error)?
        {
            Ordering::Greater => true,
            Ordering::Less => false,
            Ordering::Equal => return Err(SurfaceError::Geometry),
        };
        if reverse {
            nodes.swap(0, 1);
        }
        Ok(ArcSpec {
            field: self
                .cell
                .interpolant()
                .and_then(|f| f.face(axis, face & 1 != 0))
                .ok_or(SurfaceError::Geometry)?,
            points: [point(nodes[0])?, point(nodes[1])?],
            face,
            first: nodes[0],
            last: nodes[1],
            reverse,
        })
    }
    pub fn refine_edges(
        self: &Arc<Self>,
        limits: CellPathLimits,
        cancel: impl Fn() -> bool,
    ) -> Result<RefinedCellGraph, SurfaceError> {
        self.refine_edges_with_budget(limits, MemoryBudget::volumes(), cancel)
    }
    pub(in crate::surface) fn refine_edges_with_budget(
        self: &Arc<Self>,
        limits: CellPathLimits,
        budget: Arc<MemoryBudget>,
        cancel: impl Fn() -> bool,
    ) -> Result<RefinedCellGraph, SurfaceError> {
        check(&cancel)?;
        if limits.precision_bits > RefinementLimits::MAX_PRECISION_BITS
            || limits.max_vertices > RefinementLimits::MAX_VERTICES
            || limits.max_bytes > RefinementLimits::MAX_BYTES
        {
            return Err(SurfaceError::Limit);
        }
        if self.state != CellGraphState::Finite {
            return Err(SurfaceError::Geometry);
        }
        let original = self.nodes.len();
        let mut vertex_count = original;
        let mut index_count = 2 * self.edges.len();
        if vertex_count > limits.max_vertices
            || bytes(vertex_count, index_count, self.edges.len())? > limits.max_bytes
        {
            return Err(SurfaceError::Limit);
        }
        let scratch_bytes = self
            .edges
            .len()
            .checked_mul(std::mem::size_of::<u32>())
            .and_then(|n| n.checked_add(std::mem::size_of::<Vec<u32>>()))
            .ok_or(SurfaceError::Limit)?;
        let _scratch = budget
            .reserve(scratch_bytes as u64)
            .ok_or(SurfaceError::Memory)?;
        let mut counts = exact(self.edges.len())?;
        for edge in &self.edges {
            check(&cancel)?;
            let mut count = 2u32;
            if matches!(edge.geometry, CellGraphGeometry::Face { .. }) {
                let arc = self.arc_spec(*edge)?;
                arc.field.refine_arc(
                    arc.points,
                    limits.precision_bits,
                    &budget,
                    &cancel,
                    |_, endpoint| {
                        if !endpoint {
                            vertex_count =
                                vertex_count.checked_add(1).ok_or(SurfaceError::Limit)?;
                            index_count = index_count.checked_add(1).ok_or(SurfaceError::Limit)?;
                            count = count.checked_add(1).ok_or(SurfaceError::Limit)?;
                            if vertex_count > limits.max_vertices
                                || bytes(vertex_count, index_count, self.edges.len())?
                                    > limits.max_bytes
                            {
                                return Err(SurfaceError::Limit);
                            }
                        }
                        Ok(())
                    },
                )?;
            }
            counts.push(count);
        }
        let retained = bytes(vertex_count, index_count, self.edges.len())?;
        let reservation = budget.reserve(retained).ok_or(SurfaceError::Memory)?;
        let mut vertices = exact(vertex_count)?;
        let mut indices = exact(index_count)?;
        let mut paths = exact(self.edges.len())?;
        for (id, node) in self.nodes.iter().enumerate() {
            check(&cancel)?;
            vertices.push(CellPathVertex {
                coordinates: node.coordinates,
                origin: CellPathOrigin::Node(id as u16),
            });
        }
        for (id, e) in self.edges.iter().copied().enumerate() {
            check(&cancel)?;
            let start = indices.len();
            let edge_id = id as u16;
            if start.checked_add(2).is_none_or(|n| n > index_count) {
                return Err(SurfaceError::Geometry);
            }
            match e.geometry {
                CellGraphGeometry::Axis { .. } => {
                    indices.push(u32::from(e.nodes[0]));
                    indices.push(u32::from(e.nodes[1]));
                }
                CellGraphGeometry::Face { .. } => {
                    let arc = self.arc_spec(e)?;
                    indices.push(u32::from(arc.first));
                    let [u, v] = transverse(usize::from(arc.face / 2));
                    let fixed = endpoint(arc.face & 1 != 0)?;
                    arc.field.refine_arc(
                        arc.points,
                        limits.precision_bits,
                        &budget,
                        &cancel,
                        |point, endpoint| {
                            let index = if endpoint {
                                u32::from(arc.last)
                            } else {
                                if vertices.len() >= vertex_count {
                                    return Err(SurfaceError::Geometry);
                                }
                                let vertex_id = vertices.len() as u32;
                                let mut p = [fixed; 3];
                                p[u] = point[0];
                                p[v] = point[1];
                                vertices.push(CellPathVertex {
                                    coordinates: p,
                                    origin: CellPathOrigin::Edge(edge_id),
                                });
                                vertex_id
                            };
                            if indices.len() >= index_count {
                                return Err(SurfaceError::Geometry);
                            }
                            indices.push(index);
                            Ok(())
                        },
                    )?;
                    if arc.reverse {
                        indices[start..].reverse();
                    }
                }
            }
            let len = indices.len() - start;
            if len != counts[id] as usize
                || indices[start] != u32::from(e.nodes[0])
                || indices[indices.len() - 1] != u32::from(e.nodes[1])
            {
                return Err(SurfaceError::Geometry);
            }
            paths.push(Path {
                start: start as u32,
                len: len as u32,
            });
        }
        check(&cancel)?;
        if vertices.len() != vertex_count || indices.len() != index_count {
            return Err(SurfaceError::Geometry);
        }
        Ok(RefinedCellGraph {
            input: Arc::clone(self),
            vertices,
            indices,
            paths,
            bits: limits.precision_bits,
            bytes: retained,
            _reservation: reservation,
        })
    }
}
