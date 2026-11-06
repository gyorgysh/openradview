// SPDX-License-Identifier: GPL-3.0-or-later
//! Coordinated exact-slice polylines for two-field regions and their constraints.
use super::super::subdivision::refine::{
    Enclosed, MAX_BITS, MAX_BYTES, MAX_VERTICES, Range, Segment, allocate, certified, check,
    geometry, split,
};
use super::*;
use crate::{
    resources::{MemoryBudget, Reservation},
    surface::SurfaceError,
};
use std::sync::Arc;

// At most one nonvertical zero graph per field, plus the two domain edges.
const ACTIVE: usize = 4;
// A curve's largest span halves within two selections, up to enclosure units;
// intervening cuts selected for the other curve cannot enlarge its bounds.
// Two curves with <=24-bit requests need at most100 selected levels. A collapsed
// lens needs at most one additional split before its endpoint orders are strict.
const DEPTH: usize = 128;

#[derive(Clone, Copy)]
struct Slab {
    segments: [Option<Segment>; ACTIVE],
    depth: u8,
}
struct Scratch {
    nodes: [Option<Enclosed>; JOINT_NODES],
    events: [Option<ExactCoordinate>; JOINT_NODES],
    stack: [Option<Slab>; DEPTH],
    cuts: Vec<ExactCoordinate>,
}

