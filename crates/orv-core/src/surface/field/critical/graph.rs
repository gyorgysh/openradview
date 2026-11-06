// SPDX-License-Identifier: GPL-3.0-or-later
//! Exact finite zero-line graph within one interpolation cell. Explicit family
//! diagnostics still require resolution before contour assembly or triangulation.
use super::*;
use std::cmp::Ordering;
mod compare;
mod face;
mod wide;
pub use face::{
    ContourNode, ContourSegment, FaceArrangement, PatchHalfEdge, PatchRegion, PlaneSubdivision,
    RefinedSubdivision, RefinementLimits, SubdivisionEdge,
};
use wide::radical_sign;
const MAX_FORM: u32 = 2040;
const MAX_LINES: usize = 6;
const MAX_NODES: usize = 18;
const MAX_EDGES: usize = MAX_LINES * (MAX_NODES - 1);
type RadicalForm = (i128, i128);

/// A unit-cell coordinate. Numerical equality requires `compare`;
/// different fractional expressions deliberately have no representation-based Eq.
#[derive(Clone, Copy, Debug)]
pub struct ExactCoordinate {
    parameter: AlgebraicRoot,
    numerator: Linear,
    denominator: Linear,
}
impl ExactCoordinate {
    /// Construct a rational coordinate, retaining the closed unit-cell bound.
    pub fn from_rational(value: Rational) -> Result<Option<Self>, FieldError> {
        Self::parameter(AlgebraicRoot(Root::Rational(value)))
    }
    fn unit(
        parameter: AlgebraicRoot,
        numerator: Linear,
        denominator: Linear,
    ) -> Result<Option<Self>, FieldError> {
        if [numerator, denominator]
            .into_iter()
            .any(|v| v.constant.unsigned_abs() > MAX_FORM || v.slope.unsigned_abs() > MAX_FORM)
        {
            return Err(FieldError::Arithmetic);
        }
        let sign = denominator.sign(parameter);
        if sign == 0 {
            return Err(FieldError::Arithmetic);
        }
        if numerator.sign(parameter) * sign < 0
            || denominator.subtract(numerator).sign(parameter) * sign < 0
        {
            return Ok(None);
        }
        Ok(Some(Self {
            parameter,
            numerator,
            denominator,
        }))
    }
    fn parameter(parameter: AlgebraicRoot) -> Result<Option<Self>, FieldError> {
        Self::unit(
            parameter,
            Linear {
                constant: 0,
                slope: 1,
            },
            Linear {
                constant: 1,
                slope: 0,
            },
        )
    }
    fn endpoint(upper: bool) -> Self {
        Self {
            parameter: AlgebraicRoot(Root::Rational(Rational {
                numerator: i64::from(upper),
                denominator: 1,
            })),
            numerator: Linear {
                constant: 0,
                slope: 1,
            },
            denominator: Linear {
                constant: 1,
                slope: 0,
            },
        }
    }
    pub fn rational_value(self) -> Option<Rational> {
        if let Some(at) = self.parameter.rational() {
            return Rational::new(self.numerator.scaled(at), self.denominator.scaled(at)).ok();
        }
        let n = self.numerator;
        let d = self.denominator;
        if i64::from(n.constant) * i64::from(d.slope) != i64::from(n.slope) * i64::from(d.constant)
        {
            return None;
        }
        if d.constant != 0 {
            Rational::new(i128::from(n.constant), i128::from(d.constant)).ok()
        } else {
            Rational::new(i128::from(n.slope), i128::from(d.slope)).ok()
        }
    }
    pub fn approximate(self) -> Option<f64> {
        if let Some(value) = self.rational_value() {
            return Some(value.as_f64());
        }
        let value = self.numerator.approximate(self.parameter)
            / self.denominator.approximate(self.parameter);
        (0. ..=1.).contains(&value).then_some(value)
    }
    /// Exact within one cell's quadratic field. Unrelated irrational fields are
    /// refused, never joined or ordered using a floating approximation.
    pub fn compare_same_field(self, other: Self) -> Result<Ordering, FieldError> {
        let left = self.rational_value();
        let right = other.rational_value();
        let left_d = self.radicand(left.is_some());
        let right_d = other.radicand(right.is_some());
        let d = common_radicand(left_d, right_d);
        let (a, b) = self.forms(left, d)?;
        let (c, e) = other.forms(right, d)?;
        let p = sum([
            product(a.0, e.0)?,
            product(a.1, e.1)?
                .checked_mul(i128::from(d))
                .ok_or(FieldError::Arithmetic)?,
            -product(c.0, b.0)?,
            -product(c.1, b.1)?
                .checked_mul(i128::from(d))
                .ok_or(FieldError::Arithmetic)?,
        ])?;
        let q = sum([
            product(a.0, e.1)?,
            product(a.1, e.0)?,
            -product(c.0, b.1)?,
            -product(c.1, b.0)?,
        ])?;
        let left_sign = if left.is_some() {
            1
        } else {
            self.denominator.sign(self.parameter)
        };
        let right_sign = if right.is_some() {
            1
        } else {
            other.denominator.sign(other.parameter)
        };
        let sign = radical_sign(p, q, d)? * left_sign * right_sign;
        Ok(sign.cmp(&0))
    }
    fn radicand(self, rational: bool) -> u64 {
        if rational {
            return 0;
        }
        match self.parameter.0 {
            Root::Surd { a, b, c, .. } => discriminant(a, b, c) as u64,
            Root::Rational(_) => 0,
        }
    }
    fn forms(
        self,
        rational: Option<Rational>,
        d: u64,
    ) -> Result<(RadicalForm, RadicalForm), FieldError> {
        if let Some(value) = rational {
            return Ok((
                (i128::from(value.numerator), 0),
                (i128::from(value.denominator), 0),
            ));
        }
        let Root::Surd { a, b, c, upper } = self.parameter.0 else {
            return Err(FieldError::Arithmetic);
        };
        let original = discriminant(a, b, c) as u64;
        if d == 0 || !original.is_multiple_of(d) {
            return Err(FieldError::Arithmetic);
        }
        let quotient = original / d;
        let factor = quotient.isqrt();
        if factor * factor != quotient {
            return Err(FieldError::Arithmetic);
        }
        let form = |value: Linear| {
            (
                2 * i128::from(a) * i128::from(value.constant)
                    - i128::from(b) * i128::from(value.slope),
                i128::from(value.slope) * i128::from(factor) * if upper { 1 } else { -1 },
            )
        };
        Ok((form(self.numerator), form(self.denominator)))
    }
}
fn product(a: i128, b: i128) -> Result<i128, FieldError> {
    a.checked_mul(b).ok_or(FieldError::Arithmetic)
}
fn sum(values: [i128; 4]) -> Result<i128, FieldError> {
    values
        .into_iter()
        .try_fold(0i128, |a, b| a.checked_add(b).ok_or(FieldError::Arithmetic))
}
fn common_radicand(mut a: u64, mut b: u64) -> u64 {
    if a == 0 {
        return b;
    }
    if b == 0 {
        return a;
    }
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}
impl ProjectionPoint {
    pub fn coordinate(self, axis: usize) -> Result<ExactCoordinate, FieldError> {
        if axis >= 3 {
            return Err(FieldError::Bounds);
        }
        if axis == self.axis() {
            return ExactCoordinate::parameter(self.parameter)?.ok_or(FieldError::Arithmetic);
        }
        let index = usize::from(transverse_axes(self.axis())[1] == axis);
        ExactCoordinate::unit(self.parameter, self.numerators[index], self.denominator)?
            .ok_or(FieldError::Arithmetic)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct GraphNode {
    coordinates: [ExactCoordinate; 3],
    critical_axes: u8,
    boundary: u8,
}
impl GraphNode {
    pub fn coordinates(&self) -> &[ExactCoordinate; 3] {
        &self.coordinates
    }
    pub fn critical_axes(&self) -> u8 {
        self.critical_axes
    }
    pub fn boundary(&self) -> u8 {
        self.boundary
    }
    pub fn approximate(&self) -> Option<[f64; 3]> {
        Some([
            self.coordinates[0].approximate()?,
            self.coordinates[1].approximate()?,
            self.coordinates[2].approximate()?,
        ])
    }
    fn new(coordinates: [ExactCoordinate; 3], critical_axes: u8) -> Result<Self, FieldError> {
        let mut mask = 0;
        for (axis, value) in coordinates.into_iter().enumerate() {
            mask |= u8::from(
                value.compare_same_field(ExactCoordinate::endpoint(false))? == Ordering::Equal,
            ) << (2 * axis);
            mask |= u8::from(
                value.compare_same_field(ExactCoordinate::endpoint(true))? == Ordering::Equal,
            ) << (2 * axis + 1);
        }
        Ok(Self {
            coordinates,
            critical_axes,
            boundary: mask,
        })
    }
}
#[derive(Clone, Copy, Debug)]
pub struct AnchorLine {
    axis: u8,
    fixed: [ExactCoordinate; 2],
}
impl AnchorLine {
    pub fn axis(&self) -> usize {
        usize::from(self.axis)
    }
    pub fn fixed_coordinates(&self) -> &[ExactCoordinate; 2] {
        &self.fixed
    }
    fn endpoint(self, upper: bool) -> [ExactCoordinate; 3] {
        let mut point = [ExactCoordinate::endpoint(upper); 3];
        for (axis, value) in transverse_axes(self.axis()).into_iter().zip(self.fixed) {
            point[axis] = value;
        }
        point
    }
    fn contains(self, node: &GraphNode) -> Result<bool, FieldError> {
        for (axis, value) in transverse_axes(self.axis()).into_iter().zip(self.fixed) {
            if value.compare_same_field(node.coordinates[axis])? != Ordering::Equal {
                return Ok(false);
            }
        }
        Ok(true)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GraphEdge {
    pub line: u8,
    pub from: u8,
    pub to: u8,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WholeSection {
    pub axis: u8,
    pub parameter: AlgebraicRoot,
}
/// Finite critical points, axis-aligned zero lines on resolved eliminations, and
/// ordered segments. Family/whole-section diagnostics still require resolution.
/// This is not a contour arrangement or mesh, and an empty graph does not imply
/// absent mask membership or resolve the source's finite-grid truncation faces.
pub struct CriticalGraph {
    field: Trilinear,
    nodes: [Option<GraphNode>; MAX_NODES],
    node_count: usize,
    lines: [Option<AnchorLine>; MAX_LINES],
    line_count: usize,
    edges: [Option<GraphEdge>; MAX_EDGES],
    edge_count: usize,
    sections: [Option<WholeSection>; 6],
    section_count: usize,
    projection_families: u8,
    line_families: u8,
}
impl CriticalGraph {
    pub fn field(&self) -> Trilinear {
        self.field
    }
    pub fn nodes(&self) -> impl Iterator<Item = (u8, &GraphNode)> {
        self.nodes
            .iter()
            .enumerate()
            .filter_map(|(i, v)| v.as_ref().map(|v| (i as u8, v)))
    }
    pub fn lines(&self) -> impl Iterator<Item = (u8, &AnchorLine)> {
        self.lines
            .iter()
            .enumerate()
            .filter_map(|(i, v)| v.as_ref().map(|v| (i as u8, v)))
    }
    pub fn edges(&self) -> impl Iterator<Item = &GraphEdge> {
        self.edges.iter().flatten()
    }
    pub fn whole_sections(&self) -> impl Iterator<Item = &WholeSection> {
        self.sections.iter().flatten()
    }
    pub fn node(&self, index: u8) -> Option<&GraphNode> {
        self.nodes.get(usize::from(index))?.as_ref()
    }
    pub fn unresolved_projection_axes(&self) -> u8 {
        self.projection_families
    }
    pub fn unresolved_line_axes(&self) -> u8 {
        self.line_families
    }
    pub fn requires_family_resolution(&self) -> bool {
        self.projection_families != 0 || self.line_families != 0 || self.section_count != 0
    }
    fn add_node(&mut self, node: GraphNode) -> Result<u8, FieldError> {
        for index in 0..self.node_count {
            let prior = self.nodes[index].as_mut().ok_or(FieldError::Arithmetic)?;
            let mut same = true;
            for (a, b) in prior.coordinates.into_iter().zip(node.coordinates) {
                if a.compare_same_field(b)? != Ordering::Equal {
                    same = false;
                    break;
                }
            }
            if same {
                prior.critical_axes |= node.critical_axes;
                return Ok(index as u8);
            }
        }
        let slot = self
            .nodes
            .get_mut(self.node_count)
            .ok_or(FieldError::Arithmetic)?;
        *slot = Some(node);
        self.node_count += 1;
        Ok((self.node_count - 1) as u8)
    }
    fn add_line(&mut self, line: AnchorLine) -> Result<(), FieldError> {
        for prior in self.lines.iter().flatten() {
            if prior.axis == line.axis
                && prior.fixed[0].compare_same_field(line.fixed[0])? == Ordering::Equal
                && prior.fixed[1].compare_same_field(line.fixed[1])? == Ordering::Equal
            {
                return Ok(());
            }
        }
        *self
            .lines
            .get_mut(self.line_count)
            .ok_or(FieldError::Arithmetic)? = Some(line);
        self.line_count += 1;
        Ok(())
    }
    fn add_section(&mut self, axis: usize, parameter: AlgebraicRoot) -> Result<(), FieldError> {
        for prior in self.sections.iter().flatten() {
            if usize::from(prior.axis) == axis && prior.parameter == parameter {
                return Ok(());
            }
        }
        *self
            .sections
            .get_mut(self.section_count)
            .ok_or(FieldError::Arithmetic)? = Some(WholeSection {
            axis: axis as u8,
            parameter,
        });
        self.section_count += 1;
        Ok(())
    }
}
fn section_forms(field: Trilinear, axis: usize) -> Result<[Linear; 4], FieldError> {
    let lo = field
        .face(axis, false)
        .ok_or(FieldError::Bounds)?
        .values
        .map(i32::from);
    let hi = field
        .face(axis, true)
        .ok_or(FieldError::Bounds)?
        .values
        .map(i32::from);
    let form = |f: fn([i32; 4]) -> i32| Linear {
        constant: f(lo),
        slope: f(hi) - f(lo),
    };
    Ok([
        form(|v| v[0]),
        form(|v| v[1] - v[0]),
        form(|v| v[3] - v[0]),
        form(|v| v[0] - v[1] + v[2] - v[3]),
    ])
}
fn negative(value: Linear) -> Linear {
    Linear {
        constant: -value.constant,
        slope: -value.slope,
    }
}
fn check_cancel(cancel: &impl Fn() -> bool) -> Result<(), FieldError> {
    if cancel() {
        Err(FieldError::Cancelled)
    } else {
        Ok(())
    }
}
impl Trilinear {
    pub fn critical_graph(self, cancel: impl Fn() -> bool) -> Result<CriticalGraph, FieldError> {
        check_cancel(&cancel)?;
        let mut graph = CriticalGraph {
            field: self,
            nodes: [None; MAX_NODES],
            node_count: 0,
            lines: [None; MAX_LINES],
            line_count: 0,
            edges: [None; MAX_EDGES],
            edge_count: 0,
            sections: [None; 6],
            section_count: 0,
            projection_families: 0,
            line_families: 0,
        };
        if self.values.iter().all(|v| *v > 0) || self.values.iter().all(|v| *v < 0) {
            check_cancel(&cancel)?;
            return Ok(graph);
        }
        for axis in 0..3 {
            check_cancel(&cancel)?;
            match self.projection_critical(axis)? {
                ProjectionAnalysis::Degenerate => graph.projection_families |= 1 << axis,
                ProjectionAnalysis::Finite {
                    points,
                    whole_sections,
                } => {
                    for point in points.into_iter().flatten() {
                        graph.add_node(GraphNode::new(
                            [
                                point.coordinate(0)?,
                                point.coordinate(1)?,
                                point.coordinate(2)?,
                            ],
                            1 << axis,
                        )?)?;
                    }
                    for parameter in whole_sections.into_iter().flatten() {
                        graph.add_section(axis, parameter)?;
                    }
                }
            }
        }
        for axis in 0..3 {
            check_cancel(&cancel)?;
            let [fixed_axis, sweep] = transverse_axes(axis);
            let [a, b, c, d] = section_forms(self, sweep)?;
            let (chosen, other) = if transverse_axes(sweep)[0] == axis {
                (b, c)
            } else {
                (c, b)
            };
            let Roots::Finite(roots) =
                roots(self.section_determinant(sweep).ok_or(FieldError::Bounds)?)?
            else {
                graph.line_families |= 1 << axis;
                continue;
            };
            for root in roots.into_iter().flatten() {
                let Some(t) = ExactCoordinate::parameter(root)? else {
                    continue;
                };
                let coordinate = if d.sign(root) != 0 {
                    ExactCoordinate::unit(root, negative(chosen), d)?
                } else if chosen.sign(root) != 0 {
                    None
                } else if other.sign(root) != 0 {
                    ExactCoordinate::unit(root, negative(a), other)?
                } else {
                    if a.sign(root) == 0 {
                        graph.add_section(sweep, root)?;
                    }
                    None
                };
                if let Some(coordinate) = coordinate {
                    let mut fixed = [t; 2];
                    for (i, a) in transverse_axes(axis).into_iter().enumerate() {
                        if a == fixed_axis {
                            fixed[i] = coordinate;
                        }
                    }
                    graph.add_line(AnchorLine {
                        axis: axis as u8,
                        fixed,
                    })?;
                }
            }
        }
        for index in 0..graph.line_count {
            check_cancel(&cancel)?;
            let line = graph.lines[index].ok_or(FieldError::Arithmetic)?;
            for upper in [false, true] {
                graph.add_node(GraphNode::new(line.endpoint(upper), 0)?)?;
            }
        }
        for index in 0..graph.line_count {
            check_cancel(&cancel)?;
            let line = graph.lines[index].ok_or(FieldError::Arithmetic)?;
            let mut ordered = [0u8; MAX_NODES];
            let mut count = 0;
            for node in 0..graph.node_count {
                if !line.contains(graph.nodes[node].as_ref().ok_or(FieldError::Arithmetic)?)? {
                    continue;
                }
                let mut at = count;
                while at > 0 {
                    let left = graph.nodes[usize::from(ordered[at - 1])]
                        .as_ref()
                        .ok_or(FieldError::Arithmetic)?
                        .coordinates[line.axis()];
                    let right = graph.nodes[node]
                        .as_ref()
                        .ok_or(FieldError::Arithmetic)?
                        .coordinates[line.axis()];
                    match left.compare_same_field(right)? {
                        Ordering::Less => break,
                        Ordering::Equal => return Err(FieldError::Arithmetic),
                        Ordering::Greater => {
                            ordered[at] = ordered[at - 1];
                            at -= 1;
                        }
                    }
                }
                ordered[at] = node as u8;
                count += 1;
            }
            for pair in ordered[..count].windows(2) {
                *graph
                    .edges
                    .get_mut(graph.edge_count)
                    .ok_or(FieldError::Arithmetic)? = Some(GraphEdge {
                    line: index as u8,
                    from: pair[0],
                    to: pair[1],
                });
                graph.edge_count += 1;
            }
        }
        check_cancel(&cancel)?;
        Ok(graph)
    }
}

#[cfg(test)]
mod tests;
