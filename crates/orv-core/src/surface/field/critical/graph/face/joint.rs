// SPDX-License-Identifier: GPL-3.0-or-later
//! Exact two-field region graph, with distinct arcs and interior constraints.
use super::*;
use intersections::primitive;
mod link;
mod refine;
pub use refine::{RefinedJointSubdivision, TriangulatedJointSubdivision};

const JOINT_NODES: usize = INSERTIONS + 18;
// One field creates <=4 regions. Adding two axis factors adds <=2 lines,
// <=2 intersections with that field and one self-cross: at most9 regions.
// An irreducible second field adds <=2 arcs and <=2 intersections, fewer.
const JOINT_REGIONS: usize = 9;
// Every nonisolated component reaches the square boundary; Euler gives E<=V+8.
const JOINT_EDGES: usize = JOINT_NODES + JOINT_REGIONS - 1;
const JOINT_HALVES: usize = 2 * JOINT_EDGES;
const _: () = assert!(JOINT_HALVES <= 256);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JointEdge {
    pub from: u8,
    pub to: u8,
    pub boundary: Option<u8>,
    /// Bits identify original fields whose zero curve owns this arc.
    pub curves: u8,
    /// Primitive curve or axis geometry. Distinct curves may share endpoints.
    pub geometry: CommonZero,
    inside_forward: bool,
    right_signs: [i8; 2],
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JointRegion {
    pub first: u8,
    pub edge_count: u16,
    /// Strict interior signs; zero is reserved for an identically zero field.
    pub signs: [i8; 2],
}
/// Exact curved region graph, not independent straight chords or a GPU mesh.
/// Every supplied point is retained, including nonzero interior constraints.
#[derive(Clone)]
pub struct JointSubdivision {
    fields: [Bilinear; 2],
    nodes: [Option<ContourNode>; JOINT_NODES],
    count: usize,
    edges: [Option<JointEdge>; JOINT_EDGES],
    edge_count: usize,
    halves: [Option<PatchHalfEdge>; JOINT_HALVES],
    regions: [Option<JointRegion>; JOINT_REGIONS],
    region_count: usize,
    interior: [Option<u8>; JOINT_NODES],
}
impl JointSubdivision {
    pub const MAX_INSERTIONS: usize = INSERTIONS;
    pub fn fields(&self) -> [Bilinear; 2] {
        self.fields
    }
    pub fn nodes(&self) -> impl Iterator<Item = (u8, &ContourNode)> {
        self.nodes
            .iter()
            .enumerate()
            .filter_map(|(i, n)| n.as_ref().map(|n| (i as u8, n)))
    }
    pub fn node(&self, id: u8) -> Option<&ContourNode> {
        self.nodes.get(usize::from(id))?.as_ref()
    }
    pub fn node_signs(&self, id: u8) -> Result<[i8; 2], FieldError> {
        let p = self.node(id).ok_or(FieldError::Bounds)?.coordinates;
        Ok([self.fields[0].sign_at(p)?, self.fields[1].sign_at(p)?])
    }
    /// Only isolated interior constraints have one containing region. Other
    /// vertices retain their incident regions through the half-edges.
    pub fn interior_region(&self, id: u8) -> Option<u8> {
        *self.interior.get(usize::from(id))?
    }
    pub fn edges(&self) -> impl Iterator<Item = (u8, &JointEdge)> {
        self.edges
            .iter()
            .enumerate()
            .filter_map(|(i, e)| e.as_ref().map(|e| (i as u8, e)))
    }
    pub fn edge(&self, id: u8) -> Option<&JointEdge> {
        self.edges.get(usize::from(id))?.as_ref()
    }
    pub fn half_edges(&self) -> impl Iterator<Item = (u8, &PatchHalfEdge)> {
        self.halves
            .iter()
            .enumerate()
            .filter_map(|(i, h)| h.as_ref().map(|h| (i as u8, h)))
    }
    pub fn half_edge(&self, id: u8) -> Option<&PatchHalfEdge> {
        self.halves.get(usize::from(id))?.as_ref()
    }
    pub fn regions(&self) -> impl Iterator<Item = (u8, &JointRegion)> {
        self.regions
            .iter()
            .enumerate()
            .filter_map(|(i, r)| r.as_ref().map(|r| (i as u8, r)))
    }
    fn add_edge(
        &mut self,
        from: u8,
        to: u8,
        geometry: CommonZero,
        boundary: Option<u8>,
        owner: Option<usize>,
    ) -> Result<(), FieldError> {
        if from == to || geometry == CommonZero::Whole {
            return Err(FieldError::Arithmetic);
        }
        let forward = from < to;
        let (a, b) = (from.min(to), from.max(to));
        let apply = |edge: &mut JointEdge| -> Result<(), FieldError> {
            if let Some(side) = boundary {
                if edge.boundary.is_some_and(|old| old != side) {
                    return Err(FieldError::Arithmetic);
                }
                edge.boundary = Some(side);
                edge.inside_forward = forward;
            }
            if let Some(field) = owner {
                let sign = if forward { 1 } else { -1 };
                if edge.curves & (1 << field) != 0 && edge.right_signs[field] != sign {
                    return Err(FieldError::Arithmetic);
                }
                edge.curves |= 1 << field;
                edge.right_signs[field] = sign;
            }
            Ok(())
        };
        for edge in self.edges.iter_mut().flatten() {
            if edge.from == a && edge.to == b && edge.geometry == geometry {
                return apply(edge);
            }
        }
        let mut edge = JointEdge {
            from: a,
            to: b,
            geometry,
            boundary: None,
            curves: 0,
            inside_forward: false,
            right_signs: [0; 2],
        };
        apply(&mut edge)?;
        *self
            .edges
            .get_mut(self.edge_count)
            .ok_or(FieldError::Capacity)? = Some(edge);
        self.edge_count += 1;
        Ok(())
    }
    fn connect(
        &mut self,
        spec: CurveSpec,
        member: impl Fn([ExactCoordinate; 2]) -> Result<bool, FieldError>,
        cancel: &impl Fn() -> bool,
    ) -> Result<(), FieldError> {
        let mut prior = None;
        for i in 0..self.count {
            check_cancel(cancel)?;
            let p = self.nodes[i].ok_or(FieldError::Arithmetic)?.coordinates;
            if self.fields[spec.owner].sign_at(p)? != 0 || !member(p)? {
                continue;
            }
            if let Some(previous) = prior {
                let before = self
                    .node(previous)
                    .ok_or(FieldError::Arithmetic)?
                    .coordinates;
                let a = bilinear_sign(spec.tangent, before)?;
                let b = bilinear_sign(spec.tangent, p)?;
                let sign = consistent_sign(a, b)?;
                if sign == 0 {
                    return Err(FieldError::Arithmetic);
                }
                let (from, to) = if sign > 0 {
                    (previous, i as u8)
                } else {
                    (i as u8, previous)
                };
                self.add_edge(from, to, spec.geometry, None, Some(spec.owner))?;
            }
            prior = Some(i as u8);
        }
        Ok(())
    }
    fn curves(&mut self, owner: usize, cancel: &impl Fn() -> bool) -> Result<(), FieldError> {
        let field = self.fields[owner];
        let [a, b, c, d] = coefficients(field);
        let determinant = i64::from(a) * i64::from(d) - i64::from(b) * i64::from(c);
        if d != 0 && determinant == 0 {
            for (axis, n, tangent) in [(0, c, [b, 0, d, 0]), (1, b, [-c, -d, 0, 0])] {
                let position = Rational::new(-i128::from(n), i128::from(d))?;
                let Some(at) = ExactCoordinate::from_rational(position)? else {
                    continue;
                };
                let spec = CurveSpec {
                    owner,
                    geometry: CommonZero::Axis {
                        axis: axis as u8,
                        position,
                    },
                    tangent,
                };
                self.connect(
                    spec,
                    |p| Ok(p[axis].compare(at)? == Ordering::Equal),
                    cancel,
                )?;
            }
        } else if d != 0 {
            for sign in [-1, 1] {
                let spec = CurveSpec {
                    owner,
                    geometry: CommonZero::Curve(primitive(field)),
                    tangent: [-c, -d, 0, 0],
                };
                self.connect(
                    spec,
                    |p| Ok(bilinear_sign([c, d, 0, 0], p)? == sign),
                    cancel,
                )?;
            }
        } else if c != 0 {
            let geometry = if b == 0 {
                CommonZero::Axis {
                    axis: 1,
                    position: Rational::new(-i128::from(a), i128::from(c))?,
                }
            } else {
                CommonZero::Curve(primitive(field))
            };
            self.connect(
                CurveSpec {
                    owner,
                    geometry,
                    tangent: [-c, 0, 0, 0],
                },
                |_| Ok(true),
                cancel,
            )?;
        } else if b != 0 {
            self.connect(
                CurveSpec {
                    owner,
                    geometry: CommonZero::Axis {
                        axis: 0,
                        position: Rational::new(-i128::from(a), i128::from(b))?,
                    },
                    tangent: [b, 0, 0, 0],
                },
                |_| Ok(true),
                cancel,
            )?;
        }
        Ok(())
    }
    fn boundary_sign(&self, edge: JointEdge, field: usize) -> Result<i8, FieldError> {
        let a = self
            .node(edge.from)
            .ok_or(FieldError::Arithmetic)?
            .coordinates;
        let b = self
            .node(edge.to)
            .ok_or(FieldError::Arithmetic)?
            .coordinates;
        let x = self.fields[field].sign_at(a)?;
        let y = self.fields[field].sign_at(b)?;
        if x != 0 || y != 0 {
            return consistent_sign(x, y);
        }
        let [_, u, v, mixed] = coefficients(self.fields[field]);
        let inward = match edge.boundary {
            Some(0) => [v, mixed, 0, 0],
            Some(1) => [-u, 0, -mixed, 0],
            Some(2) => [-v, -mixed, 0, 0],
            Some(3) => [u, 0, mixed, 0],
            _ => return Err(FieldError::Arithmetic),
        };
        consistent_sign(bilinear_sign(inward, a)?, bilinear_sign(inward, b)?)
    }
    fn along_sign(&self, edge: JointEdge, other: usize) -> Result<i8, FieldError> {
        let a = self
            .node(edge.from)
            .ok_or(FieldError::Arithmetic)?
            .coordinates;
        let b = self
            .node(edge.to)
            .ok_or(FieldError::Arithmetic)?
            .coordinates;
        let x = self.fields[other].sign_at(a)?;
        let y = self.fields[other].sign_at(b)?;
        if x != 0 || y != 0 {
            return consistent_sign(x, y);
        }
        if self.fields[other].values == [0; 4] {
            return Ok(0);
        }
        if !matches!(edge.geometry, CommonZero::Curve(_)) {
            return Err(FieldError::Arithmetic);
        }
        let owner = if edge.curves & 1 != 0 { 0 } else { 1 };
        let [_, b, _, d] = coefficients(self.fields[owner]).map(i64::from);
        let [_, f, _, h] = coefficients(self.fields[other]).map(i64::from);
        let quadratic = b * h - f * d;
        let [_, _, c, d] = coefficients(self.fields[owner]);
        // Both endpoints are zeros of the other field. On this open graph arc
        // R has these two roots, so sign(R)=-sign(R_quadratic). G=-R/Fv.
        let sign = quadratic.signum() as i8 * bilinear_sign([c, d, 0, 0], a)?;
        if sign == 0 {
            return Err(FieldError::Arithmetic);
        }
        Ok(sign)
    }
    fn locate_isolated(&mut self, cancel: &impl Fn() -> bool) -> Result<(), FieldError> {
        let mut incident = [false; JOINT_NODES];
        for (_, edge) in self.edges() {
            incident[usize::from(edge.from)] = true;
            incident[usize::from(edge.to)] = true;
        }
        for (i, is_incident) in incident.iter().copied().enumerate().take(self.count) {
            check_cancel(cancel)?;
            if is_incident {
                continue;
            }
            let point = self.nodes[i].ok_or(FieldError::Arithmetic)?.coordinates;
            let mut found = None;
            for (region_id, region) in self.regions() {
                let mut half = region.first;
                let mut active = 0usize;
                let mut inside = true;
                for _ in 0..region.edge_count {
                    check_cancel(cancel)?;
                    let h = self.half_edge(half).ok_or(FieldError::Arithmetic)?;
                    let from = self.node(h.from).ok_or(FieldError::Arithmetic)?.coordinates;
                    let to = self.node(h.to).ok_or(FieldError::Arithmetic)?.coordinates;
                    let du = from[0].compare(to[0])?;
                    let low = from[0].compare(point[0])?;
                    let high = to[0].compare(point[0])?;
                    if du != Ordering::Equal
                        && ((low != Ordering::Greater && high != Ordering::Less)
                            || (high != Ordering::Greater && low != Ordering::Less))
                    {
                        active += 1;
                        let edge = self.edge(h.edge).ok_or(FieldError::Arithmetic)?;
                        let relative = match edge.geometry {
                            CommonZero::Axis { axis: 1, position } => {
                                order_sign(point[1].compare(rational_coordinate(position)?)?)
                            }
                            CommonZero::Curve(_) => {
                                let owner = if edge.curves & 1 != 0 { 0 } else { 1 };
                                let [_, _, c, d] = coefficients(self.fields[owner]);
                                self.fields[owner].sign_at(point)?
                                    * bilinear_sign([c, d, 0, 0], point)?
                            }
                            _ => return Err(FieldError::Arithmetic),
                        };
                        inside &= if du == Ordering::Less {
                            relative > 0
                        } else {
                            relative < 0
                        };
                    }
                    half = h.next;
                }
                if inside && active >= 2 {
                    if found.is_some() {
                        return Err(FieldError::Arithmetic);
                    }
                    found = Some(region_id);
                }
            }
            self.interior[i] = Some(found.ok_or(FieldError::Arithmetic)?);
        }
        Ok(())
    }
}
#[derive(Clone, Copy)]
struct CurveSpec {
    owner: usize,
    geometry: CommonZero,
    tangent: [i32; 4],
}
fn consistent_sign(a: i8, b: i8) -> Result<i8, FieldError> {
    if a != 0 && b != 0 && a != b {
        Err(FieldError::Arithmetic)
    } else {
        Ok((a + b).signum())
    }
}
fn order_sign(v: Ordering) -> i8 {
    match v {
        Ordering::Less => -1,
        Ordering::Equal => 0,
        Ordering::Greater => 1,
    }
}
fn geometry_key(geometry: CommonZero) -> [i128; 6] {
    match geometry {
        CommonZero::Axis { axis, position } => [
            0,
            i128::from(axis),
            i128::from(position.numerator),
            i128::from(position.denominator),
            0,
            0,
        ],
        CommonZero::Curve(field) => {
            let [a, b, c, d] = field.values.map(i128::from);
            [1, a, b, c, d, 0]
        }
        CommonZero::Whole => [2, 0, 0, 0, 0, 0],
    }
}
impl Bilinear {
    /// Bounded joint arrangement, retaining up to80 arbitrary exact constraints.
    /// Curved arcs require coordinated refinement before triangle conversion.
    pub fn joint_subdivision(
        self,
        other: Self,
        insertions: &[[ExactCoordinate; 2]],
        cancel: impl Fn() -> bool,
    ) -> Result<JointSubdivision, FieldError> {
        check_cancel(&cancel)?;
        if insertions.len() > INSERTIONS {
            return Err(FieldError::Capacity);
        }
        // Two <=5-node seeds, <=4 pair events, four corners and80 constraints.
        let mut result = JointSubdivision {
            fields: [self, other],
            nodes: [None; JOINT_NODES],
            count: 0,
            edges: [None; JOINT_EDGES],
            edge_count: 0,
            halves: [None; JOINT_HALVES],
            regions: [None; JOINT_REGIONS],
            region_count: 0,
            interior: [None; JOINT_NODES],
        };
        for field in [self, other] {
            let seed = field.arrangement(&[], &cancel)?;
            for (_, node) in seed.nodes() {
                add_node(
                    &mut result.nodes,
                    &mut result.count,
                    node.coordinates,
                    &cancel,
                )?;
            }
        }
        let intersections = self.intersections(other, &cancel)?;
        for point in intersections.points() {
            add_node(
                &mut result.nodes,
                &mut result.count,
                *point.coordinates(),
                &cancel,
            )?;
        }
        for corner in [[false, false], [true, false], [true, true], [false, true]] {
            add_node(
                &mut result.nodes,
                &mut result.count,
                corner.map(ExactCoordinate::endpoint),
                &cancel,
            )?;
        }
        for point in insertions {
            check_cancel(&cancel)?;
            add_node(&mut result.nodes, &mut result.count, *point, &cancel)?;
        }
        canonicalize_nodes(&mut result.nodes, result.count, &cancel)?;
        for side in 0..4 {
            let (axis, upper, mask) = match side {
                0 => (1, false, 1 << 2),
                1 => (0, true, 1 << 1),
                2 => (1, true, 1 << 3),
                _ => (0, false, 1),
            };
            let mut prior = None;
            for i in 0..result.count {
                check_cancel(&cancel)?;
                if result.nodes[i].ok_or(FieldError::Arithmetic)?.boundary & mask == 0 {
                    continue;
                }
                if let Some(previous) = prior {
                    let (from, to) = if side < 2 {
                        (previous, i as u8)
                    } else {
                        (i as u8, previous)
                    };
                    result.add_edge(
                        from,
                        to,
                        CommonZero::Axis {
                            axis,
                            position: Rational::new(i128::from(upper), 1)?,
                        },
                        Some(side),
                        None,
                    )?;
                }
                prior = Some(i as u8);
            }
        }
        for field in 0..2 {
            result.curves(field, &cancel)?;
        }
        result.link(&cancel)?;
        result.locate_isolated(&cancel)?;
        check_cancel(&cancel)?;
        Ok(result)
    }
}

#[cfg(test)]
mod tests;
