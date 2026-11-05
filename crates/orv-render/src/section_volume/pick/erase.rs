// SPDX-License-Identifier: GPL-3.0-or-later
//! Explicit manual correction proposals from a frozen set of checked view samples.
use super::*;
use orv_core::labels::{
    FaceErase, FaceEraseSelection, FaceEraseSummary, Labels, MAX_ERASE_FACES, PhysicalBrush,
};

/// Counts original-raster outcomes, including duplicate regions. Non-hit pixels
/// contribute no editable cell; this operation never infers across their gaps.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SectionEraseVisibility {
    pub hit: usize,
    pub empty: usize,
    pub obscured: usize,
    pub ambiguous: usize,
}
/// A worker-only private manual-label preview. Unlike read-only probe outcomes,
/// this operation explicitly requests an erase, and retains the original scene
/// witness until Apply. A native caller must also recheck its edit/input epoch.
pub struct SectionFaceErase {
    frame: SectionPickFrame,
    proposal: FaceErase,
    visibility: SectionEraseVisibility,
    _metadata: Reservation,
}
impl SectionFaceErase {
    /// Convert only Hit faces in these explicit sparse sample regions. All
    /// batches must belong to the same completed image/scope; at most 65,536
    /// samples, including duplicates, are admitted before allocation. This is
    /// not a screen gesture/path builder: callers must define their physical
    /// path and must not interpolate input across covered or ambiguous centres.
    pub fn prepare(
        current: &SectionPickState,
        batches: &[SectionProbeBatch],
        settings: &PhysicalBrush,
        points_mm: &[[f64; 3]],
        cancelled: impl Fn() -> bool,
    ) -> Result<Self, &'static str> {
        let obsolete = || cancelled() || !current.scope.is_active();
        if obsolete() {
            return Err("Cancelled");
        }
        if batches.is_empty() || batches.len() > MAX_ERASE_FACES {
            return Err("Choose a bounded set of completed section probe batches");
        }
        let frame = &batches[0].frame;
        if !current.matches(frame) {
            return Err("The completed sections are no longer the current view");
        }
        let MaskOwner::Manual(base) = &current.map else {
            return Err("Supplied masks are read-only");
        };
        if settings.clip != current.scene.volume.clip {
            return Err("The erase clip must match the completed section view");
        }
        let mut count = 0usize;
        for batch in batches {
            if obsolete() {
                return Err("Cancelled");
            }
            if !Arc::ptr_eq(&frame.image, &batch.frame.image)
                || !Arc::ptr_eq(&frame.scope, &batch.frame.scope)
                || !batch.is_current(current, batch.region())
            {
                return Err("The erase samples belong to different completed section views");
            }
            count = count
                .checked_add(batch.samples().len())
                .filter(|n| *n <= MAX_ERASE_FACES)
                .ok_or("Section erase is limited to 65536 original-raster samples")?;
        }
        let budget = MemoryBudget::volumes();
        let _scratch = budget
            .reserve((count * std::mem::size_of::<orv_core::surface::CellFace>()) as u64)
            .ok_or("Not enough memory for the section erase selection")?;
        let metadata = budget
            .reserve(std::mem::size_of::<Self>() as u64)
            .ok_or("Not enough memory for the section erase witness")?;
        let mut faces = Vec::new();
        faces
            .try_reserve_exact(count)
            .map_err(|_| "Not enough memory for the section erase selection")?;
        let mut visibility = SectionEraseVisibility::default();
        for sample in batches.iter().flat_map(SectionProbeBatch::samples) {
            if obsolete() {
                return Err("Cancelled");
            }
            match sample.kind() {
                SurfaceProbeKind::Hit => {
                    let hit = sample.hit().ok_or("Missing checked section erase face")?;
                    faces.push(hit.face);
                    visibility.hit += 1;
                }
                SurfaceProbeKind::Empty => visibility.empty += 1,
                SurfaceProbeKind::Obscured => visibility.obscured += 1,
                SurfaceProbeKind::Ambiguous => visibility.ambiguous += 1,
            }
        }
        let view = current.map.view(current.number)?;
        let proposal = base.propose_face_erase(
            settings,
            points_mm,
            FaceEraseSelection {
                key: &view.key(),
                revision: view.revision(),
                faces: &faces,
            },
            obsolete,
        )?;
        if obsolete() || !current.matches(frame) {
            return Err("Cancelled");
        }
        Ok(Self {
            frame: frame.clone(),
            proposal,
            visibility,
            _metadata: metadata,
        })
    }
    pub fn proposal(&self) -> &FaceErase {
        &self.proposal
    }
    pub fn visibility(&self) -> SectionEraseVisibility {
        self.visibility
    }
    pub fn summary(&self) -> FaceEraseSummary {
        self.proposal.summary()
    }
    pub fn is_current(
        &self,
        current: &SectionPickState,
        settings: &PhysicalBrush,
        points_mm: &[[f64; 3]],
    ) -> bool {
        let MaskOwner::Manual(base) = &current.map else {
            return false;
        };
        current.matches(&self.frame) && self.proposal.is_current(base, settings, points_mm)
    }
    /// The caller must recheck its live CPU request and edit/input epoch before
    /// publication. A stale/covered view or preview image cannot be substituted
    /// for this originating witness. Nothing here grants export/disclosure access.
    pub fn apply(
        self,
        current: &SectionPickState,
        settings: &PhysicalBrush,
        points_mm: &[[f64; 3]],
        cancelled: impl Fn() -> bool,
    ) -> Result<Arc<Labels>, &'static str> {
        let obsolete = || cancelled() || !current.scope.is_active();
        if obsolete() {
            return Err("Cancelled");
        }
        if !self.is_current(current, settings, points_mm) {
            return Err("The section erase no longer matches the current view or edit settings");
        }
        let MaskOwner::Manual(base) = &current.map else {
            return Err("Supplied masks are read-only");
        };
        let result = self.proposal.apply(base, settings, points_mm, obsolete)?;
        if obsolete() || !current.matches(&self.frame) {
            return Err("Cancelled");
        }
        Ok(result)
    }
}
