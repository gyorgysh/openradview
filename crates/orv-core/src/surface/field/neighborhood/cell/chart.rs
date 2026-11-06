// SPDX-License-Identifier: GPL-3.0-or-later
//! Source-owned projection arrangements and exact rational zero fibers.
use super::*;
mod edges;
pub use edges::{ChartEdgeStep, ProjectedCellEdges, ProjectedEdge, ProjectedEdgeKind};

/// Intersection of a source-cell zero set with a closed projection fiber.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CellFiber {
    Empty,
    /// A unique height; `normal` is the sign of the field derivative along the
    /// projection axis, not a physical-space normal or an edit authority.
    Point {
        height: Rational,
        normal: i8,
    },
    /// Both endpoint values vanish. No particular height may be substituted.
    Whole,
}

/// One exact two-face arrangement over a source-owned finite cell graph.
/// This is a projection domain, not a triangulated surface or a claim that all
/// chart-region boundaries are conforming to neighboring cells.
pub struct CellProjectionChart {
    graph: Arc<SourceCellGraph>,
    axis: u8,
    subdivision: JointSubdivision,
    _reservation: Reservation,
}
impl CellProjectionChart {
    pub fn graph(&self) -> &Arc<SourceCellGraph> {
        &self.graph
    }
    pub fn axis(&self) -> u8 {
        self.axis
    }
    /// Ascending source lattice axes used by the two chart coordinates.
    pub fn transverse_axes(&self) -> [usize; 2] {
        transverse(usize::from(self.axis))
    }
    pub fn subdivision(&self) -> &JointSubdivision {
        &self.subdivision
    }
    pub fn referenced_bytes(&self) -> u64 {
        std::mem::size_of::<Self>() as u64
    }
    /// Classify a rational chart coordinate and lift its unique zero height.
    /// Irrational coordinates and unrepresentable ratios fail explicitly; exact
    /// signs and retained constraints remain available in the subdivision.
    pub fn fiber(&self, point: [ExactCoordinate; 2]) -> Result<CellFiber, FieldError> {
        let [u, v] = point.map(|p| p.rational_value().ok_or(FieldError::Arithmetic));
        let (u, v) = (u?, v?);
        let weights = |r: Rational| -> Result<[i128; 2], FieldError> {
            let n = i128::from(r.numerator());
            let d = i128::from(r.denominator());
            if n < 0 || n > d {
                return Err(FieldError::Bounds);
            }
            Ok([d - n, n])
        };
        let [u0, u1] = weights(u)?;
        let [v0, v1] = weights(v)?;
        let products = [(u0, v0), (u1, v0), (u1, v1), (u0, v1)];
        let mut ends = [0i128; 2];
        for (end, face) in ends.iter_mut().zip(self.subdivision.fields()) {
            for ((a, b), value) in products.into_iter().zip(face.values()) {
                let term = a
                    .checked_mul(b)
                    .and_then(|n| n.checked_mul(i128::from(value)))
                    .ok_or(FieldError::Arithmetic)?;
                *end = end.checked_add(term).ok_or(FieldError::Arithmetic)?;
            }
        }
        let [lo, hi] = ends;
        if lo == 0 && hi == 0 {
            return Ok(CellFiber::Whole);
        }
        if (lo > 0 && hi > 0) || (lo < 0 && hi < 0) {
            return Ok(CellFiber::Empty);
        }
        let difference = hi.checked_sub(lo).ok_or(FieldError::Arithmetic)?;
        let numerator = lo.checked_neg().ok_or(FieldError::Arithmetic)?;
        // Rational::new normalizes a negative denominator and takes absolute
        // values during reduction, so the i128 minimum is not representable.
        difference.checked_abs().ok_or(FieldError::Arithmetic)?;
        Ok(CellFiber::Point {
            height: Rational::new(numerator, difference)?,
            normal: difference.signum() as i8,
        })
    }
}
impl SourceCellGraph {
    /// Build an exact projection arrangement with every original cell-graph node
    /// inserted. Projected nodes may coincide; the retained graph keeps their
    /// distinct heights and ownership. Refuses a nonfinite graph or too many
    /// insertions rather than dropping constraints.
    pub fn projection_chart(
        self: &Arc<Self>,
        axis: usize,
        cancel: impl Fn() -> bool,
    ) -> Result<CellProjectionChart, SurfaceError> {
        self.projection_chart_with_budget(axis, MemoryBudget::volumes(), cancel)
    }
    /// Add exact chart constraints, for example points requested by another
    /// boundary owner. Original source nodes remain mandatory and share the same
    /// bounded insertion limit. Callers must still coordinate the full seam.
    pub fn projection_chart_with_insertions(
        self: &Arc<Self>,
        axis: usize,
        insertions: &[[ExactCoordinate; 2]],
        cancel: impl Fn() -> bool,
    ) -> Result<CellProjectionChart, SurfaceError> {
        self.projection_chart_with_insertions_and_budget(
            axis,
            insertions,
            MemoryBudget::volumes(),
            cancel,
        )
    }
    pub(in crate::surface) fn projection_chart_with_budget(
        self: &Arc<Self>,
        axis: usize,
        budget: Arc<MemoryBudget>,
        cancel: impl Fn() -> bool,
    ) -> Result<CellProjectionChart, SurfaceError> {
        self.projection_chart_with_insertions_and_budget(axis, &[], budget, cancel)
    }
    pub(in crate::surface) fn projection_chart_with_insertions_and_budget(
        self: &Arc<Self>,
        axis: usize,
        insertions: &[[ExactCoordinate; 2]],
        budget: Arc<MemoryBudget>,
        cancel: impl Fn() -> bool,
    ) -> Result<CellProjectionChart, SurfaceError> {
        check(&cancel)?;
        if axis >= 3 || self.state != CellGraphState::Finite {
            return Err(SurfaceError::Geometry);
        }
        let count = self
            .nodes
            .len()
            .checked_add(insertions.len())
            .ok_or(SurfaceError::Limit)?;
        if count > JointSubdivision::MAX_INSERTIONS {
            return Err(SurfaceError::Limit);
        }
        let metadata = std::mem::size_of::<CellProjectionChart>() as u64;
        let reservation = budget.reserve(metadata).ok_or(SurfaceError::Memory)?;
        let scratch =
            std::mem::size_of::<[[ExactCoordinate; 2]; JointSubdivision::MAX_INSERTIONS]>() as u64;
        let _scratch = budget.reserve(scratch).ok_or(SurfaceError::Memory)?;
        let zero = endpoint(false)?;
        let mut points = [[zero; 2]; JointSubdivision::MAX_INSERTIONS];
        let [u, v] = transverse(axis);
        for (slot, node) in points.iter_mut().zip(&self.nodes) {
            check(&cancel)?;
            *slot = [node.coordinates[u], node.coordinates[v]];
        }
        for (slot, point) in points[self.nodes.len()..count].iter_mut().zip(insertions) {
            check(&cancel)?;
            *slot = *point;
        }
        let field = self.cell.interpolant().ok_or(SurfaceError::Geometry)?;
        let lower = field.face(axis, false).ok_or(SurfaceError::Geometry)?;
        let upper = field.face(axis, true).ok_or(SurfaceError::Geometry)?;
        let subdivision = lower
            .joint_subdivision(upper, &points[..count], &cancel)
            .map_err(error)?;
        check(&cancel)?;
        Ok(CellProjectionChart {
            graph: Arc::clone(self),
            axis: axis as u8,
            subdivision,
            _reservation: reservation,
        })
    }
}
