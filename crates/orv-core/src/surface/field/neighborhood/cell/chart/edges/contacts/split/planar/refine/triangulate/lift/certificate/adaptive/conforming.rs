// SPDX-License-Identifier: GPL-3.0-or-later
//! Shared-edge adaptive subdivision within one source-cell chart.
use super::*;
mod seam;
pub use seam::{
    AppliedFaceCell, AppliedFaceLimits, AppliedFacePair, AppliedFaceSpan, AppliedFaceTriangle,
    AppliedFaceVertex, AppliedTriangleState, CellFaceReport, CellFaceState, CellSeamBatch,
    CellSeamLimits, CellSeamStop, FacePartitionLimits, FacePartitionPoint, FacePartitionSpan,
    FacePlacement, FacePromotionLimits, FaceSpanCoverage, SharedFacePartition,
};

const MAX_VERTICES: usize = 65_536;
const MAX_TRIANGLES: usize = 65_536;
const MAX_OPERATIONS: usize = 4_000_000;
const MAX_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConformingSurfaceLimits {
    pub tolerance_bits: u8,
    pub max_depth: u8,
    pub max_vertices: usize,
    pub max_triangles: usize,
    /// Bounds candidate processing and adjacency visits.
    pub max_operations: usize,
    /// Additional typed output; source and scratch are separately admitted.
    pub max_bytes: u64,
}
impl Default for ConformingSurfaceLimits {
    fn default() -> Self {
        Self {
            tolerance_bits: 8,
            max_depth: MAX_DEPTH,
            max_vertices: MAX_VERTICES,
            max_triangles: MAX_TRIANGLES,
            max_operations: 1_000_000,
            max_bytes: MAX_BYTES,
        }
    }
}
/// Resource limits and requested axial separation in the source's physical space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PhysicalConformingLimits {
    pub max_error_mm: f64,
    pub max_depth: u8,
    pub max_vertices: usize,
    pub max_triangles: usize,
    pub max_operations: usize,
    /// Additional typed output; source and scratch are admitted separately.
    pub max_bytes: u64,
}
#[derive(Clone, Copy, Debug, PartialEq)]
enum Criterion {
    AxialBits(u8),
    PhysicalMm(f64),
}
impl Criterion {
    fn accepts(
        self,
        certificate: SurfaceFacetCertificate,
        chart: &CellProjectionChart,
    ) -> Result<bool, SurfaceError> {
        let SurfaceFacetCertificate::Certified {
            numerator,
            denominator,
        } = certificate
        else {
            return Ok(false);
        };
        match self {
            Self::AxialBits(bits) => Ok(numerator
                .checked_mul(1u128 << bits)
                .ok_or(SurfaceError::Limit)?
                <= denominator),
            Self::PhysicalMm(mm) => {
                let basis = super::super::axis_basis(chart.graph().field().source(), chart.axis())?;
                match super::super::physical_upper_bound(certificate, basis) {
                    Ok(Some(upper)) => Ok(upper <= mm),
                    Ok(None) | Err(SurfaceError::Limit) => Ok(false),
                    Err(error) => Err(error),
                }
            }
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConformingVertex {
    Source {
        id: u32,
        position: Option<[u32; 3]>,
    },
    Midpoint {
        parents: [u32; 2],
        position: [u32; 3],
    },
    /// `point` indexes this mesh's exact face-split provenance.
    FaceSplit {
        parents: [u32; 2],
        point: u32,
        position: [u32; 3],
    },
}
impl ConformingVertex {
    pub fn position(self) -> Option<[u32; 3]> {
        match self {
            Self::Source { position, .. } => position,
            Self::Midpoint { position, .. } | Self::FaceSplit { position, .. } => Some(position),
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConformingStopReason {
    DepthLimit,
    NeighborDepthLimit,
    NeighborNormalMismatch,
    NeighborNonPositiveProjection,
    TriangleLimit,
    VertexLimit,
    WorkLimit,
    NonPositiveProjection,
    EmptyMidpoint,
    WholeMidpoint,
    OppositeMidpointNormal,
    EndFaceContour,
    FaceRecertification,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConformingFacetState {
    WithinTolerance {
        numerator: u128,
        denominator: u128,
    },
    Unresolved {
        certificate: Option<SurfaceFacetCertificate>,
        reason: ConformingStopReason,
    },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConformingTriangle {
    pub vertices: [u32; 3],
    pub source_facet: u32,
    pub depth: u8,
    pub state: ConformingFacetState,
    normal: i8,
    dirty: bool,
}
#[derive(Clone, Copy)]
struct FaceSeamCoordinate {
    face_axis: u8,
    upper: bool,
    along: ExactCoordinate,
}
/// Retains candidate facets and their source-bound adaptive and face lineage.
/// Noncandidate facets, whole-grid seams and rendering remain unresolved.
pub struct ConformingSurfaceMesh {
    input: Arc<EnclosedSurfaceGraph>,
    seam_points: Vec<FaceSeamCoordinate>,
    lineage: Option<Arc<AppliedFacePair>>,
    lineage_depth: u8,
    criterion: Criterion,
    vertices: Vec<ConformingVertex>,
    triangles: Vec<ConformingTriangle>,
    bytes: u64,
    _reservation: Reservation,
}
impl ConformingSurfaceMesh {
    pub const POSITION_DENOMINATOR: u32 = REFINED_GRID;
    pub fn input(&self) -> &Arc<EnclosedSurfaceGraph> {
        &self.input
    }
    /// The last face application, when its exchange history is retained.
    /// Compacted meshes retain exact split coordinates without that application.
    pub fn face_application(&self) -> Option<&Arc<AppliedFacePair>> {
        self.lineage.as_ref()
    }
    /// Depth of retained exchange history; zero also describes compacted meshes.
    pub fn face_lineage_depth(&self) -> u8 {
        self.lineage_depth
    }
    /// Physical target if this mesh was refined and recertified in millimetres.
    pub fn physical_tolerance_mm(&self) -> Option<f64> {
        match self.criterion {
            Criterion::PhysicalMm(mm) => Some(mm),
            Criterion::AxialBits(_) => None,
        }
    }
    /// Local source-cell target if this mesh uses a dyadic axial tolerance.
    pub fn axial_tolerance_bits(&self) -> Option<u8> {
        match self.criterion {
            Criterion::AxialBits(bits) => Some(bits),
            Criterion::PhysicalMm(_) => None,
        }
    }
    /// Recover the source face and exact transverse coordinate for a face split.
    pub fn face_split_source(&self, point: u32) -> Option<(FaceKey, ExactCoordinate)> {
        let seam = self.seam_points.get(point as usize)?;
        let chart = self.input.input().input().input().input().edges().chart();
        let face = chart
            .graph()
            .cell()
            .face(usize::from(seam.face_axis), seam.upper)?
            .0;
        Some((face, seam.along))
    }
    pub fn vertices(&self) -> &[ConformingVertex] {
        &self.vertices
    }
    pub fn triangles(&self) -> &[ConformingTriangle] {
        &self.triangles
    }
    /// Conservative physical separation for this triangle's retained axial
    /// certificate. `None` means no numerical certificate is available; an
    /// unresolved triangle can still have a bound above its requested tolerance.
    /// World-coordinate rounding, GPU precision and unrepresented surface regions
    /// are not covered.
    pub fn triangle_physical_error_upper_mm(&self, id: u32) -> Result<Option<f64>, SurfaceError> {
        let triangle = self
            .triangles
            .get(id as usize)
            .ok_or(SurfaceError::Geometry)?;
        let certificate = match triangle.state {
            ConformingFacetState::WithinTolerance {
                numerator,
                denominator,
            } => SurfaceFacetCertificate::Certified {
                numerator,
                denominator,
            },
            ConformingFacetState::Unresolved {
                certificate: Some(certificate),
                ..
            } => certificate,
            ConformingFacetState::Unresolved {
                certificate: None, ..
            } => return Ok(None),
        };
        let chart = self.input.input().input().input().input().edges().chart();
        let basis = super::super::axis_basis(chart.graph().field().source(), chart.axis())?;
        super::super::physical_upper_bound(certificate, basis)
    }
    pub fn referenced_bytes(&self) -> u64 {
        self.bytes
    }
}
fn pending(source_facet: u32, vertices: [u32; 3], depth: u8, normal: i8) -> ConformingTriangle {
    ConformingTriangle {
        vertices,
        source_facet,
        depth,
        normal,
        dirty: true,
        state: ConformingFacetState::Unresolved {
            certificate: None,
            reason: ConformingStopReason::WorkLimit,
        },
    }
}
fn positions(vertices: &[ConformingVertex], ids: [u32; 3]) -> Result<[[u32; 3]; 3], SurfaceError> {
    let [a, b, c] = ids.map(|id| vertices.get(id as usize).and_then(|v| v.position()));
    Ok([
        a.ok_or(SurfaceError::Geometry)?,
        b.ok_or(SurfaceError::Geometry)?,
        c.ok_or(SurfaceError::Geometry)?,
    ])
}
fn oriented_edge(vertices: [u32; 3], a: u32, b: u32) -> Option<(u32, u32, u32)> {
    [
        (vertices[0], vertices[1], vertices[2]),
        (vertices[1], vertices[2], vertices[0]),
        (vertices[2], vertices[0], vertices[1]),
    ]
    .into_iter()
    .find(|(x, y, _)| (*x == a && *y == b) || (*x == b && *y == a))
}
fn stop(
    triangle: &mut ConformingTriangle,
    certificate: Option<SurfaceFacetCertificate>,
    reason: ConformingStopReason,
) {
    triangle.state = ConformingFacetState::Unresolved {
        certificate,
        reason,
    };
    triangle.dirty = false;
}
fn midpoint_reason(reason: AdaptiveStopReason) -> Result<ConformingStopReason, SurfaceError> {
    match reason {
        AdaptiveStopReason::EmptyMidpoint => Ok(ConformingStopReason::EmptyMidpoint),
        AdaptiveStopReason::WholeMidpoint => Ok(ConformingStopReason::WholeMidpoint),
        AdaptiveStopReason::OppositeMidpointNormal => {
            Ok(ConformingStopReason::OppositeMidpointNormal)
        }
        AdaptiveStopReason::EndFaceContour => Ok(ConformingStopReason::EndFaceContour),
        _ => Err(SurfaceError::Geometry),
    }
}
fn split(
    triangle: ConformingTriangle,
    edge: (u32, u32, u32),
    midpoint: u32,
) -> (ConformingTriangle, ConformingTriangle) {
    let (a, b, c) = edge;
    let depth = triangle.depth + 1;
    (
        pending(
            triangle.source_facet,
            [a, midpoint, c],
            depth,
            triangle.normal,
        ),
        pending(
            triangle.source_facet,
            [midpoint, b, c],
            depth,
            triangle.normal,
        ),
    )
}

impl EnclosedSurfaceGraph {
    /// Refine all candidate facets within this cell, splitting both candidate
    /// owners of every shared edge. Unproved leaves are retained; source-cell
    /// boundaries and noncandidate facets remain explicit in the retained input.
    pub fn refine_conforming(
        self: &Arc<Self>,
        limits: ConformingSurfaceLimits,
        cancel: impl Fn() -> bool,
    ) -> Result<ConformingSurfaceMesh, SurfaceError> {
        self.refine_conforming_with_budget(limits, MemoryBudget::volumes(), cancel)
    }
    pub(in crate::surface) fn refine_conforming_with_budget(
        self: &Arc<Self>,
        limits: ConformingSurfaceLimits,
        budget: Arc<MemoryBudget>,
        cancel: impl Fn() -> bool,
    ) -> Result<ConformingSurfaceMesh, SurfaceError> {
        self.refine_conforming_impl(
            limits,
            Criterion::AxialBits(limits.tolerance_bits),
            budget,
            cancel,
        )
    }
    /// Refine candidate facets using an explicit physical separation target.
    /// Unproved facets and resource-limited leaves remain unresolved.
    pub fn refine_conforming_physical(
        self: &Arc<Self>,
        limits: PhysicalConformingLimits,
        cancel: impl Fn() -> bool,
    ) -> Result<ConformingSurfaceMesh, SurfaceError> {
        self.refine_conforming_physical_with_budget(limits, MemoryBudget::volumes(), cancel)
    }
    pub(in crate::surface) fn refine_conforming_physical_with_budget(
        self: &Arc<Self>,
        limits: PhysicalConformingLimits,
        budget: Arc<MemoryBudget>,
        cancel: impl Fn() -> bool,
    ) -> Result<ConformingSurfaceMesh, SurfaceError> {
        check(&cancel)?;
        if !limits.max_error_mm.is_finite() || limits.max_error_mm < 0. {
            return Err(SurfaceError::Geometry);
        }
        self.refine_conforming_impl(
            ConformingSurfaceLimits {
                tolerance_bits: 0,
                max_depth: limits.max_depth,
                max_vertices: limits.max_vertices,
                max_triangles: limits.max_triangles,
                max_operations: limits.max_operations,
                max_bytes: limits.max_bytes,
            },
            Criterion::PhysicalMm(limits.max_error_mm),
            budget,
            cancel,
        )
    }
    fn refine_conforming_impl(
        self: &Arc<Self>,
        limits: ConformingSurfaceLimits,
        criterion: Criterion,
        budget: Arc<MemoryBudget>,
        cancel: impl Fn() -> bool,
    ) -> Result<ConformingSurfaceMesh, SurfaceError> {
        check(&cancel)?;
        if limits.tolerance_bits > 16
            || limits.max_depth > MAX_DEPTH
            || limits.max_vertices == 0
            || limits.max_vertices > MAX_VERTICES
            || limits.max_triangles == 0
            || limits.max_triangles > MAX_TRIANGLES
            || limits.max_operations == 0
            || limits.max_operations > MAX_OPERATIONS
            || limits.max_bytes > MAX_BYTES
        {
            return Err(SurfaceError::Limit);
        }
        let source_vertices = self.vertices.len();
        if source_vertices > limits.max_vertices {
            return Err(SurfaceError::Limit);
        }
        let mut candidate_count = 0usize;
        for state in &self.facets {
            check(&cancel)?;
            candidate_count += usize::from(matches!(state, BoundedSurfaceFacet::Candidate { .. }));
        }
        if candidate_count > limits.max_triangles {
            return Err(SurfaceError::Limit);
        }
        let bytes = size(
            &[
                (limits.max_vertices, std::mem::size_of::<ConformingVertex>()),
                (
                    limits.max_triangles,
                    std::mem::size_of::<ConformingTriangle>(),
                ),
            ],
            std::mem::size_of::<ConformingSurfaceMesh>(),
        )?;
        if bytes > limits.max_bytes {
            return Err(SurfaceError::Limit);
        }
        let reservation = budget.reserve(bytes).ok_or(SurfaceError::Memory)?;
        let _scratch = budget
            .reserve(std::mem::size_of::<[ConformingTriangle; 4]>() as u64)
            .ok_or(SurfaceError::Memory)?;
        let mut vertices = exact(limits.max_vertices)?;
        let mut triangles = exact(limits.max_triangles)?;
        for id in 0..source_vertices {
            check(&cancel)?;
            let id = u32::try_from(id).map_err(|_| SurfaceError::Limit)?;
            let position = self
                .vertex_lattice_point(id)?
                .map(|p| p.map(|v| v << MAX_DEPTH));
            vertices.push(ConformingVertex::Source { id, position });
        }
        for (id, state) in self.facets.iter().enumerate() {
            check(&cancel)?;
            if let BoundedSurfaceFacet::Candidate { normal } = *state {
                let id = u32::try_from(id).map_err(|_| SurfaceError::Limit)?;
                let source = self
                    .input
                    .triangles()
                    .get(id as usize)
                    .ok_or(SurfaceError::Geometry)?;
                positions(&vertices, source.vertices)?;
                triangles.push(pending(id, source.vertices, 0, normal));
            }
        }
        let chart = self.input.input().input().input().edges().chart();
        let axis = usize::from(chart.axis());
        let [u, v] = chart.transverse_axes();
        let field = chart
            .graph()
            .cell()
            .interpolant()
            .ok_or(SurfaceError::Geometry)?;
        let mut operations = 0usize;
        let mut cursor = 0usize;
        while cursor < triangles.len() {
            check(&cancel)?;
            if !triangles[cursor].dirty {
                if operations < limits.max_operations {
                    operations += 1;
                }
                cursor += 1;
                continue;
            }
            if operations >= limits.max_operations {
                stop(
                    &mut triangles[cursor],
                    None,
                    ConformingStopReason::WorkLimit,
                );
                cursor += 1;
                continue;
            }
            operations += 1;
            let current = triangles[cursor];
            let p = positions(&vertices, current.vertices)?;
            let certificate = evaluate_grid(
                field.values(),
                axis,
                current.normal,
                p,
                REFINED_GRID,
                &cancel,
            )?;
            if let SurfaceFacetCertificate::Certified {
                numerator,
                denominator,
            } = certificate
                && criterion.accepts(certificate, chart)?
            {
                triangles[cursor].state = ConformingFacetState::WithinTolerance {
                    numerator,
                    denominator,
                };
                triangles[cursor].dirty = false;
                cursor += 1;
                continue;
            }
            if certificate == SurfaceFacetCertificate::NonPositiveProjection {
                stop(
                    &mut triangles[cursor],
                    Some(certificate),
                    ConformingStopReason::NonPositiveProjection,
                );
                cursor += 1;
                continue;
            }
            if current.depth == limits.max_depth {
                stop(
                    &mut triangles[cursor],
                    Some(certificate),
                    ConformingStopReason::DepthLimit,
                );
                cursor += 1;
                continue;
            }
            let (a, b, c) = longest_edge(p, u, v);
            let edge = (
                current.vertices[a],
                current.vertices[b],
                current.vertices[c],
            );
            let mut neighbor = None;
            let mut incomplete = false;
            for (id, other) in triangles.iter().enumerate() {
                check(&cancel)?;
                if id == cursor {
                    continue;
                }
                if operations >= limits.max_operations {
                    incomplete = true;
                    break;
                }
                operations += 1;
                if let Some(other_edge) = oriented_edge(other.vertices, edge.0, edge.1) {
                    if neighbor.is_some() || other_edge.0 != edge.1 || other_edge.1 != edge.0 {
                        return Err(SurfaceError::Geometry);
                    }
                    neighbor = Some((id, other_edge));
                }
            }
            if incomplete {
                stop(
                    &mut triangles[cursor],
                    Some(certificate),
                    ConformingStopReason::WorkLimit,
                );
                cursor += 1;
                continue;
            }
            if let Some((id, _)) = neighbor {
                let other = triangles[id];
                let reason = if other.depth == limits.max_depth {
                    Some(ConformingStopReason::NeighborDepthLimit)
                } else if other.normal != current.normal {
                    Some(ConformingStopReason::NeighborNormalMismatch)
                } else {
                    let q = positions(&vertices, other.vertices)?;
                    let area = (i128::from(q[1][u]) - i128::from(q[0][u]))
                        * (i128::from(q[2][v]) - i128::from(q[0][v]))
                        - (i128::from(q[1][v]) - i128::from(q[0][v]))
                            * (i128::from(q[2][u]) - i128::from(q[0][u]));
                    (area <= 0).then_some(ConformingStopReason::NeighborNonPositiveProjection)
                };
                if let Some(reason) = reason {
                    stop(&mut triangles[cursor], Some(certificate), reason);
                    cursor += 1;
                    continue;
                }
            }
            let added = 1 + usize::from(neighbor.is_some());
            if triangles
                .len()
                .checked_add(added)
                .ok_or(SurfaceError::Limit)?
                > limits.max_triangles
            {
                stop(
                    &mut triangles[cursor],
                    Some(certificate),
                    ConformingStopReason::TriangleLimit,
                );
                cursor += 1;
                continue;
            }
            if vertices.len() == limits.max_vertices {
                stop(
                    &mut triangles[cursor],
                    Some(certificate),
                    ConformingStopReason::VertexLimit,
                );
                cursor += 1;
                continue;
            }
            let position = match midpoint(chart, p[a], p[b], current.normal, &cancel)? {
                Ok(point) => point,
                Err(reason) => {
                    stop(
                        &mut triangles[cursor],
                        Some(certificate),
                        midpoint_reason(reason)?,
                    );
                    cursor += 1;
                    continue;
                }
            };
            let id = u32::try_from(vertices.len()).map_err(|_| SurfaceError::Limit)?;
            vertices.push(ConformingVertex::Midpoint {
                parents: [edge.0, edge.1],
                position,
            });
            let (first, second) = split(current, edge, id);
            triangles[cursor] = first;
            triangles.push(second);
            if let Some((other, other_edge)) = neighbor {
                let (first, second) = split(triangles[other], other_edge, id);
                triangles[other] = first;
                triangles.push(second);
                cursor = cursor.min(other);
            }
        }
        check(&cancel)?;
        Ok(ConformingSurfaceMesh {
            input: Arc::clone(self),
            seam_points: Vec::new(),
            lineage: None,
            lineage_depth: 0,
            criterion,
            vertices,
            triangles,
            bytes,
            _reservation: reservation,
        })
    }
}
