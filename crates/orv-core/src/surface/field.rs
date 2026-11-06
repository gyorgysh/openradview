// SPDX-License-Identifier: GPL-3.0-or-later
//! Exact mask fields and local derived geometry. Local face meshes do not establish
//! a conforming whole-grid surface, 3D connectivity or edit authority. Critical
//! strata remain explicit.
mod affine;
mod critical;
mod factor;
mod neighborhood;
mod pipeline;
use crate::{
    mask::{MaskKey, MaskOwner, MaskRevision},
    volume::Volume,
};
pub use affine::{AffineCell, AffineDisposition, AffineFace, AffineFaceState, AffineFaceTrace};
pub use critical::{
    AlgebraicRoot, AnchorLine, AnchorRay, AnchorRing, AnchorStar, BilinearIntersections,
    BilinearSlice, CommonZero, ContourNode, ContourSegment, CriticalGraph, ExactCoordinate,
    FaceArrangement, FaceInteriorLimit, GraphEdge, GraphNode, IntersectionContact,
    IntersectionPoint, JointEdge, JointRegion, JointSubdivision, LimitContours, PatchHalfEdge,
    PatchRegion, PlaneSubdivision, ProjectionAnalysis, ProjectionPoint, RefinedJointSubdivision,
    RefinedSubdivision, RefinementLimits, RegionTriangle, RegularTangents, SubdivisionEdge,
    TangentRay, TriangulatedJointSubdivision, TriangulatedSubdivision, TriangulationLimits,
    WholeSection,
};
pub use factor::{
    AxisFactor, CellFactorization, CellPhase, CellResidual, FactorPatch, ResidualKind, ZeroBoundary,
};
pub use neighborhood::{
    AdaptiveFacetLimits, AdaptiveFacetMesh, AdaptiveFacetState, AdaptiveFacetTriangle,
    AdaptiveStopReason, AppliedFaceCell, AppliedFaceLimits, AppliedFacePair, AppliedFaceSpan,
    AppliedFaceTriangle, AppliedFaceVertex, AppliedTriangleState, BoundedSurfaceFacet,
    BoundedSurfaceVertex, CellAxisBounds, CellFaceReport, CellFaceState, CellFiber,
    CellFiberBounds, CellGraphEdge, CellGraphGeometry, CellGraphHalfEdge, CellGraphLimits,
    CellGraphNode, CellGraphRegion, CellGraphState, CellNeighborhood, CellPathLimits,
    CellPathOrigin, CellPathVertex, CellProjectionChart, CellSeamBatch, CellSeamLimits,
    CellSeamStop, ChartEdgeStep, ConformingFacetState, ConformingStopReason,
    ConformingSurfaceLimits, ConformingSurfaceMesh, ConformingTriangle, ConformingVertex,
    EdgePartition, EdgePoint, EnclosedSurfaceGraph, FacePartitionLimits, FacePartitionPoint,
    FacePartitionSpan, FacePlacement, FacePromotionLimits, FaceSpanCoverage, NeighborhoodCell,
    PhysicalConformingLimits, PlanarEdge, PlanarHalfEdge, PlanarRefinementLimits, PlanarRegion,
    PlanarTriangle, PlanarTriangulationLimits, ProjectedCellEdges, ProjectedContact,
    ProjectedContactGeometry, ProjectedContactOwner, ProjectedContacts, ProjectedEdge,
    ProjectedEdgeKind, ProjectedPlanarGraph, RefinedCellGraph, RefinedPlanarGraph,
    SharedFacePartition, SourceCellGraph, SourceFace, SplitPath, SplitProjectedConstraints,
    SplitSegment, SplitStep, SurfaceEnclosureLimits, SurfaceFacetCertificate,
    TriangulatedPlanarGraph, WholeZeroFace, WholeZeroSubdivision, ZeroFaceMesh, ZeroFacePatch,
    ZeroFacePhase, ZeroFaceSample,
};
pub use pipeline::{
    CandidateAssembly, CandidateAssemblyLimits, CandidateCell, CandidateCellState, CandidateGrid,
    CandidateGridLimits, CandidateGridUpdate, GridCoordination, PipelineStage,
};
use std::sync::Arc;

