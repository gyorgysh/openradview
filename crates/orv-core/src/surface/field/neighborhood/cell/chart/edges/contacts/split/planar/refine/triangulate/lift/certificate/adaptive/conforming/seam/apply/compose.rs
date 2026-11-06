// SPDX-License-Identifier: GPL-3.0-or-later
//! Promote both applied cells with exact face-point lineage for later exchanges.
use super::*;

const MAX_LINEAGE: u8 = 16;
const MAX_SEAM_POINTS: usize = 65_536;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FacePromotionLimits {
    pub max_vertices_per_cell: usize,
    pub max_triangles_per_cell: usize,
    pub max_seam_points_per_cell: usize,
    pub max_operations: usize,
    /// Additional retained output; prior lineage and scratch are separate.
    pub max_bytes: u64,
}
impl Default for FacePromotionLimits {
    fn default() -> Self {
        Self {
            max_vertices_per_cell: MAX_CELL_VERTICES,
            max_triangles_per_cell: MAX_CELL_TRIANGLES,
            max_seam_points_per_cell: MAX_SEAM_POINTS,
            max_operations: MAX_WORK,
            max_bytes: MAX_OUTPUT,
        }
    }
}
fn build<F: Fn() -> bool>(
    pair: &Arc<AppliedFacePair>,
    side: usize,
    depth: u8,
    seam_count: usize,
    bytes: u64,
    reservation: Reservation,
    work: &mut Work<'_, F>,
) -> Result<Arc<ConformingSurfaceMesh>, SurfaceError> {
    let source = &pair.input.inputs[side];
    let output = &pair.cells[side];
    let mut vertices = exact(output.vertices.len())?;
    let mut triangles = exact(output.triangles.len())?;
    let mut seam_points = exact(seam_count)?;
    for point in &source.seam_points {
        work.charge(1)?;
        seam_points.push(*point);
    }
    let face_axis = usize::from(pair.input.face.axis);
    if face_axis >= 3 {
        return Err(SurfaceError::Geometry);
    }
    for (index, vertex) in output.vertices.iter().copied().enumerate() {
        work.charge(1)?;
        match vertex {
            AppliedFaceVertex::Retained { id, position } => {
                if id as usize != index {
                    return Err(SurfaceError::Geometry);
                }
                let original = *source
                    .vertices
                    .get(id as usize)
                    .ok_or(SurfaceError::Geometry)?;
                if original.position() != position {
                    return Err(SurfaceError::Geometry);
                }
                vertices.push(original);
            }
            AppliedFaceVertex::Split {
                point,
                edge,
                position,
            } => {
                let partition_point = pair
                    .input
                    .points
                    .get(point as usize)
                    .ok_or(SurfaceError::Geometry)?;
                match partition_point.placements[side] {
                    FacePlacement::Split {
                        edge: expected,
                        position: p,
                        ..
                    } if expected == edge && p == position => {}
                    _ => return Err(SurfaceError::Geometry),
                }
                if edge.iter().any(|id| *id as usize >= source.vertices.len()) {
                    return Err(SurfaceError::Geometry);
                }
                let upper = match position[face_axis] {
                    0 => false,
                    REFINED_GRID => true,
                    _ => return Err(SurfaceError::Geometry),
                };
                let face = chart(source)
                    .graph()
                    .cell()
                    .face(face_axis, upper)
                    .ok_or(SurfaceError::Geometry)?
                    .0;
                if face != pair.input.face {
                    return Err(SurfaceError::Geometry);
                }
                let id = u32::try_from(seam_points.len()).map_err(|_| SurfaceError::Limit)?;
                seam_points.push(FaceSeamCoordinate {
                    face_axis: face_axis as u8,
                    upper,
                    along: partition_point.coordinate,
                });
                vertices.push(ConformingVertex::FaceSplit {
                    parents: edge,
                    point: id,
                    position,
                });
            }
        }
    }
    for triangle in &output.triangles {
        work.charge(1)?;
        let original = source
            .triangles
            .get(triangle.source_triangle as usize)
            .ok_or(SurfaceError::Geometry)?;
        if original.source_facet != triangle.source_facet
            || triangle
                .vertices
                .iter()
                .any(|id| *id as usize >= vertices.len())
        {
            return Err(SurfaceError::Geometry);
        }
        let state = match triangle.state {
            AppliedTriangleState::WithinTolerance {
                numerator,
                denominator,
            } => ConformingFacetState::WithinTolerance {
                numerator,
                denominator,
            },
            AppliedTriangleState::Unresolved { certificate } => ConformingFacetState::Unresolved {
                certificate: Some(certificate),
                reason: ConformingStopReason::FaceRecertification,
            },
        };
        triangles.push(ConformingTriangle {
            vertices: triangle.vertices,
            source_facet: triangle.source_facet,
            depth: original.depth,
            state,
            normal: original.normal,
            dirty: false,
        });
    }
    if seam_points.len() != seam_count {
        return Err(SurfaceError::Geometry);
    }
    check(work.cancel)?;
    Ok(Arc::new(ConformingSurfaceMesh {
        input: Arc::clone(&source.input),
        seam_points,
        lineage: (depth != 0).then(|| Arc::clone(pair)),
        lineage_depth: depth,
        vertices,
        triangles,
        bytes,
        _reservation: reservation,
    }))
}
impl AppliedFacePair {
    /// Publish both cells with exact partition lineage so they can take part in
    /// another compatible face exchange. No whole-grid conformance is implied.
    pub fn promote_cells(
        self: &Arc<Self>,
        limits: FacePromotionLimits,
        cancel: impl Fn() -> bool,
    ) -> Result<[Arc<ConformingSurfaceMesh>; 2], SurfaceError> {
        self.promote_cells_with_budget(limits, MemoryBudget::volumes(), cancel)
    }
    pub(in crate::surface) fn promote_cells_with_budget(
        self: &Arc<Self>,
        limits: FacePromotionLimits,
        budget: Arc<MemoryBudget>,
        cancel: impl Fn() -> bool,
    ) -> Result<[Arc<ConformingSurfaceMesh>; 2], SurfaceError> {
        self.promote_with_budget(limits, budget, cancel, true)
    }
    /// Publish both cells with materialized exact face coordinates, parent
    /// vertices and facet states. The prior application and its audit history
    /// are not retained by the outputs; callers can keep the pair separately.
    pub fn promote_cells_compacted(
        self: &Arc<Self>,
        limits: FacePromotionLimits,
        cancel: impl Fn() -> bool,
    ) -> Result<[Arc<ConformingSurfaceMesh>; 2], SurfaceError> {
        self.promote_cells_compacted_with_budget(limits, MemoryBudget::volumes(), cancel)
    }
    pub(in crate::surface) fn promote_cells_compacted_with_budget(
        self: &Arc<Self>,
        limits: FacePromotionLimits,
        budget: Arc<MemoryBudget>,
        cancel: impl Fn() -> bool,
    ) -> Result<[Arc<ConformingSurfaceMesh>; 2], SurfaceError> {
        self.promote_with_budget(limits, budget, cancel, false)
    }
    fn promote_with_budget(
        self: &Arc<Self>,
        limits: FacePromotionLimits,
        budget: Arc<MemoryBudget>,
        cancel: impl Fn() -> bool,
        retain_history: bool,
    ) -> Result<[Arc<ConformingSurfaceMesh>; 2], SurfaceError> {
        check(&cancel)?;
        if limits.max_vertices_per_cell > MAX_CELL_VERTICES
            || limits.max_triangles_per_cell > MAX_CELL_TRIANGLES
            || limits.max_seam_points_per_cell > MAX_SEAM_POINTS
            || limits.max_operations == 0
            || limits.max_operations > MAX_WORK
            || limits.max_bytes > MAX_OUTPUT
        {
            return Err(SurfaceError::Limit);
        }
        // Retaining the pair also retains both input histories.
        let depth = if retain_history {
            self.input.inputs[0]
                .lineage_depth
                .max(self.input.inputs[1].lineage_depth)
                .checked_add(1)
                .ok_or(SurfaceError::Limit)?
        } else {
            0
        };
        if depth > MAX_LINEAGE {
            return Err(SurfaceError::Limit);
        }
        let mut work = Work {
            used: 0,
            limit: limits.max_operations,
            cancel: &cancel,
        };
        let mut counts = [0usize; 2];
        let mut bytes = [0u64; 2];
        for side in 0..2 {
            let source = &self.input.inputs[side];
            let output = &self.cells[side];
            if output.vertices.len() > limits.max_vertices_per_cell
                || output.triangles.len() > limits.max_triangles_per_cell
            {
                return Err(SurfaceError::Limit);
            }
            let mut splits = 0usize;
            for vertex in &output.vertices {
                work.charge(1)?;
                splits += usize::from(matches!(vertex, AppliedFaceVertex::Split { .. }));
            }
            counts[side] = source
                .seam_points
                .len()
                .checked_add(splits)
                .ok_or(SurfaceError::Limit)?;
            if counts[side] > limits.max_seam_points_per_cell {
                return Err(SurfaceError::Limit);
            }
            bytes[side] = size(
                &[
                    (
                        output.vertices.len(),
                        std::mem::size_of::<ConformingVertex>(),
                    ),
                    (
                        output.triangles.len(),
                        std::mem::size_of::<ConformingTriangle>(),
                    ),
                    (counts[side], std::mem::size_of::<FaceSeamCoordinate>()),
                ],
                std::mem::size_of::<ConformingSurfaceMesh>() + 2 * std::mem::size_of::<usize>(),
            )?;
        }
        if bytes[0].checked_add(bytes[1]).ok_or(SurfaceError::Limit)? > limits.max_bytes {
            return Err(SurfaceError::Limit);
        }
        let _scratch = budget
            .reserve(std::mem::size_of::<[usize; 4]>() as u64)
            .ok_or(SurfaceError::Memory)?;
        let first = budget.reserve(bytes[0]).ok_or(SurfaceError::Memory)?;
        let second = budget.reserve(bytes[1]).ok_or(SurfaceError::Memory)?;
        let a = build(self, 0, depth, counts[0], bytes[0], first, &mut work)?;
        let b = build(self, 1, depth, counts[1], bytes[1], second, &mut work)?;
        check(&cancel)?;
        Ok([a, b])
    }
}
