// SPDX-License-Identifier: GPL-3.0-or-later
//! Complete canonical face/anchor insertion union, without rounded joins.
use super::*;
const _: () = assert!(CriticalGraph::MAX_LINES <= 8);
impl Scratch {
    fn node(&self, id: u16) -> Result<&CellGraphNode, SurfaceError> {
        self.nodes
            .get(usize::from(id))
            .ok_or(SurfaceError::Geometry)
    }
    fn add_node(
        &mut self,
        mut p: [ExactCoordinate; 3],
        faces: u8,
        original: Option<u8>,
        critical_axes: u8,
        limits: CellGraphLimits,
        cancel: &impl Fn() -> bool,
    ) -> Result<u16, SurfaceError> {
        for c in &mut p {
            if let Some(r) = c.rational_value() {
                *c = ExactCoordinate::from_rational(r)
                    .map_err(error)?
                    .ok_or(SurfaceError::Geometry)?;
            }
        }
        for (i, node) in self.nodes.iter_mut().enumerate() {
            check(cancel)?;
            let mut same = true;
            for (a, b) in node.coordinates.into_iter().zip(p) {
                if a.compare(b).map_err(error)? != Ordering::Equal {
                    same = false;
                    break;
                }
            }
            if same {
                if original.is_some() && node.original.is_some() && original != node.original {
                    return Err(SurfaceError::Geometry);
                }
                if original.is_some() {
                    node.original = original;
                }
                // Shared-face expressions, rather than a cell-local expression,
                // own boundary vertices. Shared source-edge ratios normalize above.
                if faces != 0 {
                    node.coordinates = p;
                }
                node.faces |= faces;
                node.critical_axes |= critical_axes;
                return Ok(i as u16);
            }
        }
        if self.nodes.len() >= limits.max_nodes {
            return Err(SurfaceError::Limit);
        }
        let mut boundary = 0;
        for (axis, c) in p.into_iter().enumerate() {
            for upper in [false, true] {
                check(cancel)?;
                if c.compare(endpoint(upper)?).map_err(error)? == Ordering::Equal {
                    boundary |= 1 << (2 * axis + usize::from(upper));
                }
            }
        }
        let id = self.nodes.len() as u16;
        self.nodes.push(CellGraphNode {
            coordinates: p,
            boundary,
            faces,
            original,
            critical_axes,
            gradient: [0; 3],
            surface_degree: 0,
        });
        Ok(id)
    }
    fn add_edge(
        &mut self,
        from: u16,
        to: u16,
        face: Option<u8>,
        anchor: Option<u8>,
        limits: CellGraphLimits,
        cancel: &impl Fn() -> bool,
    ) -> Result<(), SurfaceError> {
        if from == to {
            return Err(SurfaceError::Geometry);
        }
        let a = self.node(from)?.coordinates;
        let b = self.node(to)?.coordinates;
        let mut varying = [0usize; 3];
        let mut count = 0;
        for axis in 0..3 {
            if a[axis].compare(b[axis]).map_err(error)? != Ordering::Equal {
                varying[count] = axis;
                count += 1;
            }
        }
        let geometry = match count {
            1 => CellGraphGeometry::Axis {
                axis: varying[0] as u8,
            },
            2 => CellGraphGeometry::Face {
                face: face.ok_or(SurfaceError::Geometry)?,
            },
            _ => return Err(SurfaceError::Geometry),
        };
        if anchor.is_some() && !matches!(geometry, CellGraphGeometry::Axis { .. }) {
            return Err(SurfaceError::Geometry);
        }
        let nodes = [from.min(to), from.max(to)];
        let forward = from == nodes[0];
        let faces = face.map_or(0, |f| 1 << f);
        let anchors = anchor.map_or(0, |a| 1 << a);
        for e in &mut self.edges {
            check(cancel)?;
            if e.nodes == nodes && e.geometry == geometry {
                if matches!(geometry, CellGraphGeometry::Face { .. }) && e.face_forward != forward {
                    return Err(SurfaceError::Geometry);
                }
                e.faces |= faces;
                e.anchors |= anchors;
                return Ok(());
            }
        }
        if self.edges.len() >= limits.max_edges {
            return Err(SurfaceError::Limit);
        }
        self.edges.push(CellGraphEdge {
            nodes,
            geometry,
            faces,
            anchors,
            eligible: [false; 2],
            first_half: None,
            face_forward: forward,
        });
        Ok(())
    }
    pub(super) fn collect(
        &mut self,
        owner: &MaskField,
        cell: &FieldCell,
        graph: &CriticalGraph,
        limits: CellGraphLimits,
        cancel: &impl Fn() -> bool,
    ) -> Result<(), SurfaceError> {
        for (id, node) in graph.nodes() {
            self.add_node(
                *node.coordinates(),
                0,
                Some(id),
                node.critical_axes(),
                limits,
                cancel,
            )?;
        }
        for axis in 0..3 {
            for upper in [false, true] {
                check(cancel)?;
                let face_id = (2 * axis + usize::from(upper)) as u8;
                let (key, field) = cell.face(axis, upper).ok_or(SurfaceError::Geometry)?;
                let face = owner.source_face(key, cancel).map_err(error)?;
                let contour = face.contour().ok_or(SurfaceError::Geometry)?;
                if contour.is_whole_zero() || contour.field() != field {
                    return Err(SurfaceError::Geometry);
                }
                let [u, v] = transverse(axis);
                let mut map = [u16::MAX; FaceArrangement::MAX_NODES];
                for (id, node) in contour.nodes() {
                    check(cancel)?;
                    let mut p = [endpoint(upper)?; 3];
                    p[u] = node.coordinates()[0];
                    p[v] = node.coordinates()[1];
                    if field.sign_at([p[u], p[v]]).map_err(error)? != 0 {
                        return Err(SurfaceError::Geometry);
                    }
                    *map.get_mut(usize::from(id)).ok_or(SurfaceError::Geometry)? =
                        self.add_node(p, 1 << face_id, None, 0, limits, cancel)?;
                }
                for edge in contour.segments() {
                    check(cancel)?;
                    let from = *map
                        .get(usize::from(edge.from))
                        .ok_or(SurfaceError::Geometry)?;
                    let to = *map
                        .get(usize::from(edge.to))
                        .ok_or(SurfaceError::Geometry)?;
                    self.add_edge(from, to, Some(face_id), None, limits, cancel)?;
                }
            }
        }
        for node in &mut self.nodes {
            check(cancel)?;
            // Every aggregate boundary point must occur in every containing face.
            // A missing union event is not repaired by a nearest-vertex join.
            if node.faces != node.boundary {
                return Err(SurfaceError::Geometry);
            }
            node.gradient = graph
                .field()
                .gradient_signs(node.coordinates, cancel)
                .map_err(error)?;
        }
        for (id, line) in graph.lines() {
            check(cancel)?;
            let axis = line.axis();
            let [u, v] = transverse(axis);
            let fixed = line.fixed_coordinates();
            let mut ordered = [u16::MAX; NODES];
            let mut count = 0;
            for (i, node) in self.nodes.iter().enumerate() {
                check(cancel)?;
                if node.coordinates[u].compare(fixed[0]).map_err(error)? != Ordering::Equal
                    || node.coordinates[v].compare(fixed[1]).map_err(error)? != Ordering::Equal
                {
                    continue;
                }
                let mut at = count;
                while at > 0 {
                    check(cancel)?;
                    match self.node(ordered[at - 1])?.coordinates[axis]
                        .compare(node.coordinates[axis])
                        .map_err(error)?
                    {
                        Ordering::Less => break,
                        Ordering::Equal => return Err(SurfaceError::Geometry),
                        Ordering::Greater => {
                            ordered[at] = ordered[at - 1];
                            at -= 1;
                        }
                    }
                }
                ordered[at] = i as u16;
                count += 1;
            }
            if count < 2
                || self.node(ordered[0])?.coordinates[axis]
                    .compare(endpoint(false)?)
                    .map_err(error)?
                    != Ordering::Equal
                || self.node(ordered[count - 1])?.coordinates[axis]
                    .compare(endpoint(true)?)
                    .map_err(error)?
                    != Ordering::Equal
            {
                return Err(SurfaceError::Geometry);
            }
            for pair in ordered[..count].windows(2) {
                self.add_edge(pair[0], pair[1], None, Some(id), limits, cancel)?;
            }
        }
        Ok(())
    }
}
