// SPDX-License-Identifier: GPL-3.0-or-later
//! Local native owners; these carry no export or sharing authorization.
use super::*;
use orv_core::{
    mask::{MaskKey, MaskRevision},
    segments::Segments,
    volume::Volume,
};

pub(crate) use orv_core::mask::MaskOwner as RegionMap;
#[derive(Clone)]
pub(crate) struct RegionSelection {
    pub map: RegionMap,
    pub number: u16,
    pub key: MaskKey,
    pub revision: MaskRevision,
}
impl RegionSelection {
    pub fn new(map: RegionMap, number: u16) -> Result<Self, &'static str> {
        let view = map.view(number)?;
        let key = view.key();
        let revision = view.revision().clone();
        Ok(Self {
            map,
            number,
            key,
            revision,
        })
    }
    pub fn same(&self, other: &Self) -> bool {
        self.key == other.key
            && self.revision == other.revision
            && Arc::ptr_eq(&self.map.source(), &other.map.source())
    }
    pub fn same_data(&self, other: &Self) -> bool {
        self.key.object == other.key.object
            && self.revision == other.revision
            && Arc::ptr_eq(&self.map.source(), &other.map.source())
            && (matches!(
                (&self.map, &other.map),
                (RegionMap::Manual(_), RegionMap::Manual(_))
            ) || self.number == other.number)
    }
}
pub(crate) struct RegionMaps {
    pub manual: Result<Option<Arc<Labels>>, &'static str>,
    pub supplied: Result<Option<Arc<Segments>>, &'static str>,
}
impl LabelUi {
    pub(crate) fn is_physical_preview_region(&self, selection: &RegionSelection) -> bool {
        let Some(preview) = self
            .current
            .as_ref()
            .and_then(|map| self.physical.displayed(map))
        else {
            return false;
        };
        selection.number == u16::from(self.label)
            && matches!(&selection.map, RegionMap::Manual(map) if Arc::ptr_eq(map, preview))
    }
    pub(crate) fn region_maps(&self, selected: usize, source: &Arc<Volume>) -> RegionMaps {
        let manual = if self.series != selected {
            Ok(None)
        } else if self.task.is_some() {
            Err("Wait for label work to finish")
        } else {
            Ok(self
                .current
                .as_ref()
                .filter(|v| Arc::ptr_eq(&v.source_arc(), source))
                .map(|map| self.physical.displayed(map).unwrap_or(map).clone()))
        };
        RegionMaps {
            manual,
            supplied: self.supplied.local_volume_map(selected, source),
        }
    }
    pub(crate) fn manual_region_number(&self) -> u8 {
        self.label
    }
    pub(crate) fn set_manual_region_number(&mut self, number: u8) {
        self.label = number.max(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_maps_require_the_selected_series_and_exact_source_owner_and_wait_for_edits() {
        let current = super::super::tests::labels();
        let source = current.source_arc();
        let mut ui = LabelUi {
            series: 4,
            current: Some(current.clone()),
            ..Default::default()
        };
        assert!(ui.region_maps(3, &source).manual.unwrap().is_none());
        let copy = Arc::new(Volume::new(source.frames().to_vec()).unwrap());
        assert!(ui.region_maps(4, &copy).manual.unwrap().is_none());
        assert!(Arc::ptr_eq(
            &ui.region_maps(4, &source).manual.unwrap().unwrap(),
            &current
        ));
        ui.task = Some(Task::start(
            Operation::Threshold(current, None, [0., 100.], 1),
            eframe::egui::Context::default(),
        ));
        assert!(ui.region_maps(4, &source).manual.is_err());
    }

    #[test]
    fn native_selection_distinguishes_edits_and_ids_but_manual_uploads_can_share_ids() {
        let original = super::super::tests::labels();
        let first = RegionSelection::new(RegionMap::Manual(original.clone()), 1).unwrap();
        let second = RegionSelection::new(RegionMap::Manual(original.clone()), 2).unwrap();
        assert!(!first.same(&second));
        assert!(first.same_data(&second));
        assert!(RegionSelection::new(RegionMap::Manual(original.clone()), 0).is_err());
        assert!(RegionSelection::new(RegionMap::Manual(original.clone()), 256).is_err());
        let mut edited = original.try_clone(|| false).unwrap();
        edited.paint(1, &[[4., 4.]], 1, 1, || false).unwrap();
        let edit = RegionSelection::new(RegionMap::Manual(Arc::new(edited)), 1).unwrap();
        assert!(!first.same_data(&edit));
        let undo = RegionSelection::new(RegionMap::Manual(original), 1).unwrap();
        assert!(first.same(&undo));
    }
}