/// A single shared slice partition and canonical edge paths. The original graph
/// and node prefix remain intact. Error is measured in the exact unit square,
/// before physical-affine conversion, floating quantization or triangulation.
pub struct RefinedJointSubdivision {
    base: JointSubdivision,
    vertices: Vec<[ExactCoordinate; 2]>,
    indices: Vec<u32>,
    ranges: [Option<Range>; JOINT_EDGES],
    cuts: Vec<ExactCoordinate>,
    bits: u8,
    bytes: u64,
    _reservation: Reservation,
}
impl RefinedJointSubdivision {
    pub fn subdivision(&self) -> &JointSubdivision {
        &self.base
    }
    pub fn vertices(&self) -> &[[ExactCoordinate; 2]] {
        &self.vertices
    }
    pub fn vertex(&self, id: u32) -> Option<&[ExactCoordinate; 2]> {
        self.vertices.get(id as usize)
    }
    /// Canonical undirected edge order, from its original from to its to node.
    pub fn edge_vertices(&self, id: u8) -> Option<&[u32]> {
        let r = self.ranges.get(usize::from(id))?.as_ref()?;
        self.indices.get(r.start..r.start.checked_add(r.len)?)
    }
    pub fn slice_coordinates(&self) -> &[ExactCoordinate] {
        &self.cuts
    }
    pub fn maximum_coordinate_error(&self) -> Rational {
        Rational {
            numerator: 1,
            denominator: 1u64 << self.bits,
        }
    }
    /// Retained typed storage, excluding scratch, allocator and process overhead.
    pub fn referenced_bytes(&self) -> u64 {
        self.bytes
    }
}
fn bytes(vertices: usize, indices: usize, cuts: usize) -> Result<u64, SurfaceError> {
    let arrays = [
        (vertices, std::mem::size_of::<[ExactCoordinate; 2]>()),
        (indices, std::mem::size_of::<u32>()),
        (cuts, std::mem::size_of::<ExactCoordinate>()),
    ];
    let size = arrays.into_iter().try_fold(
        std::mem::size_of::<RefinedJointSubdivision>(),
        |sum, (count, size)| {
            count
                .checked_mul(size)
                .and_then(|v| sum.checked_add(v))
                .ok_or(SurfaceError::Limit)
        },
    )?;
    u64::try_from(size).map_err(|_| SurfaceError::Limit)
}
fn index_count(base: &JointSubdivision, vertices: usize) -> Result<usize, SurfaceError> {
    base.edge_count
        .checked_mul(2)
        .and_then(|n| n.checked_add(vertices.checked_sub(base.count)?))
        .ok_or(SurfaceError::Limit)
}
fn endpoint(nodes: &[Option<Enclosed>; JOINT_NODES], id: u8) -> Result<Enclosed, SurfaceError> {
    nodes
        .get(usize::from(id))
        .copied()
        .flatten()
        .ok_or(SurfaceError::Geometry)
}
impl JointSubdivision {
    fn curve_field(&self, edge: JointEdge) -> Result<Bilinear, SurfaceError> {
        match edge.curves & 3 {
            1 | 3 => Ok(self.fields[0]),
            2 => Ok(self.fields[1]),
            _ => Err(SurfaceError::Geometry),
        }
    }
    fn curved(&self, edge: JointEdge) -> Result<bool, SurfaceError> {
        if !matches!(edge.geometry, CommonZero::Curve(_)) {
            return Ok(false);
        }
        let [a, b, c, d] = coefficients(self.curve_field(edge)?).map(i64::from);
        Ok(d != 0 && a * d != b * c)
    }
    fn on_slice(
        &self,
        edge: JointEdge,
        u: ExactCoordinate,
        nodes: &[Option<Enclosed>; JOINT_NODES],
    ) -> Result<([ExactCoordinate; 2], Option<u8>), SurfaceError> {
        let a = endpoint(nodes, edge.from)?;
        let b = endpoint(nodes, edge.to)?;
        for p in [a, b] {
            if u.compare(p.point[0]).map_err(geometry)? == Ordering::Equal {
                return Ok((p.point, p.original));
            }
        }
        let v = match edge.geometry {
            CommonZero::Axis { axis: 1, position } => {
                rational_coordinate(position).map_err(geometry)?
            }
            CommonZero::Curve(_) => {
                match self.curve_field(edge)?.zero_slice(0, u).map_err(geometry)? {
                    BilinearSlice::Point(v) => v,
                    _ => return Err(SurfaceError::Geometry),
                }
            }
            _ => return Err(SurfaceError::Geometry),
        };
        Ok(([u, v], None))
    }
    fn enclosed(
        &self,
        edge: JointEdge,
        u: ExactCoordinate,
        nodes: &[Option<Enclosed>; JOINT_NODES],
        cancel: &impl Fn() -> bool,
    ) -> Result<Enclosed, SurfaceError> {
        let (point, original) = self.on_slice(edge, u, nodes)?;
        Enclosed::new(point, original, cancel)
    }
    fn active_edges(
        &self,
        left: ExactCoordinate,
        right: ExactCoordinate,
        nodes: &[Option<Enclosed>; JOINT_NODES],
        cancel: &impl Fn() -> bool,
    ) -> Result<[Option<u8>; ACTIVE], SurfaceError> {
        let mut active = [None; ACTIVE];
        let mut count = 0;
        for (id, edge) in self.edges() {
            check(cancel)?;
            let a = endpoint(nodes, edge.from)?.point[0];
            let b = endpoint(nodes, edge.to)?.point[0];
            if a.compare(b).map_err(geometry)? == Ordering::Equal {
                continue;
            }
            if a.compare(right).map_err(geometry)? != Ordering::Less
                || b.compare(left).map_err(geometry)? != Ordering::Greater
            {
                continue;
            }
            if a.compare(left).map_err(geometry)? == Ordering::Greater
                || b.compare(right).map_err(geometry)? == Ordering::Less
            {
                return Err(SurfaceError::Geometry);
            }
            *active.get_mut(count).ok_or(SurfaceError::Geometry)? = Some(id);
            count += 1;
        }
        if count < 2 {
            return Err(SurfaceError::Geometry);
        }
        Ok(active)
    }
    fn walk_slab(
        &self,
        active: [Option<u8>; ACTIVE],
        [left, right]: [ExactCoordinate; 2],
        scratch: &mut Scratch,
        vertices: &mut usize,
        limits: RefinementLimits,
        cancel: &impl Fn() -> bool,
    ) -> Result<(), SurfaceError> {
        let mut initial = Slab {
            segments: [None; ACTIVE],
            depth: 0,
        };
        for (i, id) in active.iter().copied().enumerate() {
            if let Some(id) = id {
                let edge = *self.edge(id).ok_or(SurfaceError::Geometry)?;
                initial.segments[i] = Some(Segment {
                    a: self.enclosed(edge, left, &scratch.nodes, cancel)?,
                    b: self.enclosed(edge, right, &scratch.nodes, cancel)?,
                    depth: 0,
                });
            }
        }
        scratch.stack[0] = Some(initial);
        let mut count = 1;
        while count > 0 {
            check(cancel)?;
            count -= 1;
            let slab = scratch.stack[count].take().ok_or(SurfaceError::Geometry)?;
            let mut selected = None;
            for (i, id) in active.iter().copied().enumerate() {
                if let Some(id) = id {
                    let edge = *self.edge(id).ok_or(SurfaceError::Geometry)?;
                    let segment = slab.segments[i].ok_or(SurfaceError::Geometry)?;
                    if self.curved(edge)?
                        && !certified(
                            segment,
                            coefficients(self.curve_field(edge)?),
                            limits.precision_bits,
                        )
                    {
                        selected = Some(i);
                        break;
                    }
                }
            }
            // Equal endpoint heights on distinct graphs would erase a lens if
            // both became one chord. A strict interior slice preserves its area.
            for i in 0..ACTIVE {
                let Some(a) = slab.segments[i] else { continue };
                for j in i + 1..ACTIVE {
                    let Some(b) = slab.segments[j] else { continue };
                    let lo = a.a.point[1].compare(b.a.point[1]).map_err(geometry)?;
                    let hi = a.b.point[1].compare(b.b.point[1]).map_err(geometry)?;
                    if lo != Ordering::Equal && hi != Ordering::Equal && lo != hi {
                        return Err(SurfaceError::Geometry);
                    }
                    if lo == Ordering::Equal && hi == Ordering::Equal {
                        let edge = *self
                            .edge(active[i].ok_or(SurfaceError::Geometry)?)
                            .ok_or(SurfaceError::Geometry)?;
                        let other = *self
                            .edge(active[j].ok_or(SurfaceError::Geometry)?)
                            .ok_or(SurfaceError::Geometry)?;
                        selected = Some(if self.curved(edge)? {
                            i
                        } else if self.curved(other)? {
                            j
                        } else {
                            return Err(SurfaceError::Geometry);
                        });
                    }
                }
            }
            let Some(chosen) = selected else {
                let mut added = 0usize;
                let mut u = None;
                for segment in slab.segments.into_iter().flatten() {
                    u = Some(segment.b.point[0]);
                    added += usize::from(segment.b.original.is_none());
                }
                let u = u.ok_or(SurfaceError::Geometry)?;
                let next = vertices.checked_add(added).ok_or(SurfaceError::Limit)?;
                if next > limits.max_vertices
                    || scratch.cuts.len() >= limits.max_vertices
                    || bytes(next, index_count(self, next)?, scratch.cuts.len() + 1)?
                        > limits.max_bytes
                {
                    return Err(SurfaceError::Limit);
                }
                let prior = scratch.cuts.last().ok_or(SurfaceError::Geometry)?;
                if prior.compare(u).map_err(geometry)? != Ordering::Less {
                    return Err(SurfaceError::Geometry);
                }
                scratch.cuts.push(u);
                *vertices = next;
                continue;
            };
            if usize::from(slab.depth) + 1 >= DEPTH || count + 2 > DEPTH {
                return Err(SurfaceError::Limit);
            }
            let edge = *self
                .edge(active[chosen].ok_or(SurfaceError::Geometry)?)
                .ok_or(SurfaceError::Geometry)?;
            let middle = split(
                slab.segments[chosen].ok_or(SurfaceError::Geometry)?,
                coefficients(self.curve_field(edge)?),
                cancel,
            )?;
            let u = middle.point[0];
            if u.rational_value().is_none() {
                return Err(SurfaceError::Geometry);
            }
            let mut first = Slab {
                segments: [None; ACTIVE],
                depth: slab.depth + 1,
            };
            let mut last = first;
            for (i, id) in active.iter().copied().enumerate() {
                if let Some(id) = id {
                    let old = slab.segments[i].ok_or(SurfaceError::Geometry)?;
                    let point = if i == chosen {
                        middle
                    } else {
                        self.enclosed(
                            *self.edge(id).ok_or(SurfaceError::Geometry)?,
                            u,
                            &scratch.nodes,
                            cancel,
                        )?
                    };
                    first.segments[i] = Some(Segment {
                        a: old.a,
                        b: point,
                        depth: 0,
                    });
                    last.segments[i] = Some(Segment {
                        a: point,
                        b: old.b,
                        depth: 0,
                    });
                }
            }
            scratch.stack[count] = Some(last);
            scratch.stack[count + 1] = Some(first);
            count += 2;
        }
        Ok(())
    }
    /// Worker-side shared count/admit/emit. No pressure-driven coarsening, source
    /// edits, triangle conversion or floating geometry decisions occur here.
    pub fn refine(
        &self,
        limits: RefinementLimits,
        cancel: impl Fn() -> bool,
    ) -> Result<RefinedJointSubdivision, SurfaceError> {
        self.refine_with_budget(limits, MemoryBudget::volumes(), cancel)
    }
    fn refine_with_budget(
        &self,
        limits: RefinementLimits,
        budget: Arc<MemoryBudget>,
        cancel: impl Fn() -> bool,
    ) -> Result<RefinedJointSubdivision, SurfaceError> {
        check(&cancel)?;
        if limits.precision_bits > MAX_BITS
            || limits.max_vertices > MAX_VERTICES
            || limits.max_bytes > MAX_BYTES
            || self.count > limits.max_vertices
            || bytes(self.count, 2 * self.edge_count, 1)? > limits.max_bytes
        {
            return Err(SurfaceError::Limit);
        }
        let scratch_bytes = std::mem::size_of::<Scratch>()
            .checked_add(
                limits
                    .max_vertices
                    .checked_mul(std::mem::size_of::<ExactCoordinate>())
                    .ok_or(SurfaceError::Limit)?,
            )
            .ok_or(SurfaceError::Limit)?;
        let _scratch = budget
            .reserve(scratch_bytes as u64)
            .ok_or(SurfaceError::Memory)?;
        let mut scratch = Scratch {
            nodes: [None; JOINT_NODES],
            events: [None; JOINT_NODES],
            stack: [None; DEPTH],
            cuts: allocate(limits.max_vertices)?,
        };
        let mut events = 0;
        for (id, node) in self.nodes() {
            check(&cancel)?;
            scratch.nodes[usize::from(id)] =
                Some(Enclosed::new(node.coordinates, Some(id), &cancel)?);
            let u = node.coordinates[0];
            let order = if events == 0 {
                Ordering::Less
            } else {
                scratch.events[events - 1]
                    .ok_or(SurfaceError::Geometry)?
                    .compare(u)
                    .map_err(geometry)?
            };
            if order == Ordering::Greater {
                return Err(SurfaceError::Geometry);
            }
            if order == Ordering::Less {
                scratch.events[events] = Some(u);
                events += 1;
            }
        }
        if events < 2 {
            return Err(SurfaceError::Geometry);
        }
        scratch
            .cuts
            .push(scratch.events[0].ok_or(SurfaceError::Geometry)?);
        let mut vertex_count = self.count;
        for i in 1..events {
            let left = scratch.events[i - 1].ok_or(SurfaceError::Geometry)?;
            let right = scratch.events[i].ok_or(SurfaceError::Geometry)?;
            let active = self.active_edges(left, right, &scratch.nodes, &cancel)?;
            self.walk_slab(
                active,
                [left, right],
                &mut scratch,
                &mut vertex_count,
                limits,
                &cancel,
            )?;
        }
        let index_count = index_count(self, vertex_count)?;
        let retained = bytes(vertex_count, index_count, scratch.cuts.len())?;
        let reservation = budget.reserve(retained).ok_or(SurfaceError::Memory)?;
        let mut vertices = allocate(vertex_count)?;
        let mut indices = allocate(index_count)?;
        let mut cuts = allocate(scratch.cuts.len())?;
        cuts.extend_from_slice(&scratch.cuts);
        for (_, node) in self.nodes() {
            vertices.push(node.coordinates);
        }
        let mut ranges = [None; JOINT_EDGES];
        for (id, edge) in self.edges() {
            check(&cancel)?;
            let start = indices.len();
            let a = endpoint(&scratch.nodes, edge.from)?;
            let b = endpoint(&scratch.nodes, edge.to)?;
            if a.point[0].compare(b.point[0]).map_err(geometry)? == Ordering::Equal {
                if indices.len().checked_add(2).is_none_or(|n| n > index_count) {
                    return Err(SurfaceError::Geometry);
                }
                indices.extend([u32::from(edge.from), u32::from(edge.to)]);
            } else {
                let lo = find_cut(&cuts, a.point[0], &cancel)?;
                let hi = find_cut(&cuts, b.point[0], &cancel)?;
                for u in cuts.get(lo..=hi).ok_or(SurfaceError::Geometry)? {
                    check(&cancel)?;
                    let (point, original) = self.on_slice(*edge, *u, &scratch.nodes)?;
                    let vertex = if let Some(original) = original {
                        u32::from(original)
                    } else {
                        if vertices.len() >= vertex_count {
                            return Err(SurfaceError::Geometry);
                        }
                        let index =
                            u32::try_from(vertices.len()).map_err(|_| SurfaceError::Limit)?;
                        vertices.push(point);
                        index
                    };
                    if indices.len() >= index_count {
                        return Err(SurfaceError::Geometry);
                    }
                    indices.push(vertex);
                }
            }
            ranges[usize::from(id)] = Some(Range {
                start,
                len: indices.len() - start,
            });
        }
        if vertices.len() != vertex_count || indices.len() != index_count {
            return Err(SurfaceError::Geometry);
        }
        check(&cancel)?;
        Ok(RefinedJointSubdivision {
            base: self.clone(),
            vertices,
            indices,
            ranges,
            cuts,
            bits: limits.precision_bits,
            bytes: retained,
            _reservation: reservation,
        })
    }
}
fn find_cut(
    cuts: &[ExactCoordinate],
    u: ExactCoordinate,
    cancel: &impl Fn() -> bool,
) -> Result<usize, SurfaceError> {
    let (mut lo, mut hi) = (0, cuts.len());
    while lo < hi {
        check(cancel)?;
        let mid = lo + (hi - lo) / 2;
        match cuts[mid].compare(u).map_err(geometry)? {
            Ordering::Less => lo = mid + 1,
            Ordering::Greater => hi = mid,
            Ordering::Equal => return Ok(mid),
        }
    }
    Err(SurfaceError::Geometry)
}

#[cfg(test)]
mod tests;
