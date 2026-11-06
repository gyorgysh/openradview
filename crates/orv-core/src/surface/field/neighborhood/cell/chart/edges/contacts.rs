// SPDX-License-Identifier: GPL-3.0-or-later
//! Exact contacts between projected constraints and end-face zero sets.
use super::*;
mod split;
pub use split::{
    BoundedSurfaceFacet, BoundedSurfaceVertex, CellAxisBounds, EnclosedSurfaceGraph, PlanarEdge,
    PlanarHalfEdge, PlanarRefinementLimits, PlanarRegion, PlanarTriangle,
    PlanarTriangulationLimits, ProjectedPlanarGraph, RefinedPlanarGraph, SplitPath,
    SplitProjectedConstraints, SplitSegment, SplitStep, SurfaceEnclosureLimits,
    SurfaceFacetCertificate, TriangulatedPlanarGraph,
};

const MAX_CONTACTS: usize = 262_144;
const MAX_BYTES: u64 = 32 * 1024 * 1024;

/// Coincident intervals retain both endpoints, rather than a representative.
#[derive(Clone, Copy, Debug)]
pub enum ProjectedContactGeometry {
    Point([ExactCoordinate; 2]),
    Overlap([[ExactCoordinate; 2]; 2]),
}
/// Source graph edge IDs remain distinct even at coincident projections.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProjectedContactOwner {
    Edges { first: u16, second: u16 },
    EndFace { edge: u16, upper: bool },
}
#[derive(Clone, Copy, Debug)]
pub struct ProjectedContact {
    pub owner: ProjectedContactOwner,
    pub geometry: ProjectedContactGeometry,
}
/// An exact contact directory, not a planar subdivision or surface mesh.
pub struct ProjectedContacts {
    edges: Arc<ProjectedCellEdges>,
    contacts: Vec<ProjectedContact>,
    bytes: u64,
    _reservation: Reservation,
}
impl ProjectedContacts {
    pub fn edges(&self) -> &Arc<ProjectedCellEdges> {
        &self.edges
    }
    pub fn contacts(&self) -> &[ProjectedContact] {
        &self.contacts
    }
    pub fn referenced_bytes(&self) -> u64 {
        self.bytes
    }
}
#[derive(Clone, Copy)]
struct Span {
    points: [[ExactCoordinate; 2]; 2],
    /// None is a collapsed point. Otherwise the varying chart axis.
    axis: Option<usize>,
}
fn cmp(a: ExactCoordinate, b: ExactCoordinate) -> Result<Ordering, SurfaceError> {
    a.compare(b).map_err(error)
}
impl Span {
    fn new(mut points: [[ExactCoordinate; 2]; 2]) -> Result<Self, SurfaceError> {
        let x = cmp(points[0][0], points[1][0])?;
        let y = cmp(points[0][1], points[1][1])?;
        let (axis, order) = match (x, y) {
            (Ordering::Equal, Ordering::Equal) => (None, Ordering::Equal),
            (x, Ordering::Equal) => (Some(0), x),
            (Ordering::Equal, y) => (Some(1), y),
            _ => return Err(SurfaceError::Geometry),
        };
        if order == Ordering::Greater {
            points.swap(0, 1);
        }
        Ok(Self { points, axis })
    }
    fn contains(self, point: [ExactCoordinate; 2]) -> Result<bool, SurfaceError> {
        for (axis, coordinate) in point.into_iter().enumerate() {
            if cmp(self.points[0][axis], coordinate)? == Ordering::Greater
                || cmp(coordinate, self.points[1][axis])? == Ordering::Greater
            {
                return Ok(false);
            }
        }
        Ok(true)
    }
}
fn intersection(a: Span, b: Span) -> Result<Option<ProjectedContactGeometry>, SurfaceError> {
    match (a.axis, b.axis) {
        (None, _) => Ok(b
            .contains(a.points[0])?
            .then_some(ProjectedContactGeometry::Point(a.points[0]))),
        (_, None) => Ok(a
            .contains(b.points[0])?
            .then_some(ProjectedContactGeometry::Point(b.points[0]))),
        (Some(axis), Some(other)) if axis != other => {
            let mut point = a.points[0];
            point[axis] = b.points[0][axis];
            Ok((a.contains(point)? && b.contains(point)?)
                .then_some(ProjectedContactGeometry::Point(point)))
        }
        (Some(axis), Some(_)) => {
            if cmp(a.points[0][1 - axis], b.points[0][1 - axis])? != Ordering::Equal {
                return Ok(None);
            }
            let start = if cmp(a.points[0][axis], b.points[0][axis])? == Ordering::Less {
                b.points[0]
            } else {
                a.points[0]
            };
            let end = if cmp(a.points[1][axis], b.points[1][axis])? == Ordering::Greater {
                b.points[1]
            } else {
                a.points[1]
            };
            Ok(match cmp(start[axis], end[axis])? {
                Ordering::Less => Some(ProjectedContactGeometry::Overlap([start, end])),
                Ordering::Equal => Some(ProjectedContactGeometry::Point(start)),
                Ordering::Greater => None,
            })
        }
    }
}
fn source_span(edges: &ProjectedCellEdges, id: usize) -> Result<Option<Span>, SurfaceError> {
    let entry = edges.edges.get(id).ok_or(SurfaceError::Geometry)?;
    if matches!(entry.kind, ProjectedEdgeKind::EndFace { .. }) {
        return Ok(None);
    }
    let chart = &edges.chart;
    let span = Span::new([
        projected_node(chart, entry.source_nodes[0])?,
        projected_node(chart, entry.source_nodes[1])?,
    ])?;
    if matches!(
        (entry.kind, span.axis),
        (ProjectedEdgeKind::Collapsed, None) | (ProjectedEdgeKind::Segment, Some(_))
    ) {
        Ok(Some(span))
    } else {
        Err(SurfaceError::Geometry)
    }
}
fn face_contact(
    span: Span,
    field: Bilinear,
) -> Result<Option<ProjectedContactGeometry>, SurfaceError> {
    let Some(axis) = span.axis else {
        return Ok((field.sign_at(span.points[0]).map_err(error)? == 0)
            .then_some(ProjectedContactGeometry::Point(span.points[0])));
    };
    let fixed = 1 - axis;
    match field
        .zero_slice(fixed, span.points[0][fixed])
        .map_err(error)?
    {
        BilinearSlice::Empty => Ok(None),
        BilinearSlice::Whole => Ok(Some(ProjectedContactGeometry::Overlap(span.points))),
        BilinearSlice::Point(coordinate) => {
            let mut point = span.points[0];
            point[axis] = coordinate;
            Ok(span
                .contains(point)?
                .then_some(ProjectedContactGeometry::Point(point)))
        }
    }
}
fn visit(
    edges: &ProjectedCellEdges,
    cancel: &impl Fn() -> bool,
    mut emit: impl FnMut(ProjectedContact) -> Result<(), SurfaceError>,
) -> Result<(), SurfaceError> {
    let count = edges.edges.len();
    for first in 0..count {
        check(cancel)?;
        let Some(a) = source_span(edges, first)? else {
            continue;
        };
        for second in first + 1..count {
            check(cancel)?;
            let Some(b) = source_span(edges, second)? else {
                continue;
            };
            if let Some(geometry) = intersection(a, b)? {
                emit(ProjectedContact {
                    owner: ProjectedContactOwner::Edges {
                        first: first as u16,
                        second: second as u16,
                    },
                    geometry,
                })?;
            }
        }
        for (side, field) in edges.chart.subdivision.fields().into_iter().enumerate() {
            check(cancel)?;
            if let Some(geometry) = face_contact(a, field)? {
                emit(ProjectedContact {
                    owner: ProjectedContactOwner::EndFace {
                        edge: first as u16,
                        upper: side != 0,
                    },
                    geometry,
                })?;
            }
        }
    }
    check(cancel)
}
impl ProjectedCellEdges {
    /// Worker-side exact contact enumeration. Distinct owners and overlapping
    /// intervals remain separate. Unsupported arithmetic, limit or cancellation
    /// returns no partial directory; no input is mutated.
    pub fn contacts(
        self: &Arc<Self>,
        cancel: impl Fn() -> bool,
    ) -> Result<ProjectedContacts, SurfaceError> {
        self.contacts_with_budget(MemoryBudget::volumes(), cancel)
    }
    pub(in crate::surface) fn contacts_with_budget(
        self: &Arc<Self>,
        budget: Arc<MemoryBudget>,
        cancel: impl Fn() -> bool,
    ) -> Result<ProjectedContacts, SurfaceError> {
        check(&cancel)?;
        if self.edges.len() > u16::MAX as usize {
            return Err(SurfaceError::Limit);
        }
        let mut count = 0usize;
        visit(self, &cancel, |_| {
            count = count.checked_add(1).ok_or(SurfaceError::Limit)?;
            if count > MAX_CONTACTS {
                return Err(SurfaceError::Limit);
            }
            Ok(())
        })?;
        let bytes = count
            .checked_mul(std::mem::size_of::<ProjectedContact>())
            .and_then(|n| n.checked_add(std::mem::size_of::<ProjectedContacts>()))
            .and_then(|n| u64::try_from(n).ok())
            .ok_or(SurfaceError::Limit)?;
        if bytes > MAX_BYTES {
            return Err(SurfaceError::Limit);
        }
        let reservation = budget.reserve(bytes).ok_or(SurfaceError::Memory)?;
        let mut contacts = exact(count)?;
        visit(self, &cancel, |contact| {
            if contacts.len() >= count {
                return Err(SurfaceError::Geometry);
            }
            contacts.push(contact);
            Ok(())
        })?;
        if contacts.len() != count {
            return Err(SurfaceError::Geometry);
        }
        check(&cancel)?;
        Ok(ProjectedContacts {
            edges: Arc::clone(self),
            contacts,
            bytes,
            _reservation: reservation,
        })
    }
}

#[cfg(test)]
mod tests;
