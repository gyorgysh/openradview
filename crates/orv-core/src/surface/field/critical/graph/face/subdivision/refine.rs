// SPDX-License-Identifier: GPL-3.0-or-later
//! Certified adaptive bilinear-arc polylines, preserving exact supplied vertices.
use super::*;
use crate::{
    resources::{MemoryBudget, Reservation},
    surface::SurfaceError,
};
use std::sync::Arc;
pub(in super::super) mod triangulate;
pub use triangulate::{RegionTriangle, TriangulatedSubdivision, TriangulationLimits};

const GRID_BITS: u32 = 40;
const GRID: u64 = 1 << GRID_BITS;
pub(in super::super) const MAX_BITS: u8 = 24;
const DEPTH: usize = 64;
pub(in super::super) const MAX_VERTICES: usize = 65536;
pub(in super::super) const MAX_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RefinementLimits {
    /// Maximum coordinate-wise Hausdorff error is2^-precision_bits in the unit
    /// square, before physical-affine conversion and GPU quantization.
    pub precision_bits: u8,
    pub max_vertices: usize,
    /// Complete retained typed storage, including the copied subdivision.
    pub max_bytes: u64,
}
impl RefinementLimits {
    pub const MAX_PRECISION_BITS: u8 = MAX_BITS;
    pub const MAX_VERTICES: usize = MAX_VERTICES;
    pub const MAX_BYTES: u64 = MAX_BYTES;
}
impl Default for RefinementLimits {
    fn default() -> Self {
        Self {
            precision_bits: 8,
            max_vertices: MAX_VERTICES,
            max_bytes: MAX_BYTES,
        }
    }
}
#[derive(Clone, Copy)]
pub(in super::super) struct Enclosed {
    pub(in super::super) point: [ExactCoordinate; 2],
    pub(in super::super) lower: [u64; 2],
    pub(in super::super) upper: [u64; 2],
    pub(in super::super) original: Option<u8>,
}
impl Enclosed {
    pub(in super::super) fn new(
        point: [ExactCoordinate; 2],
        original: Option<u8>,
        cancel: &impl Fn() -> bool,
    ) -> Result<Self, SurfaceError> {
        let mut lower = [0; 2];
        let mut upper = [0; 2];
        for axis in 0..2 {
            check(cancel)?;
            if let Some(r) = point[axis].rational_value() {
                let scaled = u128::try_from(r.numerator).map_err(|_| SurfaceError::Geometry)?
                    * u128::from(GRID);
                let denominator = u128::from(r.denominator);
                let floor = scaled / denominator;
                lower[axis] = u64::try_from(floor).map_err(|_| SurfaceError::Geometry)?;
                upper[axis] = lower[axis] + u64::from(!scaled.is_multiple_of(denominator));
            } else {
                let (mut lo, mut hi) = (0, GRID);
                let mut exact = None;
                while lo + 1 < hi {
                    check(cancel)?;
                    let mid = lo + (hi - lo) / 2;
                    let value = coordinate(mid, GRID)?;
                    match point[axis].compare(value).map_err(geometry)? {
                        Ordering::Less => hi = mid,
                        Ordering::Greater => lo = mid,
                        Ordering::Equal => {
                            exact = Some(mid);
                            break;
                        }
                    }
                }
                (lower[axis], upper[axis]) = exact.map_or((lo, hi), |v| (v, v));
            }
            if lower[axis] > upper[axis] || upper[axis] > GRID {
                return Err(SurfaceError::Geometry);
            }
        }
        Ok(Self {
            point,
            lower,
            upper,
            original,
        })
    }
}
#[derive(Clone, Copy)]
pub(in super::super) struct Segment {
    pub(in super::super) a: Enclosed,
    pub(in super::super) b: Enclosed,
    pub(in super::super) depth: u8,
}
#[derive(Clone, Copy)]
pub(in super::super) struct Range {
    pub(in super::super) start: usize,
    pub(in super::super) len: usize,
}

