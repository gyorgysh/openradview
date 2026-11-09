// SPDX-License-Identifier: GPL-3.0-or-later
//! Bounded square subdivision by one bilinear zero set, with directed regions.
use super::*;
pub(super) mod refine;
pub use refine::{
    RefinedSubdivision, RefinementLimits, RegionTriangle, TriangulatedSubdivision,
    TriangulationLimits,
};

// At most four regions: two factor lines cross, or at most two noncrossing
// hyperbola arcs meet the boundary. E<=V+3, including retained isolated points.
const EDGES: usize = NODES + 3;
const HALF_EDGES: usize = 2 * EDGES;
const REGIONS: usize = 4;
const _: () = assert!(HALF_EDGES <= 256);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SubdivisionEdge {
    pub from: u8,
    pub to: u8,
    /// Cyclic square edge index, or None for an interior zero-curve arc.
    pub boundary: Option<u8>,
    pub zero_curve: bool,
    inside_forward: bool,
    right_sign: i8,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PatchHalfEdge {
    pub edge: u8,
    pub from: u8,
    pub to: u8,
    pub twin: u8,
    pub next: u8,
    /// The region on the left. None belongs to the exterior square boundary.
    pub region: Option<u8>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PatchRegion {
    pub first: u8,
    pub edge_count: u16,
    /// Exact interior sign. Zero is reserved for a whole zero field.
    pub sign: i8,
}
/// A subdivision with exact curve arcs. Boundary points need not lie on F=0;
/// contour points must. Curved arcs are not straight chords or triangles.
/// Interior points of a whole zero field remain isolated retained constraints.
#[derive(Clone)]
pub struct PlaneSubdivision {
    arena: FaceArrangement,
    edges: [Option<SubdivisionEdge>; EDGES],
    edge_count: usize,
    half_edges: [Option<PatchHalfEdge>; HALF_EDGES],
    regions: [Option<PatchRegion>; REGIONS],
    region_count: usize,
}
impl PlaneSubdivision {
    pub const MAX_INSERTIONS: usize = INSERTIONS;
    pub fn field(&self) -> Bilinear {
        self.arena.field
    }
    pub fn nodes(&self) -> impl Iterator<Item = (u8, &ContourNode)> {
        self.arena.nodes()
    }
    pub fn node(&self, id: u8) -> Option<&ContourNode> {
        self.arena.node(id)
    }
    pub fn node_sign(&self, id: u8) -> Result<i8, FieldError> {
        let point = self.node(id).ok_or(FieldError::Bounds)?.coordinates;
        bilinear_sign(coefficients(self.field()), point)
    }
    pub fn edge(&self, id: u8) -> Option<&SubdivisionEdge> {
        self.edges.get(usize::from(id))?.as_ref()
    }
    pub fn zero_curves(&self) -> impl Iterator<Item = &ContourSegment> {
        self.arena.segments()
    }
    pub fn edges(&self) -> impl Iterator<Item = (u8, &SubdivisionEdge)> {
        self.edges
            .iter()
            .enumerate()
            .filter_map(|(i, e)| e.as_ref().map(|e| (i as u8, e)))
    }
    pub fn half_edges(&self) -> impl Iterator<Item = (u8, &PatchHalfEdge)> {
        self.half_edges
            .iter()
            .enumerate()
            .filter_map(|(i, h)| h.as_ref().map(|h| (i as u8, h)))
    }
    pub fn half_edge(&self, id: u8) -> Option<&PatchHalfEdge> {
        self.half_edges.get(usize::from(id))?.as_ref()
    }
    pub fn regions(&self) -> impl Iterator<Item = (u8, &PatchRegion)> {
        self.regions
            .iter()
            .enumerate()
            .filter_map(|(i, r)| r.as_ref().map(|r| (i as u8, r)))
    }
    fn add_edge(
        &mut self,
        from: u8,
        to: u8,
        boundary: Option<u8>,
        zero_curve: bool,
    ) -> Result<(), FieldError> {
        if from == to {
            return Err(FieldError::Arithmetic);
        }
        let forward = from < to;
        let (a, b) = if forward { (from, to) } else { (to, from) };
        let right_sign = if zero_curve {
            if forward { 1 } else { -1 }
        } else {
            0
        };
        for edge in self.edges.iter_mut().flatten() {
            if (edge.from, edge.to) != (a, b) {
                continue;
            }
            if let Some(side) = boundary {
                if edge.boundary.is_some_and(|old| old != side) {
                    return Err(FieldError::Arithmetic);
                }
                edge.boundary = Some(side);
                edge.inside_forward = forward;
            }
            if zero_curve {
                if edge.zero_curve && edge.right_sign != right_sign {
                    return Err(FieldError::Arithmetic);
                }
                edge.zero_curve = true;
                edge.right_sign = right_sign;
            }
            return Ok(());
        }
        *self
            .edges
            .get_mut(self.edge_count)
            .ok_or(FieldError::Capacity)? = Some(SubdivisionEdge {
            from: a,
            to: b,
            boundary,
            zero_curve,
            inside_forward: boundary.is_some() && forward,
            right_sign,
        });
        self.edge_count += 1;
        Ok(())
    }
    fn boundary_sign(&self, edge: SubdivisionEdge) -> Result<i8, FieldError> {
        let first = self
            .node(edge.from)
            .ok_or(FieldError::Arithmetic)?
            .coordinates;
        let last = self
            .node(edge.to)
            .ok_or(FieldError::Arithmetic)?
            .coordinates;
        let coefficients = coefficients(self.field());
        let a = bilinear_sign(coefficients, first)?;
        let b = bilinear_sign(coefficients, last)?;
        if a != 0 || b != 0 {
            return consistent_sign(a, b);
        }
        let [_, u, v, mixed] = coefficients;
        let inward = match edge.boundary {
            Some(0) => [v, mixed, 0, 0],
            Some(1) => [-u, 0, -mixed, 0],
            Some(2) => [-v, -mixed, 0, 0],
            Some(3) => [u, 0, mixed, 0],
            _ => return Err(FieldError::Arithmetic),
        };
        consistent_sign(bilinear_sign(inward, first)?, bilinear_sign(inward, last)?)
    }
    fn direction(&self, edge: SubdivisionEdge, reverse: bool) -> Result<u8, FieldError> {
        let (from, to) = if reverse {
            (edge.to, edge.from)
        } else {
            (edge.from, edge.to)
        };
        let at = self.node(from).ok_or(FieldError::Arithmetic)?.coordinates;
        let end = self.node(to).ok_or(FieldError::Arithmetic)?.coordinates;
        let [_, b, c, d] = coefficients(self.field());
        let (mut u, mut v) = (0, 0);
        if edge.boundary.is_none() {
            let sign = if reverse {
                -edge.right_sign
            } else {
                edge.right_sign
            };
            u = -bilinear_sign([c, d, 0, 0], at)? * sign;
            v = bilinear_sign([b, 0, d, 0], at)? * sign;
        }
        if u == 0 && v == 0 {
            u = order_sign(end[0].compare(at[0])?);
            v = order_sign(end[1].compare(at[1])?);
            // Boundary segments and zero-gradient bilinear crosses are axis
            // lines. A secant cannot replace a tangent of an arbitrary arc.
            if u != 0 && v != 0 {
                return Err(FieldError::Arithmetic);
            }
        }
        octant(u, v)
    }
    fn link(&mut self, cancel: &impl Fn() -> bool) -> Result<(), FieldError> {
        // Canonical undirected-edge IDs are independent of curve orientation.
        for i in 1..self.edge_count {
            let mut j = i;
            while j > 0 {
                check_cancel(cancel)?;
                let a = self.edges[j - 1].ok_or(FieldError::Arithmetic)?;
                let b = self.edges[j].ok_or(FieldError::Arithmetic)?;
                if (a.from, a.to) <= (b.from, b.to) {
                    break;
                }
                self.edges.swap(j - 1, j);
                j -= 1;
            }
        }
        let mut outgoing = [[None::<(u8, u8)>; 4]; NODES];
        let mut degree = [0usize; NODES];
        let mut signs = [None::<i8>; HALF_EDGES];
        for i in 0..self.edge_count {
            check_cancel(cancel)?;
            let edge = self.edges[i].ok_or(FieldError::Arithmetic)?;
            let inside = if edge.boundary.is_some() {
                Some(self.boundary_sign(edge)?)
            } else {
                None
            };
            for reverse in [false, true] {
                let id = 2 * i + usize::from(reverse);
                let (from, to) = if reverse {
                    (edge.to, edge.from)
                } else {
                    (edge.from, edge.to)
                };
                self.half_edges[id] = Some(PatchHalfEdge {
                    edge: i as u8,
                    from,
                    to,
                    twin: (id ^ 1) as u8,
                    next: 0,
                    region: None,
                });
                signs[id] = if let Some(sign) = inside {
                    if edge.inside_forward != reverse {
                        Some(sign)
                    } else {
                        None
                    }
                } else {
                    Some(if reverse {
                        edge.right_sign
                    } else {
                        -edge.right_sign
                    })
                };
                if edge.zero_curve {
                    let curve_left = if reverse {
                        edge.right_sign
                    } else {
                        -edge.right_sign
                    };
                    if signs[id].is_some_and(|sign| sign != curve_left) {
                        return Err(FieldError::Arithmetic);
                    }
                }
                let node = usize::from(from);
                let count = degree[node];
                let direction = self.direction(edge, reverse)?;
                let mut position = count;
                if count >= 4 {
                    return Err(FieldError::Arithmetic);
                }
                while position > 0 {
                    let prior = outgoing[node][position - 1]
                        .ok_or(FieldError::Arithmetic)?
                        .0;
                    if prior == direction {
                        return Err(FieldError::Arithmetic);
                    }
                    if prior < direction {
                        break;
                    }
                    outgoing[node][position] = outgoing[node][position - 1];
                    position -= 1;
                }
                if outgoing[node][..count]
                    .iter()
                    .flatten()
                    .any(|(d, _)| *d == direction)
                {
                    return Err(FieldError::Arithmetic);
                }
                outgoing[node][position] = Some((direction, id as u8));
                degree[node] += 1;
            }
        }
        for id in 0..2 * self.edge_count {
            check_cancel(cancel)?;
            let half = self.half_edges[id].ok_or(FieldError::Arithmetic)?;
            let node = usize::from(half.to);
            let count = degree[node];
            let at = outgoing[node][..count]
                .iter()
                .position(|v| v.is_some_and(|(_, h)| h == half.twin))
                .ok_or(FieldError::Arithmetic)?;
            let next = outgoing[node][(at + count - 1) % count]
                .ok_or(FieldError::Arithmetic)?
                .1;
            self.half_edges[id]
                .as_mut()
                .ok_or(FieldError::Arithmetic)?
                .next = next;
        }
        let mut visited = [false; HALF_EDGES];
        for start in 0..2 * self.edge_count {
            if visited[start] {
                continue;
            }
            check_cancel(cancel)?;
            let sign = signs[start];
            let region = if sign.is_some() {
                if self.region_count >= REGIONS {
                    return Err(FieldError::Capacity);
                }
                Some(self.region_count as u8)
            } else {
                None
            };
            let mut id = start;
            let mut count = 0;
            loop {
                check_cancel(cancel)?;
                if visited[id] || signs[id] != sign {
                    return Err(FieldError::Arithmetic);
                }
                visited[id] = true;
                count += 1;
                let half = self.half_edges[id].as_mut().ok_or(FieldError::Arithmetic)?;
                half.region = region;
                id = usize::from(half.next);
                if id == start {
                    break;
                }
                if count >= 2 * self.edge_count {
                    return Err(FieldError::Arithmetic);
                }
            }
            if let Some(sign) = sign {
                self.regions[self.region_count] = Some(PatchRegion {
                    first: start as u8,
                    edge_count: count as u16,
                    sign,
                });
                self.region_count += 1;
            }
        }
        Ok(())
    }
}
fn consistent_sign(a: i8, b: i8) -> Result<i8, FieldError> {
    if a != 0 && b != 0 && a != b {
        Err(FieldError::Arithmetic)
    } else {
        Ok((a + b).signum())
    }
}
fn order_sign(value: Ordering) -> i8 {
    match value {
        Ordering::Less => -1,
        Ordering::Equal => 0,
        Ordering::Greater => 1,
    }
}
fn octant(u: i8, v: i8) -> Result<u8, FieldError> {
    match (u, v) {
        (1, 0) => Ok(0),
        (1, 1) => Ok(1),
        (0, 1) => Ok(2),
        (-1, 1) => Ok(3),
        (-1, 0) => Ok(4),
        (-1, -1) => Ok(5),
        (0, -1) => Ok(6),
        (1, -1) => Ok(7),
        _ => Err(FieldError::Arithmetic),
    }
}
impl Bilinear {
    /// Partition the closed square, retaining nonzero boundary points separately
    /// from zero-curve insertions. The combined raw input limit is80. Exact curve
    /// topology and region signs do not imply chord accuracy or a triangle mesh.
    pub fn subdivision(
        self,
        boundary: &[[ExactCoordinate; 2]],
        contour: &[[ExactCoordinate; 2]],
        cancel: impl Fn() -> bool,
    ) -> Result<PlaneSubdivision, FieldError> {
        check_cancel(&cancel)?;
        if boundary
            .len()
            .checked_add(contour.len())
            .is_none_or(|n| n > INSERTIONS)
        {
            return Err(FieldError::Capacity);
        }
        let mut zeros = [[ExactCoordinate::endpoint(false); 2]; INSERTIONS];
        let mut zero_count = 0;
        for point in contour {
            zeros[zero_count] = *point;
            zero_count += 1;
        }
        for point in boundary {
            check_cancel(&cancel)?;
            let on_boundary = point.iter().try_fold(false, |found, value| {
                Ok::<_, FieldError>(
                    found
                        || value.compare(ExactCoordinate::endpoint(false))? == Ordering::Equal
                        || value.compare(ExactCoordinate::endpoint(true))? == Ordering::Equal,
                )
            })?;
            if !on_boundary {
                return Err(FieldError::Contour);
            }
            if bilinear_sign(coefficients(self), *point)? == 0 {
                zeros[zero_count] = *point;
                zero_count += 1;
            }
        }
        let mut arena = self.arrangement(&zeros[..zero_count], &cancel)?;
        let original = arena.nodes;
        for corner in [[false, false], [true, false], [true, true], [false, true]] {
            arena.add(corner.map(ExactCoordinate::endpoint), &cancel)?;
        }
        for point in boundary {
            check_cancel(&cancel)?;
            arena.add(*point, &cancel)?;
        }
        arena.canonicalize(&cancel)?;
        let mut remap = [0u8; NODES];
        for (i, prior) in original.into_iter().enumerate() {
            let Some(prior) = prior else {
                continue;
            };
            let mut found = None;
            for (id, current) in arena.nodes() {
                check_cancel(&cancel)?;
                if same_point(prior.coordinates, current.coordinates)? {
                    found = Some(id);
                    break;
                }
            }
            remap[i] = found.ok_or(FieldError::Arithmetic)?;
        }
        for segment in arena.segments.iter_mut().flatten() {
            segment.from = remap[usize::from(segment.from)];
            segment.to = remap[usize::from(segment.to)];
        }
        let mut result = PlaneSubdivision {
            arena,
            edges: [None; EDGES],
            edge_count: 0,
            half_edges: [None; HALF_EDGES],
            regions: [None; REGIONS],
            region_count: 0,
        };
        for side in 0..4 {
            let bit = match side {
                0 => 2,
                1 => 1,
                2 => 3,
                _ => 0,
            };
            let mut previous = None;
            for i in 0..result.arena.count {
                check_cancel(&cancel)?;
                let node = result.arena.nodes[i].ok_or(FieldError::Arithmetic)?;
                if node.boundary & (1 << bit) == 0 {
                    continue;
                }
                if let Some(prior) = previous {
                    let (from, to) = if side < 2 {
                        (prior, i as u8)
                    } else {
                        (i as u8, prior)
                    };
                    result.add_edge(from, to, Some(side as u8), false)?;
                }
                previous = Some(i as u8);
            }
        }
        for i in 0..result.arena.segment_count {
            check_cancel(&cancel)?;
            let segment = result.arena.segments[i].ok_or(FieldError::Arithmetic)?;
            result.add_edge(segment.from, segment.to, None, true)?;
        }
        result.link(&cancel)?;
        check_cancel(&cancel)?;
        Ok(result)
    }
}

#[cfg(test)]
mod tests;
