// SPDX-License-Identifier: GPL-3.0-or-later
//! Open-edge boundary eligibility and exact oriented surface-graph cycles.
use super::*;
impl Scratch {
    fn tangent(&self, half: u16, context: RegularTangents) -> Result<TangentRay, SurfaceError> {
        let h = self
            .halves
            .get(usize::from(half))
            .ok_or(SurfaceError::Geometry)?;
        let e = self
            .edges
            .get(usize::from(h.edge))
            .ok_or(SurfaceError::Geometry)?;
        Ok(match e.geometry {
            CellGraphGeometry::Axis { axis } => TangentRay::Axis {
                axis,
                positive: self.nodes[usize::from(h.from)].coordinates[usize::from(axis)]
                    .compare(self.nodes[usize::from(h.to)].coordinates[usize::from(axis)])
                    .map_err(error)?
                    == Ordering::Less,
            },
            CellGraphGeometry::Face { face } => context
                .ascending_face(
                    face / 2,
                    if h.from == e.nodes[0] {
                        e.face_forward
                    } else {
                        !e.face_forward
                    },
                )
                .map_err(error)?,
        })
    }
    fn double_position(
        &self,
        half: u16,
        star: &AnchorStar,
    ) -> Result<(usize, usize), SurfaceError> {
        let h = self
            .halves
            .get(usize::from(half))
            .ok_or(SurfaceError::Geometry)?;
        let CellGraphGeometry::Axis { axis } = self.edges[usize::from(h.edge)].geometry else {
            return Err(SurfaceError::Geometry);
        };
        let positive = self.nodes[usize::from(h.from)].coordinates[usize::from(axis)]
            .compare(self.nodes[usize::from(h.to)].coordinates[usize::from(axis)])
            .map_err(error)?
            == Ordering::Less;
        for (group, ring) in star.rings().iter().enumerate() {
            if let Some(position) = ring
                .rays()
                .iter()
                .position(|r| r.axis == axis && r.positive == positive)
            {
                return Ok((group, position));
            }
        }
        Err(SurfaceError::Geometry)
    }
    pub(super) fn link(
        &mut self,
        field: Trilinear,
        graph: &CriticalGraph,
        cancel: &impl Fn() -> bool,
    ) -> Result<(), SurfaceError> {
        for (index, e) in self.edges.iter_mut().enumerate() {
            check(cancel)?;
            let a = self
                .nodes
                .get(usize::from(e.nodes[0]))
                .ok_or(SurfaceError::Geometry)?;
            let b = self
                .nodes
                .get(usize::from(e.nodes[1]))
                .ok_or(SurfaceError::Geometry)?;
            let boundary = a.boundary & b.boundary;
            if boundary != e.faces {
                return Err(SurfaceError::Geometry);
            }
            e.eligible = match e.geometry {
                CellGraphGeometry::Axis { axis } => {
                    if e.anchors == 0 {
                        return Err(SurfaceError::Geometry);
                    }
                    let mut signs = [0; 3];
                    for (i, s) in signs.iter_mut().enumerate() {
                        let [p, q] = [a.gradient[i], b.gradient[i]];
                        if i == usize::from(axis) {
                            if p != 0 || q != 0 {
                                return Err(SurfaceError::Geometry);
                            }
                        } else {
                            // Each transverse component is affine on a zero
                            // line. A missing interior root or a persistent
                            // factor-plane family cannot be accepted as regular.
                            if (p == 0 && q == 0) || p * q < 0 {
                                return Err(SurfaceError::Geometry);
                            }
                            *s = (p + q).signum();
                        }
                    }
                    let context = RegularTangents::new(signs).map_err(error)?;
                    let tangent = TangentRay::Axis {
                        axis,
                        positive: a.coordinates[usize::from(axis)]
                            .compare(b.coordinates[usize::from(axis)])
                            .map_err(error)?
                            == Ordering::Less,
                    };
                    let mut allowed = [true; 2];
                    for face in 0..6 {
                        check(cancel)?;
                        if boundary & (1 << face) == 0 {
                            continue;
                        }
                        let boundary_direction = context
                            .cell_boundary(face / 2, face & 1 != 0)
                            .map_err(error)?;
                        let sign = context
                            .dot_sign(tangent, boundary_direction)
                            .map_err(error)?;
                        if sign == 0 {
                            return Err(SurfaceError::Geometry);
                        }
                        // Intersect all boundary-face constraints. Their union
                        // would fabricate a surface along an isolated zero edge.
                        allowed[0] &= sign > 0;
                        allowed[1] &= sign < 0;
                    }
                    allowed
                }
                CellGraphGeometry::Face { face } => {
                    if e.faces != 1 << face || e.anchors != 0 {
                        return Err(SurfaceError::Geometry);
                    }
                    let ascending_inside = (face & 1 != 0) == (face / 2 != 1);
                    let forward = e.face_forward == ascending_inside;
                    [forward, !forward]
                }
            };
            if e.eligible == [false; 2] {
                continue;
            }
            let first = self.halves.len();
            if first + 2 > self.halves.capacity() {
                return Err(SurfaceError::Limit);
            }
            e.first_half = Some(first as u16);
            for reverse in [false, true] {
                let direction = usize::from(reverse);
                self.halves.push(CellGraphHalfEdge {
                    edge: index as u16,
                    from: e.nodes[direction],
                    to: e.nodes[1 - direction],
                    twin: (first + 1 - direction) as u16,
                    next: u16::MAX,
                    eligible: e.eligible[direction],
                    region: None,
                });
            }
        }
        for node_id in 0..self.nodes.len() {
            check(cancel)?;
            let node = self.nodes[node_id];
            let context = if node.gradient == [0; 3] {
                None
            } else {
                Some(RegularTangents::new(node.gradient).map_err(error)?)
            };
            let star = if context.is_none() {
                let star = graph
                    .anchor_star(node.original.ok_or(SurfaceError::Geometry)?, cancel)
                    .map_err(error)?;
                if !matches!(star, AnchorStar::OrdinaryDouble { .. }) {
                    return Err(SurfaceError::Geometry);
                }
                Some(star)
            } else {
                None
            };
            let mut rings = [[u16::MAX; 12]; 2];
            let mut counts = [0usize; 2];
            let mut reference = None;
            for half in 0..self.halves.len() {
                check(cancel)?;
                if usize::from(self.halves[half].from) != node_id {
                    continue;
                }
                let group = if let Some(star) = &star {
                    self.double_position(half as u16, star)?.0
                } else {
                    0
                };
                let at = counts.get_mut(group).ok_or(SurfaceError::Geometry)?;
                *rings[group].get_mut(*at).ok_or(SurfaceError::Geometry)? = half as u16;
                let mut position = *at;
                *at += 1;
                if let Some(context) = context {
                    let incoming = self.tangent(half as u16, context)?;
                    let reference = *reference.get_or_insert(incoming);
                    while position > 0 {
                        check(cancel)?;
                        match context
                            .compare(
                                reference,
                                self.tangent(rings[group][position], context)?,
                                self.tangent(rings[group][position - 1], context)?,
                            )
                            .map_err(error)?
                        {
                            Ordering::Less => rings[group].swap(position, position - 1),
                            Ordering::Greater => break,
                            Ordering::Equal => return Err(SurfaceError::Geometry),
                        }
                        position -= 1;
                    }
                } else {
                    let star = star.as_ref().ok_or(SurfaceError::Geometry)?;
                    while position > 0 {
                        check(cancel)?;
                        match self
                            .double_position(rings[group][position], star)?
                            .1
                            .cmp(&self.double_position(rings[group][position - 1], star)?.1)
                        {
                            Ordering::Less => rings[group].swap(position, position - 1),
                            Ordering::Greater => break,
                            Ordering::Equal => return Err(SurfaceError::Geometry),
                        }
                        position -= 1;
                    }
                }
            }
            self.nodes[node_id].surface_degree = (counts[0] + counts[1]) as u8;
            for (ring, count) in rings.into_iter().zip(counts) {
                for i in 0..count {
                    check(cancel)?;
                    let incoming = self.halves[usize::from(ring[i])].twin;
                    let next = ring[(i + count - 1) % count];
                    let h = self
                        .halves
                        .get_mut(usize::from(incoming))
                        .ok_or(SurfaceError::Geometry)?;
                    if h.next != u16::MAX {
                        return Err(SurfaceError::Geometry);
                    }
                    h.next = next;
                }
            }
        }
        for h in &self.halves {
            check(cancel)?;
            let next = self
                .halves
                .get(usize::from(h.next))
                .ok_or(SurfaceError::Geometry)?;
            if next.from != h.to {
                return Err(SurfaceError::Geometry);
            }
        }
        for start in 0..self.halves.len() {
            check(cancel)?;
            if !self.halves[start].eligible || self.halves[start].region.is_some() {
                continue;
            }
            if self.regions.len() >= self.regions.capacity() {
                return Err(SurfaceError::Limit);
            }
            let id = self.regions.len() as u16;
            let mut count = 0;
            let mut current = start as u16;
            loop {
                check(cancel)?;
                if count >= self.halves.len() {
                    return Err(SurfaceError::Geometry);
                }
                let h = self
                    .halves
                    .get_mut(usize::from(current))
                    .ok_or(SurfaceError::Geometry)?;
                if !h.eligible || h.region.is_some() {
                    return Err(SurfaceError::Geometry);
                }
                h.region = Some(id);
                current = h.next;
                count += 1;
                if usize::from(current) == start {
                    break;
                }
            }
            self.regions.push(CellGraphRegion {
                first: start as u16,
                edge_count: count as u16,
            });
        }
        if field.phase() == CellPhase::Crossing && self.regions.is_empty() {
            return Err(SurfaceError::Geometry);
        }
        Ok(())
    }
}
