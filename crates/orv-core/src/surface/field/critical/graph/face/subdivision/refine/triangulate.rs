// SPDX-License-Identifier: GPL-3.0-or-later
//! Monotone region triangulation with exact predicates and retained constraints.
use super::*;
use crate::surface::field::critical::graph::compare::orientation;

const MAX_TRIANGLES: usize = 2 * MAX_VERTICES;
const MAX_TRIANGLE_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TriangulationLimits {
    pub max_triangles: usize,
    /// Additional retained typed storage; the input keeps its own reservation.
    pub max_bytes: u64,
}
impl Default for TriangulationLimits {
    fn default() -> Self {
        Self {
            max_triangles: MAX_TRIANGLES,
            max_bytes: MAX_TRIANGLE_BYTES,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RegionTriangle {
    /// Counterclockwise exact vertex IDs in the retained refinement.
    pub vertices: [u32; 3],
    pub region: u8,
}
pub struct TriangulatedSubdivision {
    input: Arc<RefinedSubdivision>,
    triangles: Vec<RegionTriangle>,
    bytes: u64,
    _reservation: Reservation,
}
impl TriangulatedSubdivision {
    pub fn subdivision(&self) -> &Arc<RefinedSubdivision> {
        &self.input
    }
    pub fn triangles(&self) -> &[RegionTriangle] {
        &self.triangles
    }
    /// Additional typed storage, excluding the separately admitted shared input.
    pub fn referenced_bytes(&self) -> u64 {
        self.bytes
    }
}
#[derive(Clone, Copy)]
struct Bounds {
    lower: [u64; 2],
    upper: [u64; 2],
}
#[derive(Clone, Copy)]
struct SweepVertex {
    id: u32,
    lower: bool,
}
struct Worker<'a, I, C> {
    input: &'a I,
    bounds: Vec<Bounds>,
    budget: Arc<MemoryBudget>,
    cancel: C,
    triangles: Vec<RegionTriangle>,
    expected: usize,
}
impl<I: Input, C: Fn() -> bool> Worker<'_, I, C> {
    fn compare(&self, a: u32, b: u32) -> Result<Ordering, SurfaceError> {
        check(&self.cancel)?;
        let a = self.input.vertex(a).ok_or(SurfaceError::Geometry)?;
        let b = self.input.vertex(b).ok_or(SurfaceError::Geometry)?;
        let u = a[0].compare(b[0]).map_err(geometry)?;
        if u == Ordering::Equal {
            a[1].compare(b[1]).map_err(geometry)
        } else {
            Ok(u)
        }
    }
    fn turn(&self, vertices: [u32; 3]) -> Result<i8, SurfaceError> {
        check(&self.cancel)?;
        let [Some(a), Some(b), Some(c)] = vertices.map(|id| self.bounds.get(id as usize).copied())
        else {
            return Err(SurfaceError::Geometry);
        };
        let difference = |a: Bounds, b: Bounds, axis: usize| {
            [
                i128::from(a.lower[axis]) - i128::from(b.upper[axis]),
                i128::from(a.upper[axis]) - i128::from(b.lower[axis]),
            ]
        };
        let product = |a: [i128; 2], b: [i128; 2]| {
            let v = [a[0] * b[0], a[0] * b[1], a[1] * b[0], a[1] * b[1]];
            [*v.iter().min().unwrap_or(&0), *v.iter().max().unwrap_or(&0)]
        };
        let p = product(difference(b, a, 0), difference(c, a, 1));
        let q = product(difference(b, a, 1), difference(c, a, 0));
        let (lo, hi) = (p[0] - q[1], p[1] - q[0]);
        if lo > 0 {
            return Ok(1);
        }
        if hi < 0 {
            return Ok(-1);
        }
        if lo == 0 && hi == 0 {
            return Ok(0);
        }
        let [Some(a), Some(b), Some(c)] = vertices.map(|id| self.input.vertex(id).copied()) else {
            return Err(SurfaceError::Geometry);
        };
        orientation([a, b, c], &self.budget, &self.cancel)
    }
    fn emit(&mut self, mut vertices: [u32; 3], region: u8) -> Result<(), SurfaceError> {
        match self.turn(vertices)? {
            -1 => vertices.swap(1, 2),
            1 => {}
            _ => return Err(SurfaceError::Geometry),
        }
        if self.triangles.len() >= self.expected {
            return Err(SurfaceError::Geometry);
        }
        self.triangles.push(RegionTriangle { vertices, region });
        Ok(())
    }
    fn region(
        &mut self,
        region: u8,
        ring: &[u32],
        sorted: &mut Vec<SweepVertex>,
        stack: &mut Vec<SweepVertex>,
    ) -> Result<(), SurfaceError> {
        // At each regular u, each bilinear sign restricts v to one interval.
        // Complete intersection events and coordinated chords preserve each
        // region's u-monotonicity. Lexicographic(u,v) supplies vertical ties.
        let n = ring.len();
        let (mut min, mut max) = (0, 0);
        for i in 1..n {
            if self.compare(ring[i], ring[min])? == Ordering::Less {
                min = i;
            }
            if self.compare(ring[i], ring[max])? == Ordering::Greater {
                max = i;
            }
        }
        let next = |i: usize| if i + 1 == n { 0 } else { i + 1 };
        let previous = |i: usize| if i == 0 { n - 1 } else { i - 1 };
        sorted.clear();
        stack.clear();
        sorted.push(SweepVertex {
            id: ring[min],
            lower: false,
        });
        let (mut lower, mut upper) = (next(min), previous(min));
        while lower != max || upper != max {
            check(&self.cancel)?;
            let take_lower = upper == max
                || (lower != max && self.compare(ring[lower], ring[upper])? == Ordering::Less);
            let id = if take_lower {
                let id = ring[lower];
                lower = next(lower);
                id
            } else {
                let id = ring[upper];
                upper = previous(upper);
                id
            };
            if sorted.len() >= n
                || self.compare(sorted.last().ok_or(SurfaceError::Geometry)?.id, id)?
                    != Ordering::Less
            {
                return Err(SurfaceError::Geometry);
            }
            sorted.push(SweepVertex {
                id,
                lower: take_lower,
            });
        }
        if sorted.len() + 1 != n {
            return Err(SurfaceError::Geometry);
        }
        sorted.push(SweepVertex {
            id: ring[max],
            lower: false,
        });
        stack.extend_from_slice(&sorted[..2]);
        for i in 2..n - 1 {
            check(&self.cancel)?;
            let current = sorted[i];
            if current.lower != stack.last().ok_or(SurfaceError::Geometry)?.lower {
                while stack.len() > 1 {
                    let last = stack.pop().ok_or(SurfaceError::Geometry)?;
                    self.emit(
                        [
                            current.id,
                            last.id,
                            stack.last().ok_or(SurfaceError::Geometry)?.id,
                        ],
                        region,
                    )?;
                }
                stack.clear();
                stack.push(sorted[i - 1]);
                stack.push(current);
            } else {
                let mut last = stack.pop().ok_or(SurfaceError::Geometry)?;
                while let Some(top) = stack.last() {
                    let turn = self.turn([current.id, last.id, top.id])?;
                    if (current.lower && turn >= 0) || (!current.lower && turn <= 0) {
                        break;
                    }
                    self.emit([current.id, last.id, top.id], region)?;
                    last = stack.pop().ok_or(SurfaceError::Geometry)?;
                }
                stack.push(last);
                stack.push(current);
            }
        }
        let last = sorted[n - 1];
        while stack.len() > 1 {
            let a = stack.pop().ok_or(SurfaceError::Geometry)?;
            self.emit(
                [
                    last.id,
                    a.id,
                    stack.last().ok_or(SurfaceError::Geometry)?.id,
                ],
                region,
            )?;
        }
        Ok(())
    }
    fn insert(&mut self, point: u32, region: u8) -> Result<(), SurfaceError> {
        let count = self.triangles.len();
        let mut matches = 0;
        let mut interior = false;
        for i in 0..count {
            check(&self.cancel)?;
            let old = self.triangles[i];
            if old.region != region {
                continue;
            }
            let [a, b, c] = old.vertices;
            let signs = [
                self.turn([a, b, point])?,
                self.turn([b, c, point])?,
                self.turn([c, a, point])?,
            ];
            if signs.into_iter().any(|v| v < 0) {
                continue;
            }
            let zero = signs.into_iter().filter(|v| *v == 0).count();
            if zero > 1 {
                return Err(SurfaceError::Geometry);
            }
            matches += 1;
            if zero == 0 {
                if matches != 1 {
                    return Err(SurfaceError::Geometry);
                }
                interior = true;
                self.triangles[i].vertices = [a, b, point];
                self.emit([b, c, point], old.region)?;
                self.emit([c, a, point], old.region)?;
                break;
            }
            let edge = signs
                .into_iter()
                .position(|v| v == 0)
                .ok_or(SurfaceError::Geometry)?;
            let [a, b, c] = [
                old.vertices[edge],
                old.vertices[(edge + 1) % 3],
                old.vertices[(edge + 2) % 3],
            ];
            self.triangles[i].vertices = [a, point, c];
            self.emit([point, b, c], old.region)?;
        }
        if (interior && matches != 1) || (!interior && matches != 2) {
            return Err(SurfaceError::Geometry);
        }
        Ok(())
    }
}
// Internal geometry access only; result types keep their concrete input owners.
pub(in super::super::super) trait Input {
    fn vertices(&self) -> &[[ExactCoordinate; 2]];
    fn original_count(&self) -> usize;
    fn region_count(&self) -> usize;
    fn region(&self, id: u8) -> Option<(u8, u16)>;
    fn half_edge(&self, id: u8) -> Option<&PatchHalfEdge>;
    fn edge(&self, id: u8) -> Option<[u8; 2]>;
    fn edge_vertices(&self, id: u8) -> Option<&[u32]>;
    fn isolated_region(&self, id: u8) -> Result<u8, SurfaceError>;
    fn vertex(&self, id: u32) -> Option<&[ExactCoordinate; 2]> {
        self.vertices().get(id as usize)
    }
    fn ring(
        &self,
        region: u8,
        cancel: &impl Fn() -> bool,
        mut emit: impl FnMut(u32) -> Result<(), SurfaceError>,
    ) -> Result<usize, SurfaceError> {
        let (first, edge_count) = self.region(region).ok_or(SurfaceError::Geometry)?;
        let mut at = first;
        let mut count = 0usize;
        for _ in 0..edge_count {
            check(cancel)?;
            let half = self.half_edge(at).ok_or(SurfaceError::Geometry)?;
            let [from, to] = self.edge(half.edge).ok_or(SurfaceError::Geometry)?;
            if !((half.from == from && half.to == to) || (half.from == to && half.to == from)) {
                return Err(SurfaceError::Geometry);
            }
            if half.region != Some(region) {
                return Err(SurfaceError::Geometry);
            }
            let path = self
                .edge_vertices(half.edge)
                .ok_or(SurfaceError::Geometry)?;
            if path.len() < 2
                || path.first() != Some(&u32::from(from))
                || path.last() != Some(&u32::from(to))
            {
                return Err(SurfaceError::Geometry);
            }
            for i in 0..path.len() - 1 {
                check(cancel)?;
                let id = if half.from == from {
                    path[i]
                } else {
                    path[path.len() - 1 - i]
                };
                count += 1;
                if count > self.vertices().len() {
                    return Err(SurfaceError::Geometry);
                }
                if self.vertex(id).is_none() {
                    return Err(SurfaceError::Geometry);
                }
                emit(id)?;
            }
            at = half.next;
        }
        if at != first || count < 3 {
            return Err(SurfaceError::Geometry);
        }
        Ok(count)
    }
}
impl Input for RefinedSubdivision {
    fn vertices(&self) -> &[[ExactCoordinate; 2]] {
        &self.vertices
    }
    fn original_count(&self) -> usize {
        self.base.nodes().count()
    }
    fn region_count(&self) -> usize {
        self.base.regions().count()
    }
    fn region(&self, id: u8) -> Option<(u8, u16)> {
        self.base
            .regions
            .get(usize::from(id))?
            .as_ref()
            .map(|r| (r.first, r.edge_count))
    }
    fn half_edge(&self, id: u8) -> Option<&PatchHalfEdge> {
        self.base.half_edge(id)
    }
    fn edge(&self, id: u8) -> Option<[u8; 2]> {
        self.base.edge(id).map(|e| [e.from, e.to])
    }
    fn edge_vertices(&self, id: u8) -> Option<&[u32]> {
        self.edge_vertices(id)
    }
    fn isolated_region(&self, id: u8) -> Result<u8, SurfaceError> {
        if !self.base.arena.whole
            || self.base.regions().count() != 1
            || self.base.node(id).is_none_or(|n| n.boundary != 0)
        {
            return Err(SurfaceError::Geometry);
        }
        Ok(0)
    }
}
impl RefinedSubdivision {
    /// Triangulate every bounded region and retain every original constraint.
    /// Unit-square curve error remains separate from affine/GPU conversion.
    pub fn triangulate(
        self: &Arc<Self>,
        limits: TriangulationLimits,
        cancel: impl Fn() -> bool,
    ) -> Result<TriangulatedSubdivision, SurfaceError> {
        self.triangulate_with_budget(limits, MemoryBudget::volumes(), cancel)
    }
    fn triangulate_with_budget(
        self: &Arc<Self>,
        limits: TriangulationLimits,
        budget: Arc<MemoryBudget>,
        cancel: impl Fn() -> bool,
    ) -> Result<TriangulatedSubdivision, SurfaceError> {
        let completed = triangulate::<NODES>(
            self.as_ref(),
            std::mem::size_of::<TriangulatedSubdivision>(),
            limits,
            budget,
            cancel,
        )?;
        Ok(TriangulatedSubdivision {
            input: Arc::clone(self),
            triangles: completed.triangles,
            bytes: completed.bytes,
            _reservation: completed.reservation,
        })
    }
}

pub(in super::super::super) struct Completed {
    pub(in super::super::super) triangles: Vec<RegionTriangle>,
    pub(in super::super::super) bytes: u64,
    pub(in super::super::super) reservation: Reservation,
}
pub(in super::super::super) fn scratch_bytes<const N: usize>(
    count: usize,
) -> Result<u64, SurfaceError> {
    let per = std::mem::size_of::<Bounds>()
        + std::mem::size_of::<u32>()
        + 2 * std::mem::size_of::<SweepVertex>();
    u64::try_from(
        count
            .checked_mul(per)
            .and_then(|v| v.checked_add(N))
            .ok_or(SurfaceError::Limit)?,
    )
    .map_err(|_| SurfaceError::Limit)
}
pub(in super::super::super) fn triangulate<const N: usize>(
    input: &impl Input,
    owner_bytes: usize,
    limits: TriangulationLimits,
    budget: Arc<MemoryBudget>,
    cancel: impl Fn() -> bool,
) -> Result<Completed, SurfaceError> {
    check(&cancel)?;
    if limits.max_triangles > MAX_TRIANGLES || limits.max_bytes > MAX_TRIANGLE_BYTES {
        return Err(SurfaceError::Limit);
    }
    let count = input.vertices().len();
    let original = input.original_count();
    if count < 4
        || original > N
        || original > count
        || count > MAX_VERTICES
        || input.region_count() == 0
        || input.region_count() > 256
    {
        return Err(SurfaceError::Geometry);
    }
    let mut used = [false; N];
    let mut expected = 0usize;
    for id in 0..input.region_count() {
        let n = input.ring(id as u8, &cancel, |v| {
            if (v as usize) < original {
                used[v as usize] = true;
            }
            Ok(())
        })?;
        expected = expected.checked_add(n - 2).ok_or(SurfaceError::Limit)?;
    }
    let isolated = used[..original].iter().filter(|v| !**v).count();
    expected = expected
        .checked_add(2 * isolated)
        .ok_or(SurfaceError::Limit)?;
    // A simple n-vertex region has n-2 triangles; inserting an interior
    // constraint adds two, including a point on a shared diagonal. The
    // complete square consequently has2V-B-2<=2V-6 triangles.
    let bytes = u64::try_from(
        expected
            .checked_mul(std::mem::size_of::<RegionTriangle>())
            .and_then(|n| n.checked_add(owner_bytes))
            .ok_or(SurfaceError::Limit)?,
    )
    .map_err(|_| SurfaceError::Limit)?;
    if expected > limits.max_triangles || bytes > limits.max_bytes {
        return Err(SurfaceError::Limit);
    }
    let scratch_bytes = scratch_bytes::<N>(count)?;
    let _scratch = budget.reserve(scratch_bytes).ok_or(SurfaceError::Memory)?;
    let reservation = budget.reserve(bytes).ok_or(SurfaceError::Memory)?;
    let mut bounds = allocate(count)?;
    for point in input.vertices() {
        let enclosed = Enclosed::new(*point, None, &cancel)?;
        bounds.push(Bounds {
            lower: enclosed.lower,
            upper: enclosed.upper,
        });
    }
    let mut ring = allocate(count)?;
    let mut sorted = allocate(count)?;
    let mut stack = allocate(count)?;
    let mut worker = Worker {
        input,
        bounds,
        budget,
        cancel,
        triangles: allocate(expected)?,
        expected,
    };
    for id in 0..input.region_count() {
        let id = id as u8;
        ring.clear();
        input.ring(id, &worker.cancel, |v| {
            if ring.len() >= count {
                return Err(SurfaceError::Geometry);
            }
            ring.push(v);
            Ok(())
        })?;
        worker.region(id, &ring, &mut sorted, &mut stack)?;
    }
    for (id, used) in used[..original].iter().copied().enumerate() {
        if !used {
            let region = input.isolated_region(id as u8)?;
            if input.region(region).is_none() {
                return Err(SurfaceError::Geometry);
            }
            worker.insert(id as u32, region)?;
        }
    }
    check(&worker.cancel)?;
    if worker.triangles.len() != expected {
        return Err(SurfaceError::Geometry);
    }
    Ok(Completed {
        triangles: worker.triangles,
        bytes,
        reservation,
    })
}

#[cfg(test)]
mod tests;
