// SPDX-License-Identifier: GPL-3.0-or-later
//! Region-bound triangles over one completed coordinated refinement.
use super::super::super::subdivision::refine::triangulate::{
    Input, triangulate as run_triangulation,
};
use super::*;

pub struct TriangulatedJointSubdivision {
    input: Arc<RefinedJointSubdivision>,
    triangles: Vec<RegionTriangle>,
    bytes: u64,
    _reservation: Reservation,
}
impl TriangulatedJointSubdivision {
    pub fn subdivision(&self) -> &Arc<RefinedJointSubdivision> {
        &self.input
    }
    pub fn triangles(&self) -> &[RegionTriangle] {
        &self.triangles
    }
    /// Additional retained typed storage; the shared refinement keeps its charge.
    pub fn referenced_bytes(&self) -> u64 {
        self.bytes
    }
}
impl Input for RefinedJointSubdivision {
    fn vertices(&self) -> &[[ExactCoordinate; 2]] {
        &self.vertices
    }
    fn original_count(&self) -> usize {
        self.base.nodes().count()
    }
    fn region_count(&self) -> usize {
        self.base.regions().count()
    }
    fn region(&self, id: u8) -> Option<(u8, u16)> {
        self.base
            .regions
            .get(usize::from(id))?
            .as_ref()
            .map(|r| (r.first, r.edge_count))
    }
    fn half_edge(&self, id: u8) -> Option<&PatchHalfEdge> {
        self.base.half_edge(id)
    }
    fn edge(&self, id: u8) -> Option<[u8; 2]> {
        self.base.edge(id).map(|e| [e.from, e.to])
    }
    fn edge_vertices(&self, id: u8) -> Option<&[u32]> {
        self.edge_vertices(id)
    }
    fn isolated_region(&self, id: u8) -> Result<u8, SurfaceError> {
        self.base.interior_region(id).ok_or(SurfaceError::Geometry)
    }
}
impl RefinedJointSubdivision {
    /// Triangulate each preserved region. Every original isolated point is bound
    /// to its exact containing region; the completed input remains the owner.
    pub fn triangulate(
        self: &Arc<Self>,
        limits: TriangulationLimits,
        cancel: impl Fn() -> bool,
    ) -> Result<TriangulatedJointSubdivision, SurfaceError> {
        self.triangulate_with_budget(limits, MemoryBudget::volumes(), cancel)
    }
    fn triangulate_with_budget(
        self: &Arc<Self>,
        limits: TriangulationLimits,
        budget: Arc<MemoryBudget>,
        cancel: impl Fn() -> bool,
    ) -> Result<TriangulatedJointSubdivision, SurfaceError> {
        let completed = run_triangulation::<JOINT_NODES>(
            self.as_ref(),
            std::mem::size_of::<TriangulatedJointSubdivision>(),
            limits,
            budget,
            cancel,
        )?;
        Ok(TriangulatedJointSubdivision {
            input: Arc::clone(self),
            triangles: completed.triangles,
            bytes: completed.bytes,
            _reservation: completed.reservation,
        })
    }
}

#[cfg(test)]
mod tests;