#[cfg(test)]
thread_local! {
    static FIELD_READS: std::cell::RefCell<Option<Vec<[usize; 3]>>> = const { std::cell::RefCell::new(None) };
}
#[cfg(test)]
pub(in crate::surface) fn capture_field_reads<T>(f: impl FnOnce() -> T) -> (T, Vec<[usize; 3]>) {
    FIELD_READS.with(|reads| *reads.borrow_mut() = Some(Vec::new()));
    let result = f();
    let reads = FIELD_READS.with(|reads| reads.borrow_mut().take().unwrap_or_default());
    (result, reads)
}

/// Proof for one independent candidate, before any neighbor-specific face split.
/// SourceFace reads its two incident cells and four EdgePartitions; those read
/// all incident cell corners and half-face samples. For a cell with lower corner
/// L every such sample lies in the clipped inclusive box [L-1, L+2].
pub(in crate::surface) struct CellReuseProof<'a> {
    old: &'a MaskField,
    new: &'a MaskField,
    origin: [usize; 3],
}
impl<'a> CellReuseProof<'a> {
    fn compare(
        old: &'a MaskField,
        new: &'a MaskField,
        origin: [usize; 3],
        cancel: &impl Fn() -> bool,
    ) -> Result<Option<Self>, crate::surface::SurfaceError> {
        use crate::surface::SurfaceError;
        if cancel() {
            return Err(SurfaceError::Cancelled);
        }
        if old.key() != new.key() || !Arc::ptr_eq(old.source(), new.source()) {
            return Ok(None);
        }
        old.cell(origin).map_err(|_| SurfaceError::Geometry)?;
        let dimensions = old.source().dimensions();
        let start = origin.map(|v| v.saturating_sub(1));
        let end =
            std::array::from_fn::<_, 3, _>(|i| origin[i].saturating_add(2).min(dimensions[i] - 1));
        for z in start[2]..=end[2] {
            for y in start[1]..=end[1] {
                for x in start[0]..=end[0] {
                    if cancel() {
                        return Err(SurfaceError::Cancelled);
                    }
                    if old.sample([x, y, z]) != new.sample([x, y, z]) {
                        return Ok(None);
                    }
                }
            }
        }
        Ok(Some(Self { old, new, origin }))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldError {
    Selection,
    Bounds,
    Value,
    Contour,
    Capacity,
    Arithmetic,
    Cancelled,
}
impl std::fmt::Display for FieldError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Selection => "Choose an existing nonzero mask ID",
            Self::Bounds => "Interpolation cell is outside the source-centre lattice",
            Self::Value => "Mask field values must be between -255 and 255",
            Self::Contour => "Contour inputs do not agree with the face field",
            Self::Capacity => "Face contour point limit exceeded",
            Self::Arithmetic => "Mask field arithmetic cannot be represented",
            Self::Cancelled => "Cancelled",
        })
    }
}
impl std::error::Error for FieldError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FieldSample {
    /// Binary -1/+1, or original fractional byte * 2 - declared maximum.
    /// None retains unknown supplied coverage, independently of source validity.
    pub value: Option<i16>,
    pub source_valid: bool,
}

