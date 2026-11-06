// SPDX-License-Identifier: GPL-3.0-or-later
//! Source-owned zero-face regions and oriented lattice-space surface patches.
use super::*;
use crate::{
    resources::{MemoryBudget, Reservation},
    surface::SurfaceError,
};

/// A derived triangle, without visibility or source-edit authority. Interface
/// winding points along its signed lattice axis; a sheet uses the positive-axis
/// front and requires two-sided rendering. Physical/GPU conversion is separate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ZeroFacePatch {
    /// Index in the complete region mesh, including nondrawn interior regions.
    pub triangle: u32,
    pub vertices: [u32; 3],
    pub region: u8,
    /// Only Interface or Sheet occurs in the completed patch iterator.
    pub phase: ZeroFacePhase,
}
/// Complete source-owned region geometry with separately classified drawable
/// patches. Empty interior output is distinct from an unresolved face.
pub struct ZeroFaceMesh {
    field: MaskField,
    key: FaceKey,
    cells: [NeighborhoodCell; 2],
    geometry: TriangulatedJointSubdivision,
    phases: [Option<ZeroFacePhase>; JointSubdivision::MAX_REGIONS],
    patch_count: usize,
    bytes: u64,
    _reservation: Reservation,
}
impl ZeroFaceMesh {
    pub fn field(&self) -> &MaskField {
        &self.field
    }
    pub fn key(&self) -> FaceKey {
        self.key
    }
    pub fn cells(&self) -> &[NeighborhoodCell; 2] {
        &self.cells
    }
    /// Complete constraint geometry and all regions, including Interior.
    pub fn geometry(&self) -> &TriangulatedJointSubdivision {
        &self.geometry
    }
    pub fn region_phase(&self, region: u8) -> Option<ZeroFacePhase> {
        self.phases.get(usize::from(region)).copied().flatten()
    }
    pub fn patch_count(&self) -> usize {
        self.patch_count
    }
    pub fn patches(&self) -> impl Iterator<Item = ZeroFacePatch> + '_ {
        self.geometry
            .triangles()
            .iter()
            .enumerate()
            .filter_map(|(i, triangle)| {
                let phase = self.region_phase(triangle.region)?;
                let normal = match phase {
                    ZeroFacePhase::Interface { normal } => normal,
                    ZeroFacePhase::Sheet => 1,
                    _ => return None,
                };
                let mut vertices = triangle.vertices;
                // Ascending transverse axes have negative cross product only for
                // global axis1. This orientation is still in the source lattice.
                if (self.key.axis == 1) != (normal < 0) {
                    vertices.swap(1, 2);
                }
                Some(ZeroFacePatch {
                    triangle: i as u32,
                    vertices,
                    region: triangle.region,
                    phase,
                })
            })
    }
    /// Complete retained typed geometry and provenance metadata. Source/mask
    /// buffers, allocator overhead and temporary preparation are excluded.
    pub fn referenced_bytes(&self) -> u64 {
        self.bytes
    }
}
fn check(cancel: &impl Fn() -> bool) -> Result<(), SurfaceError> {
    if cancel() {
        Err(SurfaceError::Cancelled)
    } else {
        Ok(())
    }
}
fn field_error(error: FieldError) -> SurfaceError {
    match error {
        FieldError::Cancelled => SurfaceError::Cancelled,
        FieldError::Capacity => SurfaceError::Limit,
        FieldError::Selection => SurfaceError::Selection,
        _ => SurfaceError::Geometry,
    }
}
fn metadata_bytes() -> Result<u64, SurfaceError> {
    // The nested mesh header is already charged by its own reservation.
    u64::try_from(
        std::mem::size_of::<ZeroFaceMesh>()
            .checked_sub(std::mem::size_of::<TriangulatedJointSubdivision>())
            .ok_or(SurfaceError::Geometry)?,
    )
    .map_err(|_| SurfaceError::Limit)
}
fn preparation_bytes() -> u64 {
    (std::mem::size_of::<WholeZeroSubdivision>()
        + std::mem::size_of::<[[ExactCoordinate; 2]; JointSubdivision::MAX_INSERTIONS]>())
        as u64
}
impl SourceFace {
    /// Worker-side mesh of a proved whole-zero face with both incident cells
    /// known. None preserves unresolved/outside/nonzero cases without a cap.
    /// Patches omit Interior and retain original source/map/revision identity.
    pub fn zero_mesh(
        &self,
        refinement: RefinementLimits,
        triangulation: TriangulationLimits,
        cancel: impl Fn() -> bool,
    ) -> Result<Option<ZeroFaceMesh>, SurfaceError> {
        self.zero_mesh_with_budget(refinement, triangulation, MemoryBudget::volumes(), cancel)
    }
    pub(in crate::surface) fn zero_mesh_with_budget(
        &self,
        refinement: RefinementLimits,
        triangulation: TriangulationLimits,
        budget: Arc<MemoryBudget>,
        cancel: impl Fn() -> bool,
    ) -> Result<Option<ZeroFaceMesh>, SurfaceError> {
        check(&cancel)?;
        if !self
            .contour
            .as_ref()
            .is_some_and(FaceArrangement::is_whole_zero)
            || self
                .cells
                .iter()
                .any(|c| c.state() != CellNeighborhood::Known)
        {
            return Ok(None);
        }
        let metadata = metadata_bytes()?;
        let reservation = budget.reserve(metadata).ok_or(SurfaceError::Memory)?;
        let preparation = budget
            .reserve(preparation_bytes())
            .ok_or(SurfaceError::Memory)?;
        let (field, key, cells, refined) = {
            let Some(prepared) = self.zero_subdivision(&cancel).map_err(field_error)? else {
                return Ok(None);
            };
            let WholeZeroSubdivision { face, subdivision } = prepared;
            let refined =
                Arc::new(subdivision.refine_with_budget(refinement, budget.clone(), &cancel)?);
            let WholeZeroFace {
                field, key, cells, ..
            } = face;
            (field, key, cells, refined)
        };
        drop(preparation);
        let geometry = refined.triangulate_with_budget(triangulation, budget, &cancel)?;
        let mut phases = [None; JointSubdivision::MAX_REGIONS];
        for (id, region) in refined.subdivision().regions() {
            check(&cancel)?;
            *phases
                .get_mut(usize::from(id))
                .ok_or(SurfaceError::Geometry)? = Some(area_phase(region.signs));
        }
        if geometry.triangles().len() > u32::MAX as usize {
            return Err(SurfaceError::Limit);
        }
        let mut patch_count = 0usize;
        for triangle in geometry.triangles() {
            check(&cancel)?;
            match phases.get(usize::from(triangle.region)).copied().flatten() {
                Some(ZeroFacePhase::Interior) => {}
                Some(ZeroFacePhase::Interface { normal: -1 | 1 } | ZeroFacePhase::Sheet) => {
                    patch_count += 1
                }
                _ => return Err(SurfaceError::Geometry),
            }
        }
        let bytes = metadata
            .checked_add(refined.referenced_bytes())
            .and_then(|b| b.checked_add(geometry.referenced_bytes()))
            .ok_or(SurfaceError::Limit)?;
        check(&cancel)?;
        Ok(Some(ZeroFaceMesh {
            field,
            key,
            cells,
            geometry,
            phases,
            patch_count,
            bytes,
            _reservation: reservation,
        }))
    }
}