/// Completed edge polylines over the same exact region graph. Original node IDs
/// remain the vertex prefix; new rational points belong to one canonical edge.
/// Owners must share this completed refinement and detail across incident patches.
pub struct RefinedSubdivision {
    base: PlaneSubdivision,
    vertices: Vec<[ExactCoordinate; 2]>,
    indices: Vec<u32>,
    ranges: [Option<Range>; EDGES],
    bits: u8,
    bytes: u64,
    _reservation: Reservation,
}
impl RefinedSubdivision {
    pub fn subdivision(&self) -> &PlaneSubdivision {
        &self.base
    }
    pub fn vertices(&self) -> &[[ExactCoordinate; 2]] {
        &self.vertices
    }
    pub fn vertex(&self, id: u32) -> Option<&[ExactCoordinate; 2]> {
        self.vertices.get(id as usize)
    }
    /// Vertex IDs follow the canonical undirected edge from its from to its to.
    pub fn edge_vertices(&self, edge: u8) -> Option<&[u32]> {
        let range = self.ranges.get(usize::from(edge))?.as_ref()?;
        self.indices
            .get(range.start..range.start.checked_add(range.len)?)
    }
    pub fn maximum_coordinate_error(&self) -> Rational {
        Rational {
            numerator: 1,
            denominator: 1u64 << self.bits,
        }
    }
    /// Typed retained storage, excluding allocator/process overhead and source data.
    pub fn referenced_bytes(&self) -> u64 {
        self.bytes
    }
}
pub(in super::super) fn geometry(error: FieldError) -> SurfaceError {
    match error {
        FieldError::Cancelled => SurfaceError::Cancelled,
        FieldError::Capacity => SurfaceError::Limit,
        _ => SurfaceError::Geometry,
    }
}
pub(in super::super) fn check(cancel: &impl Fn() -> bool) -> Result<(), SurfaceError> {
    if cancel() {
        Err(SurfaceError::Cancelled)
    } else {
        Ok(())
    }
}
fn coordinate(n: u64, d: u64) -> Result<ExactCoordinate, SurfaceError> {
    let r = Rational::new(i128::from(n), i128::from(d)).map_err(geometry)?;
    ExactCoordinate::from_rational(r)
        .map_err(geometry)?
        .ok_or(SurfaceError::Geometry)
}
fn rational_point(
    u: i128,
    uq: i128,
    v: i128,
    vq: i128,
) -> Result<[ExactCoordinate; 2], SurfaceError> {
    let point = [
        Rational::new(u, uq).map_err(geometry)?,
        Rational::new(v, vq).map_err(geometry)?,
    ];
    Ok([
        ExactCoordinate::from_rational(point[0])
            .map_err(geometry)?
            .ok_or(SurfaceError::Geometry)?,
        ExactCoordinate::from_rational(point[1])
            .map_err(geometry)?
            .ok_or(SurfaceError::Geometry)?,
    ])
}
fn bounds(segment: Segment) -> ([u64; 2], [u64; 2]) {
    (
        std::array::from_fn(|i| segment.a.lower[i].min(segment.b.lower[i])),
        std::array::from_fn(|i| segment.a.upper[i].max(segment.b.upper[i])),
    )
}
fn gradient_lower(constant: i32, slope: i32, lo: u64, hi: u64) -> u128 {
    let a = i128::from(constant) * i128::from(GRID) + i128::from(slope) * i128::from(lo);
    let b = i128::from(constant) * i128::from(GRID) + i128::from(slope) * i128::from(hi);
    if a.signum() == b.signum() {
        a.unsigned_abs().min(b.unsigned_abs())
    } else {
        0
    }
}
pub(in super::super) fn certified(segment: Segment, [_, b, c, d]: [i32; 4], bits: u8) -> bool {
    let (lo, hi) = bounds(segment);
    let span = [hi[0] - lo[0], hi[1] - lo[1]];
    let scale = 1u128 << bits;
    if span
        .into_iter()
        .all(|v| u128::from(v) * scale <= u128::from(GRID))
    {
        return true;
    }
    let gradient = gradient_lower(c, d, lo[0], hi[0]).max(gradient_lower(b, d, lo[1], hi[1]));
    // On a zero-curve chord, |F|<=|D*du*dv|/4. Comparing at the same u or
    // v gives Hausdorff error <=|D*du*dv|/(4*min|F_v| or 4*min|F_u|).
    // Enclosures overestimate spans and underestimate the gradients. With
    //40-bit grid and <=24-bit tolerance the largest product fits114bits.
    let residual = u128::from(d.unsigned_abs()) * u128::from(span[0]) * u128::from(span[1]) * scale;
    gradient != 0 && residual <= 4 * gradient * u128::from(GRID)
}
pub(in super::super) fn split(
    segment: Segment,
    [a, b, c, d]: [i32; 4],
    cancel: &impl Fn() -> bool,
) -> Result<Enclosed, SurfaceError> {
    let (lo, hi) = bounds(segment);
    let axis = usize::from(hi[1] - lo[1] > hi[0] - lo[0]);
    let middle = lo[axis] + (hi[axis] - lo[axis]) / 2;
    let n = i128::from(middle);
    let q = i128::from(GRID);
    let point = if axis == 0 {
        rational_point(
            n,
            q,
            -(i128::from(a) * q + i128::from(b) * n),
            i128::from(c) * q + i128::from(d) * n,
        )?
    } else {
        rational_point(
            -(i128::from(a) * q + i128::from(c) * n),
            i128::from(b) * q + i128::from(d) * n,
            n,
            q,
        )?
    };
    if point[0].compare(segment.a.point[0]).map_err(geometry)? != Ordering::Greater
        || point[0].compare(segment.b.point[0]).map_err(geometry)? != Ordering::Less
    {
        return Err(SurfaceError::Geometry);
    }
    Enclosed::new(point, None, cancel)
}
fn walk(
    a: Enclosed,
    b: Enclosed,
    field: Bilinear,
    curved: bool,
    bits: u8,
    cancel: &impl Fn() -> bool,
    mut emit: impl FnMut(Enclosed) -> Result<(), SurfaceError>,
) -> Result<(), SurfaceError> {
    let mut stack = [None::<Segment>; DEPTH];
    stack[0] = Some(Segment { a, b, depth: 0 });
    let mut count = 1;
    let coefficients = coefficients(field);
    while count > 0 {
        check(cancel)?;
        count -= 1;
        let segment = stack[count].take().ok_or(SurfaceError::Geometry)?;
        if !curved || certified(segment, coefficients, bits) {
            emit(segment.b)?;
            continue;
        }
        // Largest-coordinate bisection halves both spans within two levels,
        // up to two enclosure units. <=24-bit requests terminate within50levels.
        if usize::from(segment.depth) + 1 >= DEPTH || count + 2 > DEPTH {
            return Err(SurfaceError::Limit);
        }
        let middle = split(segment, coefficients, cancel)?;
        let depth = segment.depth + 1;
        stack[count] = Some(Segment {
            a: middle,
            b: segment.b,
            depth,
        });
        stack[count + 1] = Some(Segment {
            a: segment.a,
            b: middle,
            depth,
        });
        count += 2;
    }
    Ok(())
}
impl Bilinear {
    /// Shared worker kernel for a proved monotone zero arc, ordered by u.
    /// The callback receives the final original endpoint or a new exact point.
    pub(in crate::surface::field) fn refine_arc(
        self,
        points: [[ExactCoordinate; 2]; 2],
        bits: u8,
        budget: &Arc<MemoryBudget>,
        cancel: &impl Fn() -> bool,
        mut emit: impl FnMut([ExactCoordinate; 2], bool) -> Result<(), SurfaceError>,
    ) -> Result<(), SurfaceError> {
        check(cancel)?;
        if bits > MAX_BITS {
            return Err(SurfaceError::Limit);
        }
        if points[0][0].compare(points[1][0]).map_err(geometry)? != Ordering::Less
            || self.sign_at(points[0]).map_err(geometry)? != 0
            || self.sign_at(points[1]).map_err(geometry)? != 0
        {
            return Err(SurfaceError::Geometry);
        }
        let _scratch = budget
            .reserve(
                (2 * std::mem::size_of::<Enclosed>()
                    + std::mem::size_of::<[Option<Segment>; DEPTH]>()) as u64,
            )
            .ok_or(SurfaceError::Memory)?;
        let a = Enclosed::new(points[0], Some(0), cancel)?;
        let b = Enclosed::new(points[1], Some(1), cancel)?;
        let [p, q, r, s] = coefficients(self).map(i64::from);
        let curved = s != 0 && p * s != q * r;
        walk(a, b, self, curved, bits, cancel, |sample| {
            if sample.original.is_some_and(|id| id != 1) {
                return Err(SurfaceError::Geometry);
            }
            emit(sample.point, sample.original.is_some())
        })
    }
}
fn bytes(vertices: usize, indices: usize) -> Result<u64, SurfaceError> {
    let vertex_bytes = vertices
        .checked_mul(std::mem::size_of::<[ExactCoordinate; 2]>())
        .ok_or(SurfaceError::Limit)?;
    let index_bytes = indices
        .checked_mul(std::mem::size_of::<u32>())
        .ok_or(SurfaceError::Limit)?;
    u64::try_from(
        std::mem::size_of::<RefinedSubdivision>()
            .checked_add(vertex_bytes)
            .and_then(|v| v.checked_add(index_bytes))
            .ok_or(SurfaceError::Limit)?,
    )
    .map_err(|_| SurfaceError::Limit)
}
pub(in super::super) fn allocate<T>(count: usize) -> Result<Vec<T>, SurfaceError> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(count)
        .map_err(|_| SurfaceError::Memory)?;
    if values.capacity() != count {
        return Err(SurfaceError::Memory);
    }
    Ok(values)
}
impl PlaneSubdivision {
    /// Worker-side count/admit/emit. This certifies unit-square max-coordinate
    /// error, not physical-affine or GPU rounding error, triangles or source picks.
    pub fn refine(
        &self,
        limits: RefinementLimits,
        cancel: impl Fn() -> bool,
    ) -> Result<RefinedSubdivision, SurfaceError> {
        self.refine_with_budget(limits, MemoryBudget::volumes(), cancel)
    }
    fn refine_with_budget(
        &self,
        limits: RefinementLimits,
        budget: Arc<MemoryBudget>,
        cancel: impl Fn() -> bool,
    ) -> Result<RefinedSubdivision, SurfaceError> {
        check(&cancel)?;
        if limits.precision_bits > MAX_BITS
            || limits.max_vertices > MAX_VERTICES
            || limits.max_bytes > MAX_BYTES
        {
            return Err(SurfaceError::Limit);
        }
        let scratch_bytes = std::mem::size_of::<[Option<Enclosed>; NODES]>()
            + std::mem::size_of::<[Option<Segment>; DEPTH]>();
        let _scratch = budget
            .reserve(scratch_bytes as u64)
            .ok_or(SurfaceError::Memory)?;
        let mut nodes = [None; NODES];
        for (id, node) in self.nodes() {
            nodes[usize::from(id)] = Some(Enclosed::new(node.coordinates, Some(id), &cancel)?);
        }
        let base = self.nodes().count();
        let [a, b, c, d] = coefficients(self.field());
        let nonlinear = d != 0 && i64::from(a) * i64::from(d) != i64::from(b) * i64::from(c);
        let mut vertex_count = base;
        let mut index_count = 0usize;
        if vertex_count > limits.max_vertices
            || bytes(vertex_count, 2 * self.edge_count)? > limits.max_bytes
        {
            return Err(SurfaceError::Limit);
        }
        for (_, edge) in self.edges() {
            check(&cancel)?;
            index_count = index_count.checked_add(1).ok_or(SurfaceError::Limit)?;
            let first = nodes[usize::from(edge.from)].ok_or(SurfaceError::Geometry)?;
            let last = nodes[usize::from(edge.to)].ok_or(SurfaceError::Geometry)?;
            walk(
                first,
                last,
                self.field(),
                nonlinear && edge.zero_curve && edge.boundary.is_none(),
                limits.precision_bits,
                &cancel,
                |point| {
                    index_count = index_count.checked_add(1).ok_or(SurfaceError::Limit)?;
                    vertex_count = vertex_count
                        .checked_add(usize::from(point.original.is_none()))
                        .ok_or(SurfaceError::Limit)?;
                    if vertex_count > limits.max_vertices
                        || bytes(vertex_count, index_count)? > limits.max_bytes
                    {
                        return Err(SurfaceError::Limit);
                    }
                    Ok(())
                },
            )?;
        }
        let retained = bytes(vertex_count, index_count)?;
        if retained > limits.max_bytes {
            return Err(SurfaceError::Limit);
        }
        let reservation = budget.reserve(retained).ok_or(SurfaceError::Memory)?;
        let mut vertices = allocate(vertex_count)?;
        let mut indices = allocate(index_count)?;
        for (_, node) in self.nodes() {
            vertices.push(node.coordinates);
        }
        let mut ranges = [None; EDGES];
        for (id, edge) in self.edges() {
            check(&cancel)?;
            if indices.len() >= index_count {
                return Err(SurfaceError::Geometry);
            }
            let start = indices.len();
            indices.push(u32::from(edge.from));
            let first = nodes[usize::from(edge.from)].ok_or(SurfaceError::Geometry)?;
            let last = nodes[usize::from(edge.to)].ok_or(SurfaceError::Geometry)?;
            walk(
                first,
                last,
                self.field(),
                nonlinear && edge.zero_curve && edge.boundary.is_none(),
                limits.precision_bits,
                &cancel,
                |point| {
                    let vertex = if let Some(original) = point.original {
                        u32::from(original)
                    } else {
                        if vertices.len() >= vertex_count {
                            return Err(SurfaceError::Geometry);
                        }
                        let vertex =
                            u32::try_from(vertices.len()).map_err(|_| SurfaceError::Limit)?;
                        vertices.push(point.point);
                        vertex
                    };
                    if indices.len() >= index_count {
                        return Err(SurfaceError::Geometry);
                    }
                    indices.push(vertex);
                    Ok(())
                },
            )?;
            ranges[usize::from(id)] = Some(Range {
                start,
                len: indices.len() - start,
            });
        }
        if vertices.len() != vertex_count || indices.len() != index_count {
            return Err(SurfaceError::Geometry);
        }
        check(&cancel)?;
        Ok(RefinedSubdivision {
            base: self.clone(),
            vertices,
            indices,
            ranges,
            bits: limits.precision_bits,
            bytes: retained,
            _reservation: reservation,
        })
    }
}

#[cfg(test)]
pub(in super::super) mod tests;
