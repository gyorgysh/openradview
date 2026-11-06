// SPDX-License-Identifier: GPL-3.0-or-later
//! Final native owner gates for explicitly applied visible-layer corrections.
use super::*;
use orv_core::{labels::PhysicalBrush, volume::Volume};
use orv_render::section_volume::pick::{SectionFaceErase, SectionPickState};
#[cfg(all(test, feature = "gpu-tests"))]
mod tests;

impl LabelUi {
    pub(crate) fn can_start_surface_erase(&self, selected: usize, source: &Arc<Volume>) -> bool {
        !self.busy()
            && !source.is_color()
            && !self.supplied_active
            && self.series == selected
            && self.label != 0
            && self.display.visible_ids[usize::from(self.label)]
            && self
                .current
                .as_ref()
                .is_some_and(|map| Arc::ptr_eq(&map.source_arc(), source))
    }
    pub(crate) fn begin_surface_erase(&mut self) {
        self.stop_painting();
        self.open = false;
    }
    pub(crate) fn surface_erase_base(
        &self,
        selected: usize,
        source: &Arc<Volume>,
    ) -> Option<(Arc<Labels>, u8)> {
        (self.can_start_surface_erase(selected, source)
            && !self.open
            && !self.paint
            && !self.physical.open
            && !self.regions.picking
            && self.regions.preview.is_none()
            && self.regions.seeded.preview.is_none())
        .then(|| self.current.as_ref().map(|map| (map.clone(), self.label)))
        .flatten()
    }
    /// Called at the actual UI publication point, after the handler rechecks its
    /// ordered input epoch and latest CPU view. The original render scope stays
    /// live through this final exact-owner gate and prepared Apply.
    pub(crate) fn apply_surface_erase(
        &mut self,
        proposal: SectionFaceErase,
        current: &SectionPickState,
        settings: &PhysicalBrush,
        selected: usize,
        source: &Arc<Volume>,
    ) -> Result<(), String> {
        let Some((base, number)) = self.surface_erase_base(selected, source) else {
            return Err("The manual label editing context changed; request a new preview".into());
        };
        if !Arc::ptr_eq(&base, proposal.proposal().base())
            || settings.label != number
            || current.number != u16::from(number)
        {
            return Err("The erase preview no longer belongs to the current manual mask".into());
        }
        let [point] = proposal.proposal().points_mm() else {
            return Err("This control applies one visible-layer erase stamp at a time".into());
        };
        let point = *point;
        let result = proposal
            .apply(current, settings, &[point], || false)
            .map_err(str::to_owned)?;
        if !current.scope.is_active()
            || !Arc::ptr_eq(&result.source_arc(), source)
            || result.object_id() != base.object_id()
            || (!Arc::ptr_eq(&result, &base) && result.parent_revision() != Some(base.revision()))
        {
            return Err("The erase view or prepared label revision changed before Apply".into());
        }
        self.commit_change(result);
        Ok(())
    }
}
