// SPDX-License-Identifier: GPL-3.0-or-later
//! Explicit source-space swept brushes. No surface visibility or mutation authority.
use super::{LabelTile, Labels};
use crate::{resources::Reservation, volume_view::PhysicalClip};
use std::sync::Arc;

mod geometry;
use geometry::{Footprint, Segment};
#[cfg(test)]
mod tests;

pub const MAX_STROKE_POINTS: usize = 8192;
pub const MAX_STROKE_CANDIDATES: usize = 64 * 1024 * 1024;
pub const MAX_PHYSICAL_BRUSH_RADIUS_MM: f64 = 256.;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrushAction {
    /// Add only to valid background or the selected ID; competing IDs are preserved.
    Add,
    /// Remove only valid cells of the selected ID.
    Erase,
}
#[derive(Clone, Debug, PartialEq)]
pub enum BrushShape {
    /// A continuous union of physical spheres along the input polyline.
    Sphere,
    /// Project the input path onto this fixed plane, then sweep a disk through
    /// an explicit, symmetric physical thickness. This is not a source slice index.
    Plane {
        origin_mm: [f64; 3],
        normal: [f64; 3],
        thickness_mm: f64,
    },
}
#[derive(Clone, Debug, PartialEq)]
pub struct PhysicalBrush {
    pub label: u8,
    pub action: BrushAction,
    pub radius_mm: f64,
    pub shape: BrushShape,
    /// Preserves cells whose source centres are outside these physical bounds.
    pub clip: Option<PhysicalClip>,
    /// Existing IDs that must not change, including background zero. Revalidate on apply.
    pub protected_ids: [bool; 256],
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StrokeSummary {
    /// Distinct changed cells, even when segment footprints overlap.
    pub changed_voxels: usize,
    pub changed_tiles: usize,
    /// Bounding-box cell visits, including duplicates. Bounded before computation.
    pub candidate_visits: usize,
    /// Source-cell minimum and exclusive maximum; absent for an unchanged proposal.
    pub changed_bounds: Option<[[usize; 3]; 2]>,
}
/// Immutable local preview over an exact base map, with an already computed
/// private COW revision. Cancel/error cannot publish any partial edits.
/// This volumetric/plane tool can reach hidden cells; it is not a visible-surface brush.
pub struct PhysicalStroke {
    base: Arc<Labels>,
    result: Arc<Labels>,
    settings: PhysicalBrush,
    points_mm: Vec<[f64; 3]>,
    summary: StrokeSummary,
    _metadata: Reservation,
}
impl PhysicalStroke {
    pub fn base(&self) -> &Arc<Labels> {
        &self.base
    }
    pub fn settings(&self) -> &PhysicalBrush {
        &self.settings
    }
    pub fn points_mm(&self) -> &[[f64; 3]] {
        &self.points_mm
    }
    pub fn summary(&self) -> StrokeSummary {
        self.summary
    }
    /// Read-only prepared map for local preview. This does not apply the stroke;
    /// renderers must retain the proposal's identity and their own preview scope.
    pub fn preview(&self) -> &Arc<Labels> {
        &self.result
    }
    pub fn value(&self, voxel: [usize; 3]) -> Option<u8> {
        self.result.value(voxel)
    }
    pub fn changed_tiles(&self) -> impl Iterator<Item = LabelTile> + '_ {
        self.result
            .changed_tiles()
            .filter(|_| self.summary.changed_voxels > 0)
    }
    /// Numeric geometry or equal revision values cannot replace this exact base.
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
    /// Publish only after the caller also revalidates its source/session/tool
    /// scope and input epoch. Consumes once; unchanged work returns the original Arc.
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
            return Err(
                "The physical stroke no longer matches the current map, path or edit settings",
            );
        }
        Ok(if self.summary.changed_voxels == 0 {
            self.base
        } else {
            self.result
        })
    }
}
impl Labels {
    /// Worker-only continuous physical stroke over source-cell centres. Uses the
    /// full fitted affine, separate source validity and immutable label tiles.
    /// Supplied masks have no mutation path. No UI/pick/sharing authority is granted.
    pub fn propose_physical_stroke(
        self: &Arc<Self>,
        settings: &PhysicalBrush,
        points_mm: &[[f64; 3]],
        cancelled: impl Fn() -> bool,
    ) -> Result<PhysicalStroke, &'static str> {
        if cancelled() {
            return Err("Cancelled");
        }
        let [width, height, depth] = self.source.dimensions();
        Self::check_dimensions([width, height, depth])?;
        if settings.label == 0 || settings.protected_ids[usize::from(settings.label)] {
            return Err("Choose an unprotected, nonzero label ID");
        }
        if points_mm.is_empty() || points_mm.len() > MAX_STROKE_POINTS {
            return Err("Physical strokes need between one and 8192 points");
        }
        let footprint = Footprint::new(&self.source, settings)?;
        let budget = self.values.budget();
        let count = (points_mm.len() - 1).max(1);
        let _scratch = budget
            .reserve((count * std::mem::size_of::<Segment>()) as u64)
            .ok_or("Not enough memory for physical stroke bounds")?;
        let mut segments = Vec::new();
        segments
            .try_reserve_exact(count)
            .map_err(|_| "Not enough memory for physical stroke bounds")?;
        let mut summary = StrokeSummary::default();
        for i in 0..count {
            if cancelled() {
                return Err("Cancelled");
            }
            let end = if points_mm.len() == 1 { 0 } else { i + 1 };
            let segment = footprint.segment(&self.source, points_mm[i], points_mm[end])?;
            let [lo, hi] = segment.bounds;
            let visits = (0..3).try_fold(1usize, |n, a| n.checked_mul(hi[a] - lo[a]));
            summary.candidate_visits = visits
                .and_then(|n| summary.candidate_visits.checked_add(n))
                .filter(|n| *n <= MAX_STROKE_CANDIDATES)
                .ok_or("Physical stroke is too large; use shorter strokes or a smaller radius")?;
            segments.push(segment);
        }
        let metadata = budget
            .reserve(
                (std::mem::size_of::<PhysicalStroke>() + std::mem::size_of_val(points_mm)) as u64,
            )
            .ok_or("Not enough memory for the physical stroke proposal")?;
        let mut points = Vec::new();
        points
            .try_reserve_exact(points_mm.len())
            .map_err(|_| "Not enough memory for the physical stroke path")?;
        points.extend_from_slice(points_mm);
        let mut result = self.try_clone(&cancelled)?;
        result.begin_edit();
        let value = match settings.action {
            BrushAction::Add => settings.label,
            BrushAction::Erase => 0,
        };
        for segment in &segments {
            let [lo, hi] = segment.bounds;
            for z in lo[2]..hi[2] {
                let frame = &self.source.frames()[z];
                for y in lo[1]..hi[1] {
                    if cancelled() {
                        return Err("Cancelled");
                    }
                    for x in lo[0]..hi[0] {
                        if (x - lo[0]).is_multiple_of(256) && cancelled() {
                            return Err("Cancelled");
                        }
                        let index = (z * height + y) * width + x;
                        let old = *result
                            .values
                            .get(index)
                            .ok_or("Label tile is inconsistent")?;
                        if old == value
                            || settings.protected_ids[usize::from(old)]
                            || (old != 0 && old != settings.label)
                            || !frame.has_sample(y * width + x)
                        {
                            continue;
                        }
                        let p = self
                            .source
                            .voxel_vector_to_world([x as f64, y as f64, z as f64]);
                        if !footprint.contains(segment, p) {
                            continue;
                        }
                        result.set_value(index, value)?;
                        summary.changed_voxels += 1;
                        let bounds = summary
                            .changed_bounds
                            .get_or_insert([[x, y, z], [x + 1, y + 1, z + 1]]);
                        for (a, v) in [x, y, z].into_iter().enumerate() {
                            bounds[0][a] = bounds[0][a].min(v);
                            bounds[1][a] = bounds[1][a].max(v + 1);
                        }
                    }
                }
            }
        }
        if cancelled() {
            return Err("Cancelled");
        }
        summary.changed_tiles = result.changed_tiles().count();
        let result = if summary.changed_voxels == 0 {
            self.clone()
        } else {
            Arc::new(result)
        };
        Ok(PhysicalStroke {
            base: self.clone(),
            result,
            settings: settings.clone(),
            points_mm: points,
            summary,
            _metadata: metadata,
        })
    }
}
