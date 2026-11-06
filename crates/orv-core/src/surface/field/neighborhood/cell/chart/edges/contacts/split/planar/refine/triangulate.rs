// SPDX-License-Identifier: GPL-3.0-or-later
//! Exact shared-slice planar triangulation with retained source constraints.
use super::*;
mod lift;
pub use lift::{
    AdaptiveFacetLimits, AdaptiveFacetMesh, AdaptiveFacetState, AdaptiveFacetTriangle,
    AdaptiveStopReason, BoundedSurfaceFacet, BoundedSurfaceVertex, CellAxisBounds,
    ConformingFacetState, ConformingStopReason, ConformingSurfaceLimits, ConformingSurfaceMesh,
    ConformingTriangle, ConformingVertex, EnclosedSurfaceGraph, FacePartitionLimits,
    FacePartitionPoint, FacePartitionSpan, FacePlacement, FaceSpanCoverage, SharedFacePartition,
    SurfaceEnclosureLimits, SurfaceFacetCertificate,
};

const MAX_TRIANGLES: usize = 262_144;
const MAX_BYTES: u64 = 16 * 1024 * 1024;
const MAX_PIECES: usize = 262_144;
const MAX_SCRATCH: u64 = 64 * 1024 * 1024;
const MAX_COMPARISONS: usize = 32_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlanarTriangulationLimits {
    pub max_triangles: usize,
    /// Additional retained typed storage; input and scratch are separate.
    pub max_bytes: u64,
}
impl Default for PlanarTriangulationLimits {
    fn default() -> Self {
        Self {
            max_triangles: MAX_TRIANGLES,
            max_bytes: MAX_BYTES,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlanarTriangle {
    /// Exact input vertex IDs in counterclockwise chart order.
    pub vertices: [u32; 3],
    pub region: u32,
}
/// Source-bound planar triangles. This does not lift or certify a 3D surface.
pub struct TriangulatedPlanarGraph {
    input: Arc<RefinedPlanarGraph>,
    triangles: Vec<PlanarTriangle>,
    bytes: u64,
    _reservation: Reservation,
}
impl TriangulatedPlanarGraph {
    pub fn input(&self) -> &Arc<RefinedPlanarGraph> {
        &self.input
    }
    pub fn triangles(&self) -> &[PlanarTriangle] {
        &self.triangles
    }
    /// Enclose the original field's zero fiber at an exact mesh vertex.
    /// Empty and whole fibers remain explicit; this does not lift a triangle.
    pub fn vertex_fiber_bounds(
        &self,
        id: u32,
        bits: u8,
        cancel: impl Fn() -> bool,
    ) -> Result<CellFiberBounds, SurfaceError> {
        let point = *self.input.vertex(id).ok_or(SurfaceError::Geometry)?;
        self.input
            .input()
            .input()
            .edges()
            .chart()
            .fiber_bounds(point, bits, cancel)
            .map_err(error)
    }
    pub fn referenced_bytes(&self) -> u64 {
        self.bytes
    }
}
#[derive(Clone, Copy)]
struct SweepEdge {
    slab: usize,
    left: u32,
    right: u32,
    above: Option<u32>,
    below: Option<u32>,
}
fn vertex(input: &RefinedPlanarGraph, id: u32) -> Result<&[ExactCoordinate; 2], SurfaceError> {
    input.vertex(id).ok_or(SurfaceError::Geometry)
}
fn vertex_order(input: &RefinedPlanarGraph, a: u32, b: u32) -> Result<Ordering, SurfaceError> {
    let a = vertex(input, a)?;
    let b = vertex(input, b)?;
    let x = cmp(a[0], b[0])?;
    if x == Ordering::Equal {
        cmp(a[1], b[1])
    } else {
        Ok(x)
    }
}
fn sort_vertices(
    input: &RefinedPlanarGraph,
    ids: &mut Vec<u32>,
    tmp: &mut Vec<u32>,
    cancel: &impl Fn() -> bool,
) -> Result<(), SurfaceError> {
    let n = ids.len();
    let mut width = 1usize;
    while width < n {
        let mut start = 0;
        while start < n {
            check(cancel)?;
            let middle = (start + width).min(n);
            let end = (middle + width).min(n);
            let (mut a, mut b) = (start, middle);
            for slot in &mut tmp[start..end] {
                check(cancel)?;
                if a < middle
                    && (b == end || vertex_order(input, ids[a], ids[b])? != Ordering::Greater)
                {
                    *slot = ids[a];
                    a += 1;
                } else {
                    *slot = ids[b];
                    b += 1;
                }
            }
            start = end;
        }
        std::mem::swap(ids, tmp);
        width = width.checked_mul(2).ok_or(SurfaceError::Limit)?;
    }
    for pair in ids.windows(2) {
        check(cancel)?;
        if vertex_order(input, pair[0], pair[1])? != Ordering::Less {
            return Err(SurfaceError::Geometry);
        }
    }
    Ok(())
}
fn slice_index(
    cuts: &[ExactCoordinate],
    x: ExactCoordinate,
    cancel: &impl Fn() -> bool,
) -> Result<usize, SurfaceError> {
    let (mut lo, mut hi) = (0, cuts.len());
    while lo < hi {
        check(cancel)?;
        let mid = lo + (hi - lo) / 2;
        match cmp(cuts[mid], x)? {
            Ordering::Less => lo = mid + 1,
            Ordering::Greater => hi = mid,
            Ordering::Equal => return Ok(mid),
        }
    }
    Err(SurfaceError::Geometry)
}
fn sweep_order(
    input: &RefinedPlanarGraph,
    a: SweepEdge,
    b: SweepEdge,
) -> Result<Ordering, SurfaceError> {
    let order = cmp(vertex(input, a.left)?[1], vertex(input, b.left)?[1])?;
    if order == Ordering::Equal {
        cmp(vertex(input, a.right)?[1], vertex(input, b.right)?[1])
    } else {
        Ok(order)
    }
}
fn interval_positions(
    input: &RefinedPlanarGraph,
    lower: SweepEdge,
    upper: SweepEdge,
    positions: &[usize],
    slice_starts: &[usize],
) -> Result<([usize; 2], [usize; 2], u32), SurfaceError> {
    let region = lower
        .above
        .filter(|v| Some(*v) == upper.below)
        .ok_or(SurfaceError::Geometry)?;
    if region as usize >= input.input.regions.len() {
        return Err(SurfaceError::Geometry);
    }
    let l = [
        *positions
            .get(lower.left as usize)
            .ok_or(SurfaceError::Geometry)?,
        *positions
            .get(upper.left as usize)
            .ok_or(SurfaceError::Geometry)?,
    ];
    let r = [
        *positions
            .get(lower.right as usize)
            .ok_or(SurfaceError::Geometry)?,
        *positions
            .get(upper.right as usize)
            .ok_or(SurfaceError::Geometry)?,
    ];
    let slab = lower.slab;
    if upper.slab != slab
        || slab + 2 >= slice_starts.len()
        || l[0] > l[1]
        || r[0] > r[1]
        || l[0] < slice_starts[slab]
        || l[1] >= slice_starts[slab + 1]
        || r[0] < slice_starts[slab + 1]
        || r[1] >= slice_starts[slab + 2]
        || (l[0] == l[1] && r[0] == r[1])
    {
        return Err(SurfaceError::Geometry);
    }
    Ok((l, r, region))
}
struct SliceDirectory<'a> {
    ids: &'a [u32],
    positions: &'a [usize],
    starts: &'a [usize],
    sweeps: &'a [SweepEdge],
}
fn process(
    input: &RefinedPlanarGraph,
    directory: &SliceDirectory<'_>,
    limits: PlanarTriangulationLimits,
    cancel: &impl Fn() -> bool,
    mut emit: impl FnMut(PlanarTriangle) -> Result<(), SurfaceError>,
) -> Result<usize, SurfaceError> {
    let SliceDirectory {
        ids,
        positions,
        starts,
        sweeps,
    } = *directory;
    let mut cursor = 0usize;
    let mut count = 0usize;
    for slab in 0..starts.len() - 2 {
        check(cancel)?;
        let begin = cursor;
        while cursor < sweeps.len() && sweeps[cursor].slab == slab {
            cursor += 1;
        }
        let bin = &sweeps[begin..cursor];
        if bin.len() < 2 || bin[0].below.is_some() || bin[bin.len() - 1].above.is_some() {
            return Err(SurfaceError::Geometry);
        }
        for pair in bin.windows(2) {
            check(cancel)?;
            let (left, right, region) =
                interval_positions(input, pair[0], pair[1], positions, starts)?;
            let n = (left[1] - left[0])
                .checked_add(right[1] - right[0])
                .ok_or(SurfaceError::Limit)?;
            count = count.checked_add(n).ok_or(SurfaceError::Limit)?;
            if count > limits.max_triangles {
                return Err(SurfaceError::Limit);
            }
            let r0 = *ids.get(right[0]).ok_or(SurfaceError::Geometry)?;
            let ln = *ids.get(left[1]).ok_or(SurfaceError::Geometry)?;
            for i in left[0]..left[1] {
                check(cancel)?;
                emit(PlanarTriangle {
                    vertices: [ids[i], r0, ids[i + 1]],
                    region,
                })?;
            }
            for i in right[0]..right[1] {
                check(cancel)?;
                emit(PlanarTriangle {
                    vertices: [ln, ids[i], ids[i + 1]],
                    region,
                })?;
            }
        }
    }
    if cursor != sweeps.len() {
        return Err(SurfaceError::Geometry);
    }
    check(cancel)?;
    Ok(count)
}
impl RefinedPlanarGraph {
    /// Triangulate every chart slab using exact slice order and face ownership.
    /// Vertical slice vertices, including isolated constraints, are retained in
    /// both incident slabs. No 3D lift, physical error or seam is implied.
    pub fn triangulate(
        self: &Arc<Self>,
        limits: PlanarTriangulationLimits,
        cancel: impl Fn() -> bool,
    ) -> Result<TriangulatedPlanarGraph, SurfaceError> {
        self.triangulate_with_budget(limits, MemoryBudget::volumes(), cancel)
    }
    pub(in crate::surface) fn triangulate_with_budget(
        self: &Arc<Self>,
        limits: PlanarTriangulationLimits,
        budget: Arc<MemoryBudget>,
        cancel: impl Fn() -> bool,
    ) -> Result<TriangulatedPlanarGraph, SurfaceError> {
        check(&cancel)?;
        if limits.max_triangles > MAX_TRIANGLES
            || limits.max_bytes > MAX_BYTES
            || self.indices.len() > MAX_PIECES
        {
            return Err(SurfaceError::Limit);
        }
        let count = self.vertex_count();
        let cuts = self.curves.slice_coordinates();
        if count > RefinementLimits::MAX_VERTICES
            || cuts.len() < 2
            || cuts.len() > RefinementLimits::MAX_VERTICES
        {
            return Err(SurfaceError::Geometry);
        }
        let scratch = size(
            &[
                (
                    count,
                    2 * std::mem::size_of::<u32>() + std::mem::size_of::<usize>(),
                ),
                (cuts.len() + 1, std::mem::size_of::<usize>()),
                (self.indices.len(), std::mem::size_of::<SweepEdge>()),
            ],
            5 * std::mem::size_of::<Vec<u32>>() + std::mem::size_of::<SliceDirectory<'_>>(),
        )?;
        if scratch > MAX_SCRATCH {
            return Err(SurfaceError::Limit);
        }
        let _scratch = budget.reserve(scratch).ok_or(SurfaceError::Memory)?;
        let mut ids = exact(count)?;
        let mut tmp = exact(count)?;
        let mut positions = exact(count)?;
        let mut starts = exact(cuts.len() + 1)?;
        let mut sweeps = exact(self.indices.len())?;
        for id in 0..count {
            ids.push(u32::try_from(id).map_err(|_| SurfaceError::Limit)?);
        }
        tmp.resize(count, 0);
        positions.resize(count, 0usize);
        sort_vertices(self, &mut ids, &mut tmp, &cancel)?;
        for (at, id) in ids.iter().copied().enumerate() {
            positions[id as usize] = at;
        }
        let mut at = 0usize;
        for u in cuts {
            check(&cancel)?;
            starts.push(at);
            while at < ids.len() && cmp(vertex(self, ids[at])?[0], *u)? == Ordering::Equal {
                at += 1;
            }
        }
        starts.push(at);
        if at != count || starts.windows(2).any(|w| w[0] == w[1]) {
            return Err(SurfaceError::Geometry);
        }
        for (id, e) in self.input.edges.iter().enumerate() {
            check(&cancel)?;
            let half = self
                .input
                .halves
                .get(2 * id)
                .ok_or(SurfaceError::Geometry)?;
            let twin = self
                .input
                .halves
                .get(2 * id + 1)
                .ok_or(SurfaceError::Geometry)?;
            if [half.from, half.to] != e.nodes
                || twin.twin != 2 * id as u32
                || half.twin != (2 * id + 1) as u32
            {
                return Err(SurfaceError::Geometry);
            }
            let path = self
                .edge_vertices(id as u32)
                .ok_or(SurfaceError::Geometry)?;
            for pair in path.windows(2) {
                check(&cancel)?;
                let x = cmp(vertex(self, pair[0])?[0], vertex(self, pair[1])?[0])?;
                if x == Ordering::Equal {
                    continue;
                }
                let (left, right, above, below) = if x == Ordering::Less {
                    (pair[0], pair[1], half.region, twin.region)
                } else {
                    (pair[1], pair[0], twin.region, half.region)
                };
                let l = slice_index(cuts, vertex(self, left)?[0], &cancel)?;
                let r = slice_index(cuts, vertex(self, right)?[0], &cancel)?;
                if r != l + 1 || sweeps.len() >= self.indices.len() {
                    return Err(SurfaceError::Geometry);
                }
                sweeps.push(SweepEdge {
                    slab: l,
                    left,
                    right,
                    above,
                    below,
                });
            }
        }
        sweeps.sort_unstable_by_key(|e| e.slab);
        check(&cancel)?;
        let mut begin = 0usize;
        let mut comparisons = 0usize;
        while begin < sweeps.len() {
            let slab = sweeps[begin].slab;
            let mut end = begin + 1;
            while end < sweeps.len() && sweeps[end].slab == slab {
                end += 1;
            }
            for i in begin + 1..end {
                let mut j = i;
                while j > begin {
                    check(&cancel)?;
                    comparisons = comparisons.checked_add(1).ok_or(SurfaceError::Limit)?;
                    if comparisons > MAX_COMPARISONS {
                        return Err(SurfaceError::Limit);
                    }
                    match sweep_order(self, sweeps[j - 1], sweeps[j])? {
                        Ordering::Less => break,
                        Ordering::Greater => sweeps.swap(j - 1, j),
                        Ordering::Equal => return Err(SurfaceError::Geometry),
                    }
                    j -= 1;
                }
            }
            for pair in sweeps[begin..end].windows(2) {
                if cmp(
                    vertex(self, pair[0].right)?[1],
                    vertex(self, pair[1].right)?[1],
                )? == Ordering::Greater
                {
                    return Err(SurfaceError::Geometry);
                }
            }
            begin = end;
        }
        let directory = SliceDirectory {
            ids: &ids,
            positions: &positions,
            starts: &starts,
            sweeps: &sweeps,
        };
        let expected = process(self, &directory, limits, &cancel, |_| Ok(()))?;
        let bytes = size(
            &[(expected, std::mem::size_of::<PlanarTriangle>())],
            std::mem::size_of::<TriangulatedPlanarGraph>(),
        )?;
        if bytes > limits.max_bytes {
            return Err(SurfaceError::Limit);
        }
        let reservation = budget.reserve(bytes).ok_or(SurfaceError::Memory)?;
        let mut triangles = exact(expected)?;
        let actual = process(self, &directory, limits, &cancel, |t| {
            if triangles.len() >= expected {
                return Err(SurfaceError::Geometry);
            }
            triangles.push(t);
            Ok(())
        })?;
        if actual != expected || triangles.len() != expected {
            return Err(SurfaceError::Geometry);
        }
        check(&cancel)?;
        Ok(TriangulatedPlanarGraph {
            input: Arc::clone(self),
            triangles,
            bytes,
            _reservation: reservation,
        })
    }
}