/// An immutable source-bound field with no dense copy or additional sample cache.
#[derive(Clone)]
pub struct MaskField {
    owner: MaskOwner,
    source: Arc<Volume>,
    key: MaskKey,
    revision: MaskRevision,
}
impl MaskField {
    pub fn new(owner: MaskOwner, number: u16) -> Result<Self, FieldError> {
        let view = owner.view(number).map_err(|_| FieldError::Selection)?;
        let key = view.key();
        let revision = view.revision().clone();
        Ok(Self {
            source: owner.source(),
            owner,
            key,
            revision,
        })
    }
    pub fn owner(&self) -> &MaskOwner {
        &self.owner
    }
    pub fn source(&self) -> &Arc<Volume> {
        &self.source
    }
    pub fn key(&self) -> &MaskKey {
        &self.key
    }
    pub fn revision(&self) -> &MaskRevision {
        &self.revision
    }
    /// None means outside the finite source, never an exterior zero sample.
    pub fn sample(&self, voxel: [usize; 3]) -> Option<FieldSample> {
        #[cfg(test)]
        FIELD_READS.with(|reads| {
            if let Some(reads) = reads.borrow_mut().as_mut() {
                reads.push(voxel);
            }
        });
        let view = self.owner.view(self.key.number).ok()?;
        let membership = view.sample(voxel)?;
        let value = match &self.owner {
            MaskOwner::Manual(_) => {
                membership
                    .known
                    .then_some(if membership.present { 1 } else { -1 })
            }
            MaskOwner::Supplied(map) => {
                let mask = map.mask(self.key.number)?;
                mask.raw_value(voxel)
                    .map(|v| 2 * i16::from(v) - i16::from(mask.encoding().maximum()))
            }
        };
        Some(FieldSample {
            value,
            source_valid: membership.source_valid,
        })
    }
    pub fn cell(&self, lower: [usize; 3]) -> Result<FieldCell, FieldError> {
        let dimensions = self.source.dimensions();
        if (0..3).any(|a| lower[a].checked_add(1).is_none_or(|p| p >= dimensions[a])) {
            return Err(FieldError::Bounds);
        }
        let mut corners = [FieldSample {
            value: None,
            source_valid: false,
        }; 8];
        let mut origin = [0; 3];
        for a in 0..3 {
            origin[a] = u32::try_from(lower[a]).map_err(|_| FieldError::Bounds)?;
        }
        for (index, sample) in corners.iter_mut().enumerate() {
            *sample = self
                .sample(std::array::from_fn(|a| lower[a] + ((index >> a) & 1)))
                .ok_or(FieldError::Bounds)?;
        }
        Ok(FieldCell { origin, corners })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FieldCell {
    origin: [u32; 3],
    corners: [FieldSample; 8],
}
impl FieldCell {
    pub fn origin(&self) -> [u32; 3] {
        self.origin
    }
    /// Corner bit order is x + 2*y + 4*z.
    pub fn corners(&self) -> &[FieldSample; 8] {
        &self.corners
    }
    pub fn interpolant(&self) -> Option<Trilinear> {
        let mut values = [0; 8];
        for (value, sample) in values.iter_mut().zip(self.corners) {
            if !sample.source_valid {
                return None;
            }
            *value = sample.value?;
        }
        Trilinear::new(values).ok()
    }
    pub fn face(&self, axis: usize, upper: bool) -> Option<(FaceKey, Bilinear)> {
        let face = self.interpolant()?.face(axis, upper)?;
        let mut origin = self.origin;
        if upper {
            origin[axis] = origin[axis].checked_add(1)?;
        }
        Some((
            FaceKey {
                origin,
                axis: axis as u8,
            },
            face,
        ))
    }
    /// `corner` is the lower endpoint's cube bit index; its axis bit must be zero.
    pub fn edge(&self, axis: usize, corner: u8) -> Option<(EdgeKey, EdgeZero)> {
        let zero = self.interpolant()?.edge(axis, corner)?;
        let mut origin = self.origin;
        for (a, coordinate) in origin.iter_mut().enumerate() {
            *coordinate = coordinate.checked_add(u32::from((corner >> a) & 1))?;
        }
        Some((
            EdgeKey {
                origin,
                axis: axis as u8,
            },
            zero,
        ))
    }
}

/// Keys use original lattice coordinates and ascending axes on either owner.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FaceKey {
    pub origin: [u32; 3],
    pub axis: u8,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EdgeKey {
    pub origin: [u32; 3],
    pub axis: u8,
}

/// Reduced exact ratio with positive denominator. Construction is bounded by
/// retained uint8 mask values; no floating comparison selects a topology.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rational {
    numerator: i64,
    denominator: u64,
}
impl Rational {
    fn new(mut numerator: i128, mut denominator: i128) -> Result<Self, FieldError> {
        if denominator == 0 {
            return Err(FieldError::Arithmetic);
        }
        if denominator < 0 {
            numerator = -numerator;
            denominator = -denominator;
        }
        let (mut a, mut b) = (numerator.abs(), denominator);
        while b != 0 {
            (a, b) = (b, a % b);
        }
        Ok(Self {
            numerator: i64::try_from(numerator / a).map_err(|_| FieldError::Arithmetic)?,
            denominator: u64::try_from(denominator / a).map_err(|_| FieldError::Arithmetic)?,
        })
    }
    pub fn numerator(self) -> i64 {
        self.numerator
    }
    pub fn denominator(self) -> u64 {
        self.denominator
    }
    /// Approximate coordinates only; use the exact numerator for a sign test.
    pub fn as_f64(self) -> f64 {
        self.numerator as f64 / self.denominator as f64
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EdgeZero {
    Empty,
    Point(Rational),
    Entire,
}
fn edge_zero(a: i16, b: i16) -> Option<EdgeZero> {
    if a == 0 && b == 0 {
        Some(EdgeZero::Entire)
    } else if (a > 0 && b > 0) || (a < 0 && b < 0) {
        Some(EdgeZero::Empty)
    } else {
        Rational::new(i128::from(a), i128::from(a) - i128::from(b))
            .ok()
            .map(EdgeZero::Point)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bilinear {
    values: [i16; 4],
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FaceTopology {
    Empty,
    /// Local cyclic edge indices: 00->10->11->01->00.
    Arc([u8; 2]),
    Split {
        arcs: [[u8; 2]; 2],
        joined_corners: [u8; 2],
    },
    /// Both contour branches meet here; no arbitrary pairing is substituted.
    Saddle {
        position: [Rational; 2],
    },
    /// Corner/edge contacts need a shared singularity policy before triangulation.
    NonRegular {
        zero_corners: u8,
    },
}
impl Bilinear {
    pub fn new(values: [i16; 4]) -> Result<Self, FieldError> {
        check_values(&values)?;
        Ok(Self { values })
    }
    pub fn values(self) -> [i16; 4] {
        self.values
    }
    pub fn topology(self) -> Result<FaceTopology, FieldError> {
        let mut zeros = 0;
        let mut crossing = [0; 4];
        let mut count = 0;
        for (i, a) in self.values.into_iter().enumerate() {
            zeros |= u8::from(a == 0) << i;
            if (a > 0) != (self.values[(i + 1) % 4] > 0) {
                crossing[count] = i as u8;
                count += 1;
            }
        }
        if zeros != 0 {
            return Ok(FaceTopology::NonRegular {
                zero_corners: zeros,
            });
        }
        match count {
            0 => Ok(FaceTopology::Empty),
            2 => Ok(FaceTopology::Arc([crossing[0], crossing[1]])),
            4 => {
                let [a, b, c, d] = self.values.map(i64::from);
                match (a * c - b * d).cmp(&0) {
                    std::cmp::Ordering::Greater => Ok(FaceTopology::Split {
                        arcs: [[0, 1], [2, 3]],
                        joined_corners: [0, 2],
                    }),
                    std::cmp::Ordering::Less => Ok(FaceTopology::Split {
                        arcs: [[0, 3], [1, 2]],
                        joined_corners: [1, 3],
                    }),
                    std::cmp::Ordering::Equal => Ok(FaceTopology::Saddle {
                        position: [
                            Rational::new(i128::from(a - d), i128::from(a - b + c - d))?,
                            Rational::new(i128::from(a - b), i128::from(a - b + c - d))?,
                        ],
                    }),
                }
            }
            _ => Err(FieldError::Arithmetic),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Trilinear {
    values: [i16; 8],
}
fn check_values(values: &[i16]) -> Result<(), FieldError> {
    if values.iter().all(|v| (-255..=255).contains(v)) {
        Ok(())
    } else {
        Err(FieldError::Value)
    }
}
fn face_indices(axis: usize, upper: bool) -> Option<[usize; 4]> {
    if axis >= 3 {
        return None;
    }
    let u = usize::from(axis == 0);
    let v = if axis == 2 { 1 } else { 2 };
    let origin = usize::from(upper) << axis;
    Some([
        origin,
        origin | (1 << u),
        origin | (1 << u) | (1 << v),
        origin | (1 << v),
    ])
}
impl Trilinear {
    pub fn new(values: [i16; 8]) -> Result<Self, FieldError> {
        check_values(&values)?;
        Ok(Self { values })
    }
    pub fn values(self) -> [i16; 8] {
        self.values
    }
    pub fn face(self, axis: usize, upper: bool) -> Option<Bilinear> {
        Some(Bilinear {
            values: face_indices(axis, upper)?.map(|i| self.values[i]),
        })
    }
    pub fn edge(self, axis: usize, corner: u8) -> Option<EdgeZero> {
        if axis >= 3 || corner >= 8 || corner & (1 << axis) != 0 {
            return None;
        }
        edge_zero(
            self.values[usize::from(corner)],
            self.values[usize::from(corner) | (1 << axis)],
        )
    }
    /// Bernstein interpolation in the closed unit cell, without source-affine or
    /// physical-normal conversion. This floating value cannot choose connectivity.
    pub fn evaluate(self, point: [f64; 3]) -> Option<f64> {
        if !point.into_iter().all(|v| (0. ..=1.).contains(&v)) {
            return None;
        }
        Some(
            self.values
                .into_iter()
                .enumerate()
                .map(|(i, value)| {
                    f64::from(value)
                        * (0..3)
                            .map(|a| {
                                if i & (1 << a) == 0 {
                                    1. - point[a]
                                } else {
                                    point[a]
                                }
                            })
                            .product::<f64>()
                })
                .sum(),
        )
    }
    /// Determinant A(t)*C(t)-B(t)*D(t) of sections normal to `axis`.
    pub fn section_determinant(self, axis: usize) -> Option<SectionDeterminant> {
        let lo = self.face(axis, false)?.values.map(i64::from);
        let hi = self.face(axis, true)?.values.map(i64::from);
        let delta: [i64; 4] = std::array::from_fn(|i| hi[i] - lo[i]);
        Some(SectionDeterminant {
            quadratic: delta[0] * delta[2] - delta[1] * delta[3],
            linear: lo[0] * delta[2] + delta[0] * lo[2] - lo[1] * delta[3] - delta[1] * lo[3],
            constant: lo[0] * lo[2] - lo[1] * lo[3],
        })
    }
    /// A determinant extremum is only an interior-test candidate. The tested
    /// diagonal, phase, saddle location and face components still need checking.
    pub fn section_extremum(self, axis: usize) -> Result<Option<SectionExtremum>, FieldError> {
        let determinant = self.section_determinant(axis).ok_or(FieldError::Bounds)?;
        if determinant.quadratic == 0 {
            return Ok(None);
        }
        let t = Rational::new(
            -i128::from(determinant.linear),
            2 * i128::from(determinant.quadratic),
        )?;
        if t.numerator <= 0 || t.numerator as u64 >= t.denominator {
            return Ok(None);
        }
        let lo = self.face(axis, false).ok_or(FieldError::Bounds)?.values;
        let hi = self.face(axis, true).ok_or(FieldError::Bounds)?.values;
        let n = i128::from(t.numerator);
        let d = i128::from(t.denominator);
        let mut corners = [t; 4];
        let scaled: [i128; 4] =
            std::array::from_fn(|i| i128::from(lo[i]) * (d - n) + i128::from(hi[i]) * n);
        for (corner, value) in corners.iter_mut().zip(scaled) {
            *corner = Rational::new(value, d)?;
        }
        Ok(Some(SectionExtremum {
            coordinate: t,
            corners,
            determinant: Rational::new(scaled[0] * scaled[2] - scaled[1] * scaled[3], d * d)?,
            minimum: determinant.quadratic > 0,
        }))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SectionDeterminant {
    pub quadratic: i64,
    pub linear: i64,
    pub constant: i64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SectionExtremum {
    pub coordinate: Rational,
    pub corners: [Rational; 4],
    pub determinant: Rational,
    pub minimum: bool,
}
