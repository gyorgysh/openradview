// SPDX-License-Identifier: GPL-3.0-or-later
//! Atomic application of source-bound face partition requests.
use super::*;
mod compose;
pub use compose::FacePromotionLimits;

const MAX_OUTPUT: u64 = 32 * 1024 * 1024;
const MAX_CELL_VERTICES: usize = 65_536;
const MAX_CELL_TRIANGLES: usize = 65_536;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AppliedFaceLimits {
    /// Requested local axial tolerance for each retained source cell.
    pub tolerance_bits: [u8; 2],
    pub max_vertices_per_cell: usize,
    pub max_triangles_per_cell: usize,
    pub max_operations: usize,
    /// Additional retained output; input and scratch are separately admitted.
    pub max_bytes: u64,
}
impl Default for AppliedFaceLimits {
    fn default() -> Self {
        Self {
            tolerance_bits: [8; 2],
            max_vertices_per_cell: MAX_CELL_VERTICES,
            max_triangles_per_cell: MAX_CELL_TRIANGLES,
            max_operations: MAX_WORK,
            max_bytes: MAX_OUTPUT,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppliedFaceVertex {
    Retained {
        id: u32,
        position: Option<[u32; 3]>,
    },
    /// `point` identifies the exact coordinate in the retained partition.
    Split {
        point: u32,
        edge: [u32; 2],
        position: [u32; 3],
    },
}
impl AppliedFaceVertex {
    pub fn position(self) -> Option<[u32; 3]> {
        match self {
            Self::Retained { position, .. } => position,
            Self::Split { position, .. } => Some(position),
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppliedTriangleState {
    WithinTolerance {
        numerator: u128,
        denominator: u128,
    },
    /// Includes failed proofs and certified bounds above the requested tolerance.
    Unresolved {
        certificate: SurfaceFacetCertificate,
    },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AppliedFaceTriangle {
    pub vertices: [u32; 3],
    /// Index in the corresponding retained conforming cell mesh.
    pub source_triangle: u32,
    pub source_facet: u32,
    pub state: AppliedTriangleState,
}
pub struct AppliedFaceCell {
    vertices: Vec<AppliedFaceVertex>,
    triangles: Vec<AppliedFaceTriangle>,
}
impl AppliedFaceCell {
    pub fn vertices(&self) -> &[AppliedFaceVertex] {
        &self.vertices
    }
    pub fn triangles(&self) -> &[AppliedFaceTriangle] {
        &self.triangles
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AppliedFaceSpan {
    /// Index in the retained partition; None means that side has no coverage.
    pub partition_span: u32,
    pub segments: [Option<[u32; 2]>; 2],
}
/// Both updated candidate meshes are published together. Gaps, unproved facets,
/// other faces, and whole-grid/cross-projection seams remain explicit or open.
pub struct AppliedFacePair {
    input: Arc<SharedFacePartition>,
    cells: [AppliedFaceCell; 2],
    point_vertices: Vec<[Option<u32>; 2]>,
    spans: Vec<AppliedFaceSpan>,
    bytes: u64,
    _reservation: Reservation,
}
impl AppliedFacePair {
    pub const POSITION_DENOMINATOR: u32 = REFINED_GRID;
    pub fn input(&self) -> &Arc<SharedFacePartition> {
        &self.input
    }
    pub fn cells(&self) -> &[AppliedFaceCell; 2] {
        &self.cells
    }
    pub fn point_vertices(&self) -> &[[Option<u32>; 2]] {
        &self.point_vertices
    }
    pub fn spans(&self) -> &[AppliedFaceSpan] {
        &self.spans
    }
    pub fn referenced_bytes(&self) -> u64 {
        self.bytes
    }
}
#[derive(Clone, Copy)]
struct Request {
    triangle: u32,
    edge: [u32; 2],
    point: u32,
    vertex: u32,
}
fn recertify<F: Fn() -> bool>(
    source: &ConformingSurfaceMesh,
    original: u32,
    ids: [u32; 3],
    vertices: &[AppliedFaceVertex],
    bits: u8,
    work: &mut Work<'_, F>,
) -> Result<AppliedFaceTriangle, SurfaceError> {
    work.charge(1)?;
    let triangle = *source
        .triangles
        .get(original as usize)
        .ok_or(SurfaceError::Geometry)?;
    let [a, b, c] = ids.map(|id| vertices.get(id as usize).and_then(|v| v.position()));
    let coords = [
        a.ok_or(SurfaceError::Geometry)?,
        b.ok_or(SurfaceError::Geometry)?,
        c.ok_or(SurfaceError::Geometry)?,
    ];
    let chart = chart(source);
    let field = chart
        .graph()
        .cell()
        .interpolant()
        .ok_or(SurfaceError::Geometry)?;
    let certificate = evaluate_grid(
        field.values(),
        usize::from(chart.axis()),
        triangle.normal,
        coords,
        REFINED_GRID,
        work.cancel,
    )?;
    let state = if let SurfaceFacetCertificate::Certified {
        numerator,
        denominator,
    } = certificate
        && numerator
            .checked_mul(1u128 << bits)
            .ok_or(SurfaceError::Limit)?
            <= denominator
    {
        AppliedTriangleState::WithinTolerance {
            numerator,
            denominator,
        }
    } else {
        AppliedTriangleState::Unresolved { certificate }
    };
    Ok(AppliedFaceTriangle {
        vertices: ids,
        source_triangle: original,
        source_facet: triangle.source_facet,
        state,
    })
}
fn fan(
    triangle: [u32; 3],
    edge: [u32; 2],
    requests: &[Request],
    mut emit: impl FnMut([u32; 3]) -> Result<(), SurfaceError>,
) -> Result<(), SurfaceError> {
    if requests.iter().any(|request| request.edge != edge) {
        return Err(SurfaceError::Geometry);
    }
    let (a, b, c) = oriented_edge(triangle, edge[0], edge[1]).ok_or(SurfaceError::Geometry)?;
    let forward = a == edge[0];
    let mut previous = a;
    for i in 0..requests.len() {
        let next = requests[if forward { i } else { requests.len() - 1 - i }].vertex;
        emit([previous, next, c])?;
        previous = next;
    }
    emit([previous, b, c])
}
fn build_triangles<F: Fn() -> bool>(
    source: &ConformingSurfaceMesh,
    vertices: &[AppliedFaceVertex],
    requests: &mut [Request],
    count: usize,
    bits: u8,
    work: &mut Work<'_, F>,
) -> Result<Vec<AppliedFaceTriangle>, SurfaceError> {
    // Count comparisons and shifts explicitly, including cancellation checks.
    for i in 1..requests.len() {
        let value = requests[i];
        let key = (value.triangle, value.edge, value.point);
        let mut j = i;
        while j > 0 {
            work.charge(1)?;
            let previous = requests[j - 1];
            if (previous.triangle, previous.edge, previous.point) <= key {
                break;
            }
            requests[j] = previous;
            j -= 1;
        }
        requests[j] = value;
    }
    let mut result = exact(count)?;
    let mut at = 0usize;
    for (index, triangle) in source.triangles.iter().enumerate() {
        work.charge(1)?;
        let id = u32::try_from(index).map_err(|_| SurfaceError::Limit)?;
        let begin = at;
        while at < requests.len() && requests[at].triangle == id {
            work.charge(1)?;
            at += 1;
        }
        if begin == at {
            result.push(recertify(
                source,
                id,
                triangle.vertices,
                vertices,
                bits,
                work,
            )?);
            continue;
        }
        let group = &requests[begin..at];
        let edge = group[0].edge;
        work.charge(group.len())?;
        fan(triangle.vertices, edge, group, |ids| {
            result.push(recertify(source, id, ids, vertices, bits, work)?);
            Ok(())
        })?;
    }
    if at != requests.len() || result.len() != count {
        return Err(SurfaceError::Geometry);
    }
    Ok(result)
}
impl SharedFacePartition {
    /// Apply all representable split requests to both sides in one transaction.
    /// Every resulting triangle is recertified at its side's requested tolerance.
    /// Uncovered spans remain absent; this is not a whole-grid seam certificate.
    pub fn apply(
        self: &Arc<Self>,
        limits: AppliedFaceLimits,
        cancel: impl Fn() -> bool,
    ) -> Result<AppliedFacePair, SurfaceError> {
        self.apply_with_budget(limits, MemoryBudget::volumes(), cancel)
    }
    pub(in crate::surface) fn apply_with_budget(
        self: &Arc<Self>,
        limits: AppliedFaceLimits,
        budget: Arc<MemoryBudget>,
        cancel: impl Fn() -> bool,
    ) -> Result<AppliedFacePair, SurfaceError> {
        check(&cancel)?;
        if limits.tolerance_bits.iter().any(|bits| *bits > 16)
            || limits.max_vertices_per_cell > MAX_CELL_VERTICES
            || limits.max_triangles_per_cell > MAX_CELL_TRIANGLES
            || limits.max_operations == 0
            || limits.max_operations > MAX_WORK
            || limits.max_bytes > MAX_OUTPUT
        {
            return Err(SurfaceError::Limit);
        }
        let mut work = Work {
            used: 0,
            limit: limits.max_operations,
            cancel: &cancel,
        };
        let mut split_counts = [0usize; 2];
        for point in &self.points {
            work.charge(1)?;
            for (side, place) in point.placements.iter().enumerate() {
                split_counts[side] += usize::from(matches!(place, FacePlacement::Split { .. }));
            }
        }
        let mut vertex_counts = [0usize; 2];
        let mut triangle_counts = [0usize; 2];
        for side in 0..2 {
            vertex_counts[side] = self.inputs[side]
                .vertices
                .len()
                .checked_add(split_counts[side])
                .ok_or(SurfaceError::Limit)?;
            triangle_counts[side] = self.inputs[side]
                .triangles
                .len()
                .checked_add(split_counts[side])
                .ok_or(SurfaceError::Limit)?;
            if vertex_counts[side] > limits.max_vertices_per_cell
                || triangle_counts[side] > limits.max_triangles_per_cell
            {
                return Err(SurfaceError::Limit);
            }
        }
        let bytes = size(
            &[
                (
                    vertex_counts[0] + vertex_counts[1],
                    std::mem::size_of::<AppliedFaceVertex>(),
                ),
                (
                    triangle_counts[0] + triangle_counts[1],
                    std::mem::size_of::<AppliedFaceTriangle>(),
                ),
                (self.points.len(), std::mem::size_of::<[Option<u32>; 2]>()),
                (self.spans.len(), std::mem::size_of::<AppliedFaceSpan>()),
            ],
            std::mem::size_of::<AppliedFacePair>(),
        )?;
        if bytes > limits.max_bytes {
            return Err(SurfaceError::Limit);
        }
        let scratch = size(
            &[(
                split_counts[0] + split_counts[1],
                std::mem::size_of::<Request>(),
            )],
            2 * std::mem::size_of::<Vec<Request>>(),
        )?;
        let reservation = budget.reserve(bytes).ok_or(SurfaceError::Memory)?;
        let _scratch = budget.reserve(scratch).ok_or(SurfaceError::Memory)?;
        let mut vertices = [exact(vertex_counts[0])?, exact(vertex_counts[1])?];
        let mut requests = [exact(split_counts[0])?, exact(split_counts[1])?];
        let mut point_vertices = exact(self.points.len())?;
        let mut spans = exact(self.spans.len())?;
        for (side, output) in vertices.iter_mut().enumerate() {
            for (id, vertex) in self.inputs[side].vertices.iter().enumerate() {
                work.charge(1)?;
                output.push(AppliedFaceVertex::Retained {
                    id: u32::try_from(id).map_err(|_| SurfaceError::Limit)?,
                    position: vertex.position(),
                });
            }
        }
        for (i, point) in self.points.iter().enumerate() {
            work.charge(1)?;
            let point_id = u32::try_from(i).map_err(|_| SurfaceError::Limit)?;
            let mut mapped = [None; 2];
            for side in 0..2 {
                mapped[side] = match point.placements[side] {
                    FacePlacement::Absent => None,
                    FacePlacement::Existing { vertex } => {
                        if self.inputs[side]
                            .vertices
                            .get(vertex as usize)
                            .and_then(|v| v.position())
                            .is_none()
                        {
                            return Err(SurfaceError::Geometry);
                        }
                        Some(vertex)
                    }
                    FacePlacement::Split {
                        triangle,
                        edge,
                        position,
                    } => {
                        let id =
                            u32::try_from(vertices[side].len()).map_err(|_| SurfaceError::Limit)?;
                        vertices[side].push(AppliedFaceVertex::Split {
                            point: point_id,
                            edge,
                            position,
                        });
                        requests[side].push(Request {
                            triangle,
                            edge,
                            point: point_id,
                            vertex: id,
                        });
                        Some(id)
                    }
                };
            }
            point_vertices.push(mapped);
        }
        let [v0, v1] = vertices;
        let [mut r0, mut r1] = requests;
        let cells = [
            AppliedFaceCell {
                triangles: build_triangles(
                    &self.inputs[0],
                    &v0,
                    &mut r0,
                    triangle_counts[0],
                    limits.tolerance_bits[0],
                    &mut work,
                )?,
                vertices: v0,
            },
            AppliedFaceCell {
                triangles: build_triangles(
                    &self.inputs[1],
                    &v1,
                    &mut r1,
                    triangle_counts[1],
                    limits.tolerance_bits[1],
                    &mut work,
                )?,
                vertices: v1,
            },
        ];
        for (id, span) in self.spans.iter().enumerate() {
            work.charge(1)?;
            let a = *point_vertices
                .get(span.points[0] as usize)
                .ok_or(SurfaceError::Geometry)?;
            let b = *point_vertices
                .get(span.points[1] as usize)
                .ok_or(SurfaceError::Geometry)?;
            let mut segments = [None; 2];
            for side in 0..2 {
                if matches!(span.coverage[side], FaceSpanCoverage::Present { .. }) {
                    segments[side] = Some([
                        a[side].ok_or(SurfaceError::Geometry)?,
                        b[side].ok_or(SurfaceError::Geometry)?,
                    ]);
                }
            }
            spans.push(AppliedFaceSpan {
                partition_span: u32::try_from(id).map_err(|_| SurfaceError::Limit)?,
                segments,
            });
        }
        check(&cancel)?;
        Ok(AppliedFacePair {
            input: Arc::clone(self),
            cells,
            point_vertices,
            spans,
            bytes,
            _reservation: reservation,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ordered_multiple_splits_preserve_both_edge_orientations() {
        let requests = [
            Request {
                triangle: 0,
                edge: [0, 1],
                point: 2,
                vertex: 3,
            },
            Request {
                triangle: 0,
                edge: [0, 1],
                point: 3,
                vertex: 4,
            },
        ];
        let mut output = Vec::new();
        fan([0, 1, 2], [0, 1], &requests, |ids| {
            output.push(ids);
            Ok(())
        })
        .unwrap();
        assert_eq!(output, [[0, 3, 2], [3, 4, 2], [4, 1, 2]]);
        output.clear();
        fan([1, 0, 2], [0, 1], &requests, |ids| {
            output.push(ids);
            Ok(())
        })
        .unwrap();
        assert_eq!(output, [[1, 4, 2], [4, 3, 2], [3, 0, 2]]);
    }
}
