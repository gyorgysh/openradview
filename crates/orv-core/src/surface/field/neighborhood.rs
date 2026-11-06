// SPDX-License-Identifier: GPL-3.0-or-later
//! Source-bound canonical edge partitions and face insertion neighborhoods.
use super::*;
mod cell;
mod zero_face;
pub use cell::{
    CellFiber, CellGraphEdge, CellGraphGeometry, CellGraphHalfEdge, CellGraphLimits, CellGraphNode,
    CellGraphRegion, CellGraphState, CellPathLimits, CellPathOrigin, CellPathVertex,
    CellProjectionChart, ChartEdgeStep, ProjectedCellEdges, ProjectedContact,
    ProjectedContactGeometry, ProjectedContactOwner, ProjectedContacts, ProjectedEdge,
    ProjectedEdgeKind, RefinedCellGraph, SourceCellGraph,
};
pub use zero_face::{
    WholeZeroFace, WholeZeroSubdivision, ZeroFaceMesh, ZeroFacePatch, ZeroFacePhase, ZeroFaceSample,
};
const EDGE_POINTS: usize = 10;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CellNeighborhood {
    Outside,
    Unavailable {
        invalid_corners: u8,
        unknown_corners: u8,
    },
    Known,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NeighborhoodCell {
    origin: Option<[u32; 3]>,
    state: CellNeighborhood,
    interpolant: Option<Trilinear>,
}
impl NeighborhoodCell {
    pub fn origin(&self) -> Option<[u32; 3]> {
        self.origin
    }
    pub fn state(&self) -> CellNeighborhood {
        self.state
    }
    pub fn interpolant(&self) -> Option<Trilinear> {
        self.interpolant
    }
    pub fn phase(&self) -> Option<CellPhase> {
        self.interpolant.map(Trilinear::phase)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EdgePoint {
    position: Rational,
    forms: u8,
}
impl EdgePoint {
    pub fn position(&self) -> Rational {
        self.position
    }
    /// Bits0..3 name the four normal half-faces, bits4..7 the four mixed
    /// cell coefficients. Endpoints can have no coefficient-root provenance.
    pub fn forms(&self) -> u8 {
        self.forms
    }
}
/// A finite parameter partition, not a cell-topology or closure decision.
/// For an entire zero edge, each known cell expands as
/// F(s,r,t)=B(s)r+C(s)t+D(s)rt with r,t directed into that cell.
/// Every finite root of B,C,D is retained, including higher-order attachments.
pub struct EdgePartition {
    field: MaskField,
    key: EdgeKey,
    endpoints: [FieldSample; 2],
    zero: Option<EdgeZero>,
    cells: [NeighborhoodCell; 4],
    normal_samples: [Option<[FieldSample; 2]>; 4],
    normal_forms: [Option<[i16; 2]>; 4],
    mixed_forms: [Option<[i16; 2]>; 4],
    points: [Option<EdgePoint>; EDGE_POINTS],
    count: usize,
}
impl EdgePartition {
    pub fn field(&self) -> &MaskField {
        &self.field
    }
    pub fn key(&self) -> EdgeKey {
        self.key
    }
    pub fn endpoints(&self) -> &[FieldSample; 2] {
        &self.endpoints
    }
    pub fn zero(&self) -> Option<EdgeZero> {
        self.zero
    }
    /// Slot bits specify whether the edge is on the upper side of the cell
    /// along each ascending transverse axis. Outside is distinct from unknown.
    pub fn cells(&self) -> &[NeighborhoodCell; 4] {
        &self.cells
    }
    /// For each ascending transverse axis: positive half-face, negative half-face.
    /// None is outside; unknown/invalid offset samples remain distinct inside.
    pub fn normal_samples(&self) -> &[Option<[FieldSample; 2]>; 4] {
        &self.normal_samples
    }
    /// Endpoint values of each normal coefficient, where its stencil is known.
    /// Some([0,0]) is an entire family, never an invented isolated root.
    pub fn normal_forms(&self) -> &[Option<[i16; 2]>; 4] {
        &self.normal_forms
    }
    pub fn mixed_forms(&self) -> &[Option<[i16; 2]>; 4] {
        &self.mixed_forms
    }
    pub fn points(&self) -> impl Iterator<Item = &EdgePoint> {
        self.points.iter().flatten()
    }
    fn add(
        &mut self,
        position: Rational,
        forms: u8,
        cancel: &impl Fn() -> bool,
    ) -> Result<(), FieldError> {
        for point in self.points.iter_mut().flatten() {
            check_cancel(cancel)?;
            if point.position == position {
                point.forms |= forms;
                return Ok(());
            }
        }
        *self
            .points
            .get_mut(self.count)
            .ok_or(FieldError::Capacity)? = Some(EdgePoint { position, forms });
        self.count += 1;
        Ok(())
    }
}
/// One canonical face, retaining its exact mask/source/revision owner, both
/// incident cell graphs and all four complete edge neighborhoods. Unknown
/// samples leave the contour unavailable; absent outside cells create no caps.
pub struct SourceFace {
    field: MaskField,
    key: FaceKey,
    samples: [FieldSample; 4],
    cells: [NeighborhoodCell; 2],
    graphs: [Option<CriticalGraph>; 2],
    factorizations: [Option<CellFactorization>; 2],
    edges: [EdgePartition; 4],
    contour: Option<FaceArrangement>,
}
impl SourceFace {
    pub fn field(&self) -> &MaskField {
        &self.field
    }
    pub fn key(&self) -> FaceKey {
        self.key
    }
    pub fn samples(&self) -> &[FieldSample; 4] {
        &self.samples
    }
    /// Negative-normal cell first, positive-normal cell second.
    pub fn cells(&self) -> &[NeighborhoodCell; 2] {
        &self.cells
    }
    /// Unresolved eliminations and whole sections stay attached to their cell.
    pub fn graphs(&self) -> &[Option<CriticalGraph>; 2] {
        &self.graphs
    }
    pub fn factorizations(&self) -> &[Option<CellFactorization>; 2] {
        &self.factorizations
    }
    pub fn edges(&self) -> &[EdgePartition; 4] {
        &self.edges
    }
    pub fn contour(&self) -> Option<&FaceArrangement> {
        self.contour.as_ref()
    }
}
fn transverse(axis: usize) -> [usize; 2] {
    [usize::from(axis == 0), if axis == 2 { 1 } else { 2 }]
}
fn check_cancel(cancel: &impl Fn() -> bool) -> Result<(), FieldError> {
    if cancel() {
        Err(FieldError::Cancelled)
    } else {
        Ok(())
    }
}
fn as_usize(origin: [u32; 3]) -> Result<[usize; 3], FieldError> {
    Ok([
        usize::try_from(origin[0]).map_err(|_| FieldError::Bounds)?,
        usize::try_from(origin[1]).map_err(|_| FieldError::Bounds)?,
        usize::try_from(origin[2]).map_err(|_| FieldError::Bounds)?,
    ])
}
fn coordinate(position: Rational) -> Result<ExactCoordinate, FieldError> {
    ExactCoordinate::from_rational(position)?.ok_or(FieldError::Arithmetic)
}
impl MaskField {
    fn neighborhood_cell(&self, origin: Option<[u32; 3]>) -> Result<NeighborhoodCell, FieldError> {
        let outside = NeighborhoodCell {
            origin: None,
            state: CellNeighborhood::Outside,
            interpolant: None,
        };
        let Some(origin) = origin else {
            return Ok(outside);
        };
        let lower = as_usize(origin)?;
        let dimensions = self.source.dimensions();
        if (0..3).any(|a| lower[a].checked_add(1).is_none_or(|v| v >= dimensions[a])) {
            return Ok(outside);
        }
        let cell = self.cell(lower)?;
        let mut invalid_corners = 0;
        let mut unknown_corners = 0;
        for (i, sample) in cell.corners.into_iter().enumerate() {
            invalid_corners |= u8::from(!sample.source_valid) << i;
            unknown_corners |= u8::from(sample.value.is_none()) << i;
        }
        let interpolant = cell.interpolant();
        Ok(NeighborhoodCell {
            origin: Some(origin),
            state: if interpolant.is_some() {
                CellNeighborhood::Known
            } else {
                CellNeighborhood::Unavailable {
                    invalid_corners,
                    unknown_corners,
                }
            },
            interpolant,
        })
    }
    /// Worker-side bounded read of all four incident cells. The key is global
    /// within this retained source, never relative to an arbitrary caller cell.
    pub fn edge_partition(
        &self,
        key: EdgeKey,
        cancel: impl Fn() -> bool,
    ) -> Result<EdgePartition, FieldError> {
        check_cancel(&cancel)?;
        let axis = usize::from(key.axis);
        if axis >= 3 {
            return Err(FieldError::Bounds);
        }
        let origin = as_usize(key.origin)?;
        let dimensions = self.source.dimensions();
        if (0..3).any(|a| origin[a] >= dimensions[a])
            || origin[axis]
                .checked_add(1)
                .is_none_or(|v| v >= dimensions[axis])
        {
            return Err(FieldError::Bounds);
        }
        let mut end = origin;
        end[axis] += 1;
        let endpoints = [
            self.sample(origin).ok_or(FieldError::Bounds)?,
            self.sample(end).ok_or(FieldError::Bounds)?,
        ];
        let zero = if endpoints
            .iter()
            .all(|s| s.source_valid && s.value.is_some())
        {
            Some(
                edge_zero(
                    endpoints[0].value.ok_or(FieldError::Arithmetic)?,
                    endpoints[1].value.ok_or(FieldError::Arithmetic)?,
                )
                .ok_or(FieldError::Arithmetic)?,
            )
        } else {
            None
        };
        let outside = NeighborhoodCell {
            origin: None,
            state: CellNeighborhood::Outside,
            interpolant: None,
        };
        let mut result = EdgePartition {
            field: self.clone(),
            key,
            endpoints,
            zero,
            cells: [outside; 4],
            normal_samples: [None; 4],
            normal_forms: [None; 4],
            mixed_forms: [None; 4],
            points: [None; EDGE_POINTS],
            count: 0,
        };
        match zero {
            Some(EdgeZero::Point(t)) => result.add(t, 0, &cancel)?,
            Some(EdgeZero::Entire) => {
                result.add(Rational::new(0, 1)?, 0, &cancel)?;
                result.add(Rational::new(1, 1)?, 0, &cancel)?;
            }
            _ => {}
        }
        let normals = transverse(axis);
        // A known face can exist even when its incident cell's diagonal samples
        // are unavailable. Grid boundary stencils remain explicit too.
        // Read these four half-face stencils independently of complete cells.
        for (index, normal) in normals.into_iter().enumerate() {
            for negative in [false, true] {
                check_cancel(&cancel)?;
                let slot = 2 * index + usize::from(negative);
                let offset = if negative {
                    origin[normal].checked_sub(1)
                } else {
                    origin[normal]
                        .checked_add(1)
                        .filter(|v| *v < dimensions[normal])
                };
                let Some(offset) = offset else {
                    continue;
                };
                let mut first = origin;
                first[normal] = offset;
                let mut last = first;
                last[axis] += 1;
                let samples = [
                    self.sample(first).ok_or(FieldError::Bounds)?,
                    self.sample(last).ok_or(FieldError::Bounds)?,
                ];
                result.normal_samples[slot] = Some(samples);
                if endpoints
                    .into_iter()
                    .chain(samples)
                    .all(|s| s.source_valid && s.value.is_some())
                {
                    result.normal_forms[slot] = Some([
                        samples[0].value.ok_or(FieldError::Arithmetic)?
                            - endpoints[0].value.ok_or(FieldError::Arithmetic)?,
                        samples[1].value.ok_or(FieldError::Arithmetic)?
                            - endpoints[1].value.ok_or(FieldError::Arithmetic)?,
                    ]);
                }
            }
        }
        for slot in 0..4 {
            check_cancel(&cancel)?;
            let mut lower = Some(key.origin);
            let mut corner = 0;
            for (bit, normal) in normals.into_iter().enumerate() {
                if slot & (1 << bit) != 0 {
                    lower = lower.and_then(|mut p| {
                        p[normal] = p[normal].checked_sub(1)?;
                        Some(p)
                    });
                    corner |= 1 << normal;
                }
            }
            let cell = self.neighborhood_cell(lower)?;
            result.cells[slot] = cell;
            let Some(field) = cell.interpolant else {
                continue;
            };
            let values = field.values;
            result.mixed_forms[slot] = Some(std::array::from_fn(|s| {
                let at = corner | (s << axis);
                let a = values[at];
                let b = values[at ^ (1 << normals[0])];
                let c = values[at ^ (1 << normals[1])];
                values[at ^ (1 << normals[0]) ^ (1 << normals[1])] - b - c + a
            }));
        }
        // Four normal and four mixed linear forms contribute at most eight
        // distinct roots; with endpoints the complete finite partition fits10.
        if zero == Some(EdgeZero::Entire) {
            for (form, values) in result
                .normal_forms
                .into_iter()
                .chain(result.mixed_forms)
                .enumerate()
            {
                check_cancel(&cancel)?;
                let Some([a, b]) = values else {
                    continue;
                };
                if a == b {
                    continue;
                }
                let t = Rational::new(-i128::from(a), i128::from(b) - i128::from(a))?;
                if t.numerator >= 0 && i128::from(t.numerator) <= i128::from(t.denominator) {
                    result.add(t, 1 << form, &cancel)?;
                }
            }
        }
        for i in 1..result.count {
            let mut j = i;
            while j > 0 {
                check_cancel(&cancel)?;
                let a = result.points[j - 1].ok_or(FieldError::Arithmetic)?.position;
                let b = result.points[j].ok_or(FieldError::Arithmetic)?.position;
                if i128::from(a.numerator) * i128::from(b.denominator)
                    <= i128::from(b.numerator) * i128::from(a.denominator)
                {
                    break;
                }
                result.points.swap(j - 1, j);
                j -= 1;
            }
        }
        check_cancel(&cancel)?;
        Ok(result)
    }
    /// Worker-side canonical face assembly, including finite cell anchors and
    /// roots of all inward normal/mixed-normal edge coefficients. This partitions
    /// known neighborhoods without claiming complete 3D family resolution.
    pub fn source_face(
        &self,
        key: FaceKey,
        cancel: impl Fn() -> bool,
    ) -> Result<SourceFace, FieldError> {
        check_cancel(&cancel)?;
        let axis = usize::from(key.axis);
        if axis >= 3 {
            return Err(FieldError::Bounds);
        }
        let origin = as_usize(key.origin)?;
        let dimensions = self.source.dimensions();
        let [u, v] = transverse(axis);
        if origin[axis] >= dimensions[axis]
            || [u, v]
                .into_iter()
                .any(|a| origin[a].checked_add(1).is_none_or(|x| x >= dimensions[a]))
        {
            return Err(FieldError::Bounds);
        }
        let mut samples = [FieldSample {
            value: None,
            source_valid: false,
        }; 4];
        for (i, offset) in [[0, 0], [1, 0], [1, 1], [0, 1]].into_iter().enumerate() {
            let mut p = origin;
            p[u] += offset[0];
            p[v] += offset[1];
            samples[i] = self.sample(p).ok_or(FieldError::Bounds)?;
        }
        let negative = key.origin[axis].checked_sub(1).map(|x| {
            let mut p = key.origin;
            p[axis] = x;
            p
        });
        let cells = [
            self.neighborhood_cell(negative)?,
            self.neighborhood_cell(Some(key.origin))?,
        ];
        let mut graphs = [None, None];
        let mut factorizations = [None, None];
        for (i, cell) in cells.into_iter().enumerate() {
            check_cancel(&cancel)?;
            if let Some(field) = cell.interpolant {
                factorizations[i] = Some(field.factorization(&cancel)?);
                graphs[i] = Some(field.critical_graph(&cancel)?);
            }
        }
        let mut keys = [EdgeKey {
            origin: key.origin,
            axis: u as u8,
        }; 4];
        keys[1].axis = v as u8;
        keys[1].origin[u] = keys[1].origin[u].checked_add(1).ok_or(FieldError::Bounds)?;
        keys[2].origin[v] = keys[2].origin[v].checked_add(1).ok_or(FieldError::Bounds)?;
        keys[3].axis = v as u8;
        let [a, b, c, d] = keys;
        let edges = [
            self.edge_partition(a, &cancel)?,
            self.edge_partition(b, &cancel)?,
            self.edge_partition(c, &cancel)?,
            self.edge_partition(d, &cancel)?,
        ];
        let contour = if samples.iter().all(|s| s.source_valid && s.value.is_some()) {
            let mut values = [0; 4];
            for (value, sample) in values.iter_mut().zip(samples) {
                *value = sample.value.ok_or(FieldError::Arithmetic)?;
            }
            let mut points =
                [[coordinate(Rational::new(0, 1)?)?; 2]; FaceArrangement::MAX_INSERTIONS];
            let mut count = 0;
            for (index, graph) in graphs.iter().enumerate() {
                let Some(graph) = graph else {
                    continue;
                };
                for (_, node) in graph.nodes() {
                    check_cancel(&cancel)?;
                    if node.boundary() & (1 << (2 * axis + usize::from(index == 0))) == 0 {
                        continue;
                    }
                    *points.get_mut(count).ok_or(FieldError::Capacity)? =
                        [node.coordinates()[u], node.coordinates()[v]];
                    count += 1;
                }
            }
            let zero = coordinate(Rational::new(0, 1)?)?;
            let one = coordinate(Rational::new(1, 1)?)?;
            for (edge, partition) in edges.iter().enumerate() {
                for point in partition.points() {
                    check_cancel(&cancel)?;
                    let t = coordinate(point.position)?;
                    *points.get_mut(count).ok_or(FieldError::Capacity)? = match edge {
                        0 => [t, zero],
                        1 => [one, t],
                        2 => [t, one],
                        _ => [zero, t],
                    };
                    count += 1;
                }
            }
            Some(Bilinear::new(values)?.arrangement(&points[..count], &cancel)?)
        } else {
            None
        };
        check_cancel(&cancel)?;
        Ok(SourceFace {
            field: self.clone(),
            key,
            samples,
            cells,
            graphs,
            factorizations,
            edges,
            contour,
        })
    }
}
