// SPDX-License-Identifier: GPL-3.0-or-later
//! Erase only explicitly selected boundary cells. Geometry is not visibility authority.
use super::{
    BrushAction, BrushShape, LabelTile, Labels, MAX_STROKE_CANDIDATES, MAX_STROKE_POINTS,
    PhysicalBrush,
    physical::geometry::{Footprint, Segment},
};
use crate::{
    mask::{MaskKey, MaskRevision, MaskView},
    resources::Reservation,
    surface::{CellFace, FaceBoundary},
};
use std::sync::Arc;

#[cfg(test)]
mod tests;

pub const MAX_ERASE_FACES: usize = 65536;

/// Borrowed geometric selection for an exact manual ID/revision. These faces
/// alone do not prove visibility. A view tool must retain and revalidate its
/// original completed-image witness, including optics, camera and clipping.
pub struct FaceEraseSelection<'a> {
    pub key: &'a MaskKey,
    pub revision: &'a MaskRevision,
    pub faces: &'a [CellFace],
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FaceEraseSummary {
    /// Input descriptors, including multiple sides/pixels for the same cell.
    pub candidate_faces: usize,
    /// Admitted face/segment tests, including duplicates and early-exit cases.
    pub candidate_tests: usize,
    pub changed_voxels: usize,
    pub changed_tiles: usize,
    /// Source-cell minimum and exclusive maximum; absent for an unchanged proposal.
    pub changed_bounds: Option<[[usize; 3]; 2]>,
}
/// Private immutable COW preview. Only original selection cells can change;
/// removing a front cell never discovers another layer within this proposal.
pub struct FaceErase {
    base: Arc<Labels>,
    result: Arc<Labels>,
    settings: PhysicalBrush,
    points_mm: Vec<[f64; 3]>,
    faces: Vec<CellFace>,
    summary: FaceEraseSummary,
    _metadata: Reservation,
}
impl FaceErase {
    pub fn base(&self) -> &Arc<Labels> {
        &self.base
    }
    pub fn preview(&self) -> &Arc<Labels> {
        &self.result
    }
    pub fn settings(&self) -> &PhysicalBrush {
        &self.settings
    }
    pub fn points_mm(&self) -> &[[f64; 3]] {
        &self.points_mm
    }
    pub fn faces(&self) -> &[CellFace] {
        &self.faces
    }
    pub fn summary(&self) -> FaceEraseSummary {
        self.summary
    }
    pub fn changed_tiles(&self) -> impl Iterator<Item = LabelTile> + '_ {
        self.result
            .changed_tiles()
            .filter(|_| self.summary.changed_voxels > 0)
    }
    pub fn is_current(
        &self,
        current: &Arc<Labels>,
        settings: &PhysicalBrush,
        points_mm: &[[f64; 3]],
    ) -> bool {
        Arc::ptr_eq(current, &self.base)
            && settings == &self.settings
            && points_mm == self.points_mm
    }
    /// Caller must also revalidate the originating view witness and its edit
    /// session/input epoch. Geometry and a matching map do not authorize Apply.
    pub fn apply(
        self,
        current: &Arc<Labels>,
        settings: &PhysicalBrush,
        points_mm: &[[f64; 3]],
        cancelled: impl Fn() -> bool,
    ) -> Result<Arc<Labels>, &'static str> {
        if cancelled() {
            return Err("Cancelled");
        }
        if !self.is_current(current, settings, points_mm) {
            return Err("The face erase no longer matches the current map, path or edit settings");
        }
        Ok(self.result)
    }
}
fn current_face(view: MaskView<'_>, face: &CellFace) -> bool {
    let cell = face.cell().map(|v| v as usize);
    if !view.sample(cell).is_some_and(|s| s.eligible()) {
        return false;
    }
    let side = face.side();
    let axis = side.axis();
    let dimensions = view.source().dimensions();
    let grid = if side.positive() {
        cell[axis] + 1 == dimensions[axis]
    } else {
        cell[axis] == 0
    };
    if grid {
        return face.boundary() == FaceBoundary::Grid;
    }
    let mut neighbor = cell;
    if side.positive() {
        neighbor[axis] += 1;
    } else {
        neighbor[axis] -= 1;
    }
    face.boundary() == FaceBoundary::Mask
        && view
            .sample(neighbor)
            .is_some_and(|s| s.known && s.source_valid && !s.present)
}
impl Labels {
    /// Worker-only one-layer erase of an explicit geometric face selection.
    /// Full affine source-cell centres must also meet the physical sweep/clip.
    /// Unknown, invalid, competing or stale descriptors refuse the whole result.
    /// The caller supplies a separate complete-image visibility proof; a surface
    /// extractor's output alone must not be used as that proof by a view tool.
    pub fn propose_face_erase(
        self: &Arc<Self>,
        settings: &PhysicalBrush,
        points_mm: &[[f64; 3]],
        selection: FaceEraseSelection<'_>,
        cancelled: impl Fn() -> bool,
    ) -> Result<FaceErase, &'static str> {
        if cancelled() {
            return Err("Cancelled");
        }
        Self::check_dimensions(self.source.dimensions())?;
        if settings.label == 0 || settings.protected_ids[usize::from(settings.label)] {
            return Err("Choose an unprotected, nonzero label ID");
        }
        if settings.action != BrushAction::Erase || settings.shape != BrushShape::Sphere {
            return Err("Face erase requires an erase action and a spherical physical footprint");
        }
        if points_mm.is_empty() || points_mm.len() > MAX_STROKE_POINTS {
            return Err("Face erase needs between one and 8192 path points");
        }
        if selection.faces.len() > MAX_ERASE_FACES {
            return Err("Face erase is limited to 65536 face descriptors per proposal");
        }
        let view = MaskView::manual(self, settings.label)?;
        if selection.key != &view.key() || selection.revision != view.revision() {
            return Err("The face selection belongs to another mask ID or revision");
        }
        let count = (points_mm.len() - 1).max(1);
        let candidate_tests = selection
            .faces
            .len()
            .checked_mul(count)
            .filter(|v| *v <= MAX_STROKE_CANDIDATES)
            .ok_or("Face erase is too large; use a shorter path or fewer faces")?;
        let footprint = Footprint::new(&self.source, settings)?;
        let budget = self.values.budget();
        let _scratch = budget
            .reserve((count * std::mem::size_of::<Segment>()) as u64)
            .ok_or("Not enough memory for face erase bounds")?;
        let mut segments = Vec::new();
        segments
            .try_reserve_exact(count)
            .map_err(|_| "Not enough memory for face erase bounds")?;
        for i in 0..count {
            if cancelled() {
                return Err("Cancelled");
            }
            let end = if points_mm.len() == 1 { 0 } else { i + 1 };
            segments.push(footprint.segment(&self.source, points_mm[i], points_mm[end])?);
        }
        for (i, face) in selection.faces.iter().enumerate() {
            if i.is_multiple_of(64) && cancelled() {
                return Err("Cancelled");
            }
            if !current_face(view, face) {
                return Err("The face selection contains a stale or unknown boundary cell");
            }
        }
        let metadata = budget
            .reserve(
                (std::mem::size_of::<FaceErase>()
                    + std::mem::size_of_val(points_mm)
                    + std::mem::size_of_val(selection.faces)) as u64,
            )
            .ok_or("Not enough memory for the face erase proposal")?;
        let mut points = Vec::new();
        points
            .try_reserve_exact(points_mm.len())
            .map_err(|_| "Not enough memory for the face erase path")?;
        points.extend_from_slice(points_mm);
        let mut faces = Vec::new();
        faces
            .try_reserve_exact(selection.faces.len())
            .map_err(|_| "Not enough memory for the face erase selection")?;
        faces.extend_from_slice(selection.faces);
        let mut result = self.try_clone(&cancelled)?;
        result.begin_edit();
        let [width, height, _] = self.source.dimensions();
        let mut summary = FaceEraseSummary {
            candidate_faces: faces.len(),
            candidate_tests,
            ..Default::default()
        };
        for (i, face) in faces.iter().enumerate() {
            if i.is_multiple_of(64) && cancelled() {
                return Err("Cancelled");
            }
            let cell = face.cell().map(|v| v as usize);
            if result.value(cell) != Some(settings.label) {
                continue;
            }
            let p = self.source.voxel_vector_to_world(cell.map(|v| v as f64));
            let mut inside = false;
            for (s, segment) in segments.iter().enumerate() {
                if s.is_multiple_of(64) && cancelled() {
                    return Err("Cancelled");
                }
                if footprint.contains(segment, p) {
                    inside = true;
                    break;
                }
            }
            if !inside {
                continue;
            }
            let [x, y, z] = cell;
            result.set_value((z * height + y) * width + x, 0)?;
            summary.changed_voxels += 1;
            let bounds = summary
                .changed_bounds
                .get_or_insert([cell, cell.map(|v| v + 1)]);
            for (a, v) in cell.into_iter().enumerate() {
                bounds[0][a] = bounds[0][a].min(v);
                bounds[1][a] = bounds[1][a].max(v + 1);
            }
        }
        if cancelled() {
            return Err("Cancelled");
        }
        summary.changed_tiles = result.changed_tiles().count();
        Ok(FaceErase {
            base: self.clone(),
            result: if summary.changed_voxels == 0 {
                self.clone()
            } else {
                Arc::new(result)
            },
            settings: settings.clone(),
            points_mm: points,
            faces,
            summary,
            _metadata: metadata,
        })
    }
}
