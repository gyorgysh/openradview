// SPDX-License-Identifier: GPL-3.0-or-later
//! Exact source-face partition exchange between two neighboring cell meshes.
use super::*;
mod apply;
pub use apply::{
    AppliedFaceCell, AppliedFaceLimits, AppliedFacePair, AppliedFaceSpan, AppliedFaceTriangle,
    AppliedFaceVertex, AppliedTriangleState,
};

const MAX_EDGES: usize = 4096;
const MAX_POINTS: usize = 8192;
const MAX_WORK: usize = 2_000_000;
const MAX_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FacePartitionLimits {
    pub max_edges_per_cell: usize,
    pub max_points: usize,
    pub max_operations: usize,
    /// Additional retained output; temporary arrays are admitted separately.
    pub max_bytes: u64,
}
impl Default for FacePartitionLimits {
    fn default() -> Self {
        Self {
            max_edges_per_cell: MAX_EDGES,
            max_points: MAX_POINTS,
            max_operations: MAX_WORK,
            max_bytes: MAX_BYTES,
        }
    }
}
/// A source-face point may already be present, require a split, or have no
/// candidate-facet coverage on this side. A split is a request, not an edit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FacePlacement {
    Absent,
    Existing {
        vertex: u32,
    },
    Split {
        triangle: u32,
        /// Edge endpoints ordered by increasing exact face coordinate.
        edge: [u32; 2],
        position: [u32; 3],
    },
}
/// Candidate-edge coverage of one open interval between consecutive exact points.
/// Presence does not certify the underlying contour or a completed seam.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FaceSpanCoverage {
    Absent,
    Present { triangle: u32, edge: [u32; 2] },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FacePartitionSpan {
    /// Indices into `SharedFacePartition::points`, in increasing exact order.
    pub points: [u32; 2],
    pub coverage: [FaceSpanCoverage; 2],
}
#[derive(Clone, Copy, Debug)]
pub struct FacePartitionPoint {
    /// Exact source-face coordinate along the non-projection transverse axis.
    pub coordinate: ExactCoordinate,
    pub placements: [FacePlacement; 2],
}
/// Retains both immutable meshes and all exact candidate-face partition points.
/// This is a split-request directory; no facet, contour or seam is modified.
pub struct SharedFacePartition {
    inputs: [Arc<ConformingSurfaceMesh>; 2],
    face: FaceKey,
    projection_axis: u8,
    points: Vec<FacePartitionPoint>,
    spans: Vec<FacePartitionSpan>,
    bytes: u64,
    _reservation: Reservation,
}
impl SharedFacePartition {
    pub fn inputs(&self) -> &[Arc<ConformingSurfaceMesh>; 2] {
        &self.inputs
    }
    pub fn face(&self) -> FaceKey {
        self.face
    }
    pub fn projection_axis(&self) -> u8 {
        self.projection_axis
    }
    pub fn points(&self) -> &[FacePartitionPoint] {
        &self.points
    }
    pub fn spans(&self) -> &[FacePartitionSpan] {
        &self.spans
    }
    pub fn referenced_bytes(&self) -> u64 {
        self.bytes
    }
}
#[derive(Clone, Copy)]
struct FaceKeyPoint {
    coordinate: ExactCoordinate,
    representative: u32,
}
#[derive(Clone, Copy)]
struct FaceEdge {
    lower: ExactCoordinate,
    upper: ExactCoordinate,
    vertices: [u32; 2],
    representatives: [u32; 2],
    triangle: u32,
    normal: i8,
}
struct Work<'a, F: Fn() -> bool> {
    used: usize,
    limit: usize,
    cancel: &'a F,
}
impl<F: Fn() -> bool> Work<'_, F> {
    fn charge(&mut self, amount: usize) -> Result<(), SurfaceError> {
        check(self.cancel)?;
        self.used = self.used.checked_add(amount).ok_or(SurfaceError::Limit)?;
        if self.used > self.limit {
            return Err(SurfaceError::Limit);
        }
        Ok(())
    }
    fn compare(
        &mut self,
        a: ExactCoordinate,
        b: ExactCoordinate,
    ) -> Result<Ordering, SurfaceError> {
        self.charge(1)?;
        a.compare(b).map_err(error)
    }
}
fn chart(mesh: &ConformingSurfaceMesh) -> &CellProjectionChart {
    mesh.input.input().input().input().input().edges().chart()
}
fn coordinate(n: u32) -> Result<ExactCoordinate, SurfaceError> {
    let value = Rational::new(i128::from(n), i128::from(REFINED_GRID)).map_err(error)?;
    ExactCoordinate::from_rational(value)
        .map_err(error)?
        .ok_or(SurfaceError::Geometry)
}
fn exact_along(
    mesh: &ConformingSurfaceMesh,
    id: u32,
    face_axis: usize,
) -> Result<ExactCoordinate, SurfaceError> {
    let vertex = *mesh
        .vertices
        .get(id as usize)
        .ok_or(SurfaceError::Geometry)?;
    let c = chart(mesh);
    let transverse = c.transverse_axes();
    let along = 1 - transverse
        .iter()
        .position(|a| *a == face_axis)
        .ok_or(SurfaceError::Geometry)?;
    match vertex {
        ConformingVertex::Source { id, .. } => mesh
            .input
            .input()
            .input()
            .vertex(id)
            .map(|p| p[along])
            .ok_or(SurfaceError::Geometry),
        ConformingVertex::Midpoint { position, .. } => coordinate(position[transverse[along]]),
    }
}
fn canonical(
    c: &CellProjectionChart,
    face_axis: usize,
    upper: bool,
    along: ExactCoordinate,
    normal: i8,
    representative: u32,
    cancel: &impl Fn() -> bool,
) -> Result<[u32; 3], SurfaceError> {
    check(cancel)?;
    let transverse = c.transverse_axes();
    let side = transverse
        .iter()
        .position(|a| *a == face_axis)
        .ok_or(SurfaceError::Geometry)?;
    let mut point = [along; 2];
    point[side] = endpoint(upper)?;
    let fiber = c.fiber_bounds(point, 16, cancel).map_err(error)?;
    let height = midpoint_height(fiber, normal)?.map_err(|_| SurfaceError::Geometry)?;
    if representative > REFINED_GRID {
        return Err(SurfaceError::Geometry);
    }
    let mut p = [0; 3];
    p[face_axis] = if upper { REFINED_GRID } else { 0 };
    p[transverse[1 - side]] = representative;
    p[usize::from(c.axis())] = height;
    Ok(p)
}
fn verify_representative(
    mesh: &ConformingSurfaceMesh,
    id: u32,
    along: ExactCoordinate,
    representative: u32,
    cancel: &impl Fn() -> bool,
) -> Result<(), SurfaceError> {
    let vertex = mesh
        .vertices
        .get(id as usize)
        .ok_or(SurfaceError::Geometry)?;
    match vertex {
        ConformingVertex::Source { .. } => {
            let interval = coordinate_bounds(along, 16, cancel)?;
            let twice = interval
                .denominator
                .checked_mul(2)
                .ok_or(SurfaceError::Geometry)?;
            if twice == 0 || !REFINED_GRID.is_multiple_of(twice) {
                return Err(SurfaceError::Geometry);
            }
            let expected = interval
                .lower
                .checked_add(interval.upper)
                .and_then(|n| n.checked_mul(REFINED_GRID / twice))
                .ok_or(SurfaceError::Geometry)?;
            if representative != expected {
                return Err(SurfaceError::Geometry);
            }
        }
        ConformingVertex::Midpoint { .. } => {
            if along.compare(coordinate(representative)?).map_err(error)? != Ordering::Equal {
                return Err(SurfaceError::Geometry);
            }
        }
    }
    Ok(())
}
fn insert_edge<F: Fn() -> bool>(
    edges: &mut Vec<FaceEdge>,
    edge: FaceEdge,
    work: &mut Work<'_, F>,
) -> Result<(), SurfaceError> {
    let mut at = 0;
    while at < edges.len() && work.compare(edges[at].lower, edge.lower)? == Ordering::Less {
        at += 1;
    }
    work.charge(edges.len() - at)?;
    if edges.len() == edges.capacity() {
        return Err(SurfaceError::Limit);
    }
    edges.insert(at, edge);
    Ok(())
}
fn collect_edges<F: Fn() -> bool>(
    mesh: &ConformingSurfaceMesh,
    face_axis: usize,
    upper: bool,
    edges: &mut Vec<FaceEdge>,
    keys: &mut Vec<FaceKeyPoint>,
    work: &mut Work<'_, F>,
) -> Result<(), SurfaceError> {
    let boundary = if upper { REFINED_GRID } else { 0 };
    for (i, triangle) in mesh.triangles.iter().enumerate() {
        work.charge(1)?;
        for [a, b] in [
            [triangle.vertices[0], triangle.vertices[1]],
            [triangle.vertices[1], triangle.vertices[2]],
            [triangle.vertices[2], triangle.vertices[0]],
        ] {
            let pa = mesh
                .vertices
                .get(a as usize)
                .and_then(|v| v.position())
                .ok_or(SurfaceError::Geometry)?;
            let pb = mesh
                .vertices
                .get(b as usize)
                .and_then(|v| v.position())
                .ok_or(SurfaceError::Geometry)?;
            if pa[face_axis] != boundary || pb[face_axis] != boundary {
                continue;
            }
            let ca = exact_along(mesh, a, face_axis)?;
            let cb = exact_along(mesh, b, face_axis)?;
            let transverse = chart(mesh).transverse_axes();
            let along_axis = *transverse
                .iter()
                .find(|axis| **axis != face_axis)
                .ok_or(SurfaceError::Geometry)?;
            verify_representative(mesh, a, ca, pa[along_axis], work.cancel)?;
            verify_representative(mesh, b, cb, pb[along_axis], work.cancel)?;
            if canonical(
                chart(mesh),
                face_axis,
                upper,
                ca,
                triangle.normal,
                pa[along_axis],
                work.cancel,
            )? != pa
                || canonical(
                    chart(mesh),
                    face_axis,
                    upper,
                    cb,
                    triangle.normal,
                    pb[along_axis],
                    work.cancel,
                )? != pb
            {
                return Err(SurfaceError::Geometry);
            }
            let order = work.compare(ca, cb)?;
            if order == Ordering::Equal {
                return Err(SurfaceError::Geometry);
            }
            let (lower, upper_coord, vertices, representatives) = if order == Ordering::Less {
                (ca, cb, [a, b], [pa[along_axis], pb[along_axis]])
            } else {
                (cb, ca, [b, a], [pb[along_axis], pa[along_axis]])
            };
            if representatives[0] >= representatives[1] {
                return Err(SurfaceError::Geometry);
            }
            insert_edge(
                edges,
                FaceEdge {
                    lower,
                    upper: upper_coord,
                    vertices,
                    representatives,
                    triangle: u32::try_from(i).map_err(|_| SurfaceError::Limit)?,
                    normal: triangle.normal,
                },
                work,
            )?;
            for (coordinate, representative) in [(ca, pa[along_axis]), (cb, pb[along_axis])] {
                insert_key(
                    keys,
                    FaceKeyPoint {
                        coordinate,
                        representative,
                    },
                    work,
                )?;
            }
        }
    }
    for pair in edges.windows(2) {
        if work.compare(pair[0].upper, pair[1].lower)? == Ordering::Greater {
            return Err(SurfaceError::Geometry);
        }
    }
    Ok(())
}
fn insert_key<F: Fn() -> bool>(
    keys: &mut Vec<FaceKeyPoint>,
    value: FaceKeyPoint,
    work: &mut Work<'_, F>,
) -> Result<(), SurfaceError> {
    let (mut lo, mut hi) = (0, keys.len());
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        match work.compare(keys[mid].coordinate, value.coordinate)? {
            Ordering::Less => lo = mid + 1,
            Ordering::Greater => hi = mid,
            Ordering::Equal => {
                if keys[mid].representative != value.representative {
                    return Err(SurfaceError::Geometry);
                }
                return Ok(());
            }
        }
    }
    work.charge(keys.len() - lo)?;
    // Distinct exact points must stay distinguishable and ordered on the
    // output grid; otherwise a later split could create a collapsed edge.
    if (lo > 0 && keys[lo - 1].representative >= value.representative)
        || (lo < keys.len() && value.representative >= keys[lo].representative)
    {
        return Err(SurfaceError::Geometry);
    }
    if keys.len() == keys.capacity() {
        return Err(SurfaceError::Limit);
    }
    keys.insert(lo, value);
    Ok(())
}
fn placement<F: Fn() -> bool>(
    edges: &[FaceEdge],
    key: FaceKeyPoint,
    c: &CellProjectionChart,
    face_axis: usize,
    upper: bool,
    work: &mut Work<'_, F>,
) -> Result<FacePlacement, SurfaceError> {
    let mut found = FacePlacement::Absent;
    for edge in edges {
        let l = work.compare(key.coordinate, edge.lower)?;
        let h = work.compare(key.coordinate, edge.upper)?;
        if l == Ordering::Less {
            break;
        }
        if h == Ordering::Greater {
            continue;
        }
        let next = if l == Ordering::Equal {
            FacePlacement::Existing {
                vertex: edge.vertices[0],
            }
        } else if h == Ordering::Equal {
            FacePlacement::Existing {
                vertex: edge.vertices[1],
            }
        } else {
            if key.representative <= edge.representatives[0]
                || key.representative >= edge.representatives[1]
            {
                return Err(SurfaceError::Geometry);
            }
            FacePlacement::Split {
                triangle: edge.triangle,
                edge: edge.vertices,
                position: canonical(
                    c,
                    face_axis,
                    upper,
                    key.coordinate,
                    edge.normal,
                    key.representative,
                    work.cancel,
                )?,
            }
        };
        if found != FacePlacement::Absent && found != next {
            return Err(SurfaceError::Geometry);
        }
        found = next;
    }
    Ok(found)
}
fn span_coverage<F: Fn() -> bool>(
    edges: &[FaceEdge],
    lower: ExactCoordinate,
    upper: ExactCoordinate,
    work: &mut Work<'_, F>,
) -> Result<FaceSpanCoverage, SurfaceError> {
    let mut found = FaceSpanCoverage::Absent;
    for edge in edges {
        if work.compare(edge.lower, lower)? == Ordering::Greater {
            break;
        }
        if work.compare(edge.upper, upper)? == Ordering::Less {
            continue;
        }
        if found != FaceSpanCoverage::Absent {
            return Err(SurfaceError::Geometry);
        }
        found = FaceSpanCoverage::Present {
            triangle: edge.triangle,
            edge: edge.vertices,
        };
    }
    Ok(found)
}
fn location(mesh: &ConformingSurfaceMesh, p: FacePlacement) -> Option<[u32; 3]> {
    match p {
        FacePlacement::Absent => None,
        FacePlacement::Existing { vertex } => mesh
            .vertices
            .get(vertex as usize)
            .and_then(|v| v.position()),
        FacePlacement::Split { position, .. } => Some(position),
    }
}
impl ConformingSurfaceMesh {
    /// Build exact source-bound split requests for a shared transverse face.
    /// Cells must share source identity, face and projection axis. Candidate-only
    /// gaps remain Absent in points and spans; an empty partition does not prove coverage. Exact
    /// coordinates with incompatible representatives are refused. Applying
    /// requests and cross-cell conformance are separate.
    pub fn shared_face_partition(
        self: &Arc<Self>,
        other: &Arc<Self>,
        face_axis: usize,
        limits: FacePartitionLimits,
        cancel: impl Fn() -> bool,
    ) -> Result<SharedFacePartition, SurfaceError> {
        self.shared_face_partition_with_budget(
            other,
            face_axis,
            limits,
            MemoryBudget::volumes(),
            cancel,
        )
    }
    pub(in crate::surface) fn shared_face_partition_with_budget(
        self: &Arc<Self>,
        other: &Arc<Self>,
        face_axis: usize,
        limits: FacePartitionLimits,
        budget: Arc<MemoryBudget>,
        cancel: impl Fn() -> bool,
    ) -> Result<SharedFacePartition, SurfaceError> {
        check(&cancel)?;
        if face_axis >= 3
            || limits.max_edges_per_cell > MAX_EDGES
            || limits.max_points > MAX_POINTS
            || limits.max_operations == 0
            || limits.max_operations > MAX_WORK
            || limits.max_bytes > MAX_BYTES
        {
            return Err(SurfaceError::Limit);
        }
        let charts = [chart(self), chart(other)];
        let fields = charts.map(|c| c.graph().field());
        if fields[0].key() != fields[1].key()
            || fields[0].revision() != fields[1].revision()
            || !Arc::ptr_eq(fields[0].source(), fields[1].source())
            || charts[0].axis() != charts[1].axis()
            || usize::from(charts[0].axis()) == face_axis
        {
            return Err(SurfaceError::Geometry);
        }
        let origins = charts.map(|c| c.graph().cell().origin());
        if (0..3).any(|d| d != face_axis && origins[0][d] != origins[1][d]) {
            return Err(SurfaceError::Geometry);
        }
        let upper = if origins[0][face_axis].checked_add(1) == Some(origins[1][face_axis]) {
            [true, false]
        } else if origins[1][face_axis].checked_add(1) == Some(origins[0][face_axis]) {
            [false, true]
        } else {
            return Err(SurfaceError::Geometry);
        };
        let f0 = charts[0]
            .graph()
            .cell()
            .face(face_axis, upper[0])
            .ok_or(SurfaceError::Geometry)?
            .0;
        let f1 = charts[1]
            .graph()
            .cell()
            .face(face_axis, upper[1])
            .ok_or(SurfaceError::Geometry)?
            .0;
        if f0 != f1 {
            return Err(SurfaceError::Geometry);
        }
        let bytes = size(
            &[
                (limits.max_points, std::mem::size_of::<FacePartitionPoint>()),
                (
                    limits.max_points.saturating_sub(1),
                    std::mem::size_of::<FacePartitionSpan>(),
                ),
            ],
            std::mem::size_of::<SharedFacePartition>(),
        )?;
        if bytes > limits.max_bytes {
            return Err(SurfaceError::Limit);
        }
        let scratch = size(
            &[
                (
                    limits.max_edges_per_cell * 2,
                    std::mem::size_of::<FaceEdge>(),
                ),
                (limits.max_points, std::mem::size_of::<FaceKeyPoint>()),
            ],
            0,
        )?;
        let reservation = budget.reserve(bytes).ok_or(SurfaceError::Memory)?;
        let _scratch = budget.reserve(scratch).ok_or(SurfaceError::Memory)?;
        let mut edges0 = exact(limits.max_edges_per_cell)?;
        let mut edges1 = exact(limits.max_edges_per_cell)?;
        let mut keys = exact(limits.max_points)?;
        let mut points = exact(limits.max_points)?;
        let mut spans = exact(limits.max_points.saturating_sub(1))?;
        let mut work = Work {
            used: 0,
            limit: limits.max_operations,
            cancel: &cancel,
        };
        collect_edges(self, face_axis, upper[0], &mut edges0, &mut keys, &mut work)?;
        collect_edges(
            other,
            face_axis,
            upper[1],
            &mut edges1,
            &mut keys,
            &mut work,
        )?;
        for key in keys.iter().copied() {
            work.charge(1)?;
            let placements = [
                placement(&edges0, key, charts[0], face_axis, upper[0], &mut work)?,
                placement(&edges1, key, charts[1], face_axis, upper[1], &mut work)?,
            ];
            if let (Some(a), Some(b)) = (
                location(self, placements[0]),
                location(other, placements[1]),
            ) && (0..3).any(|d| d != face_axis && a[d] != b[d])
            {
                return Err(SurfaceError::Geometry);
            }
            points.push(FacePartitionPoint {
                coordinate: key.coordinate,
                placements,
            });
        }
        for (index, pair) in keys.windows(2).enumerate() {
            work.charge(1)?;
            let first = u32::try_from(index).map_err(|_| SurfaceError::Limit)?;
            let second = first.checked_add(1).ok_or(SurfaceError::Limit)?;
            spans.push(FacePartitionSpan {
                points: [first, second],
                coverage: [
                    span_coverage(&edges0, pair[0].coordinate, pair[1].coordinate, &mut work)?,
                    span_coverage(&edges1, pair[0].coordinate, pair[1].coordinate, &mut work)?,
                ],
            });
        }
        check(&cancel)?;
        Ok(SharedFacePartition {
            inputs: [Arc::clone(self), Arc::clone(other)],
            face: f0,
            projection_axis: charts[0].axis(),
            points,
            spans,
            bytes,
            _reservation: reservation,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rational(n: i128, d: i128) -> ExactCoordinate {
        ExactCoordinate::from_rational(Rational::new(n, d).unwrap())
            .unwrap()
            .unwrap()
    }

    #[test]
    fn interval_gap_remains_absent_even_when_both_endpoints_exist() {
        let edge = |lower, upper, triangle| FaceEdge {
            lower,
            upper,
            vertices: [triangle, triangle + 1],
            representatives: [0, 1],
            triangle,
            normal: 1,
        };
        let edges = [
            edge(rational(0, 1), rational(1, 4), 0),
            edge(rational(3, 4), rational(1, 1), 1),
        ];
        let mut work = Work {
            used: 0,
            limit: 100,
            cancel: &|| false,
        };
        assert_eq!(
            span_coverage(&edges, rational(1, 4), rational(3, 4), &mut work),
            Ok(FaceSpanCoverage::Absent)
        );
        assert_eq!(
            span_coverage(&edges, rational(0, 1), rational(1, 4), &mut work),
            Ok(FaceSpanCoverage::Present {
                triangle: 0,
                edge: [0, 1]
            })
        );
    }

    #[test]
    fn exact_keys_do_not_merge_distinct_coordinates_with_the_same_representative() {
        let a = rational((1 << 29) + 1, 1 << 30);
        let b = rational((1 << 29) + 2, 1 << 30);
        let midpoint = (REFINED_GRID / 2) + (REFINED_GRID >> 17);
        let mut keys = Vec::with_capacity(3);
        let mut work = Work {
            used: 0,
            limit: 100,
            cancel: &|| false,
        };
        insert_key(
            &mut keys,
            FaceKeyPoint {
                coordinate: b,
                representative: midpoint,
            },
            &mut work,
        )
        .unwrap();
        assert!(matches!(
            insert_key(
                &mut keys,
                FaceKeyPoint {
                    coordinate: a,
                    representative: midpoint
                },
                &mut work
            ),
            Err(SurfaceError::Geometry)
        ));
        assert_eq!(keys.len(), 1);
        assert!(matches!(
            insert_key(
                &mut keys,
                FaceKeyPoint {
                    coordinate: b,
                    representative: midpoint + 1
                },
                &mut work
            ),
            Err(SurfaceError::Geometry)
        ));
    }
}
