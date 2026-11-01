// SPDX-License-Identifier: GPL-3.0-or-later
//! Frozen applied mask group and the same categorical pixels/mesh used natively.
use super::{
    LabelUi,
    overlay::{self, Display, Mode, PlanePixels},
};
use eframe::egui;
use orv_core::{
    ViewState,
    labels::Labels,
    resources::{MemoryBudget, Reservation},
    segments::Segments,
    study::Frame,
    volume::Volume,
};
use serde::Serialize;
use std::sync::Arc;
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum MaskGroup {
    Manual,
    Supplied,
}
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum MaskStyle {
    Fill,
    Outline,
    Hatched,
}
impl From<Mode> for MaskStyle {
    fn from(mode: Mode) -> Self {
        match mode {
            Mode::Fill => Self::Fill,
            Mode::Outline => Self::Outline,
            Mode::Hatched => Self::Hatched,
        }
    }
}
#[derive(Serialize)]
pub(crate) struct MaskDescription {
    pub group: MaskGroup,
    pub style: MaskStyle,
    pub opacity: f32,
    pub visible_ids: Vec<u16>,
}
enum Map {
    Manual(Arc<Labels>, Box<Display>),
    Supplied(Arc<Segments>, super::supplied::overlay::Display),
}
pub(crate) struct CapturedMasks {
    map: Map,
    description: MaskDescription,
    _memory: Reservation,
}
pub(crate) struct MaskPixels {
    pub image: egui::ColorImage,
    pub _cpu: Reservation,
    pub _gpu: Reservation,
}
impl CapturedMasks {
    pub fn name(&self) -> &'static str {
        match self.description.group {
            MaskGroup::Manual => "Manual labels",
            MaskGroup::Supplied => "Supplied masks",
        }
    }
    fn new(map: Map, group: MaskGroup, mode: Mode, opacity: f32) -> Result<Self, &'static str> {
        if !opacity.is_finite() || !(0.0..=1.0).contains(&opacity) {
            return Err("Mask opacity is invalid");
        }
        let memory = MemoryBudget::volumes()
            .reserve(4096)
            .ok_or("Not enough memory for illustration mask settings")?;
        let mut ids = Vec::new();
        ids.try_reserve_exact(255)
            .map_err(|_| "Not enough memory for mask visibility")?;
        match &map {
            Map::Manual(_, display) => ids.extend(
                (1..=255)
                    .filter(|id| display.visible_ids[*id as usize])
                    .map(|id| id as u16),
            ),
            Map::Supplied(map, display) => ids.extend(
                map.infos()
                    .enumerate()
                    .filter(|(index, _)| display.visible & (1u64 << index) != 0)
                    .map(|(_, info)| info.number),
            ),
        }
        Ok(Self {
            map,
            description: MaskDescription {
                group,
                style: mode.into(),
                opacity,
                visible_ids: ids,
            },
            _memory: memory,
        })
    }
    pub(in crate::labels) fn supplied(
        map: Arc<Segments>,
        display: super::supplied::overlay::Display,
        opacity: f32,
    ) -> Result<Self, &'static str> {
        let mode = display.mode;
        Self::new(
            Map::Supplied(map, display),
            MaskGroup::Supplied,
            mode,
            opacity,
        )
    }
    pub fn description(&self) -> &MaskDescription {
        &self.description
    }
    pub fn source(&self) -> Arc<Volume> {
        match &self.map {
            Map::Manual(map, _) => map.source_arc(),
            Map::Supplied(map, _) => map.source_arc(),
        }
    }
    pub fn prepare(
        &self,
        image: &Frame,
        maximum_side: u32,
        cancel: &dyn Fn() -> bool,
    ) -> Result<MaskPixels, String> {
        if image.width > maximum_side || image.height > maximum_side {
            return Err("The mask plane exceeds this device's texture limit".into());
        }
        let PlanePixels { image, _cpu } = match &self.map {
            Map::Manual(map, display) => {
                overlay::plane_pixels(map.as_ref(), image, display, cancel)?
            }
            Map::Supplied(map, display) => {
                overlay::plane_pixels(map.as_ref(), image, display, cancel)?
            }
        };
        let gpu = MemoryBudget::gpu()
            .reserve(image.size[0] as u64 * image.size[1] as u64 * 4)
            .ok_or("Not enough GPU allowance for illustration masks")?;
        Ok(MaskPixels {
            image,
            _cpu,
            _gpu: gpu,
        })
    }
    pub fn paint(
        &self,
        painter: &egui::Painter,
        rect: egui::Rect,
        image: &Frame,
        view: &ViewState,
        texture: egui::TextureId,
    ) -> Result<(), &'static str> {
        let mesh =
            super::canvas::overlay_mesh(rect, image, view, texture, self.description.opacity)
                .ok_or("The mask view is invalid")?;
        painter.add(egui::Shape::mesh(mesh));
        Ok(())
    }
}
impl LabelUi {
    pub(crate) fn illustration_masks(
        &self,
        selected: usize,
    ) -> Result<Option<CapturedMasks>, &'static str> {
        if self.supplied_active {
            return self.supplied.illustration_masks(selected);
        }
        if !self.visible
            || self.series != selected
            || self.display.visible_ids[1..].iter().all(|v| !*v)
        {
            return Ok(None);
        }
        let Some(map) = &self.current else {
            return Ok(None);
        };
        if self.busy() || !self.stroke.is_empty() {
            return Err("Wait for the current label operation to finish, then reopen export");
        }
        if self.region_preview_visible() {
            return Err("Apply or discard the connected-region preview before including labels");
        }
        CapturedMasks::new(
            Map::Manual(map.clone(), Box::new(self.display.clone())),
            MaskGroup::Manual,
            self.display.mode,
            self.opacity,
        )
        .map(Some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn state() -> LabelUi {
        let original = crate::labels::tests::labels();
        let mut map = original.try_clone(|| false).unwrap();
        map.threshold(None, [20., 20.], 1, || false).unwrap();
        LabelUi {
            current: Some(Arc::new(map)),
            ..Default::default()
        }
    }
    #[test]
    fn applied_mask_capture_freezes_visibility_and_style_without_serializing_source_metadata() {
        let mut state = state();
        let source = state.current.as_ref().unwrap().source_arc();
        let captured = state.illustration_masks(0).unwrap().unwrap();
        state.display.visible_ids[1] = false;
        state.opacity = 1.;
        let pixels = captured.prepare(&source.frames()[1], 8, &|| false).unwrap();
        assert_eq!(pixels.image.size, [8, 8]);
        assert!(
            pixels
                .image
                .pixels
                .iter()
                .all(|p| *p == egui::Color32::from_gray(197))
        );
        assert_eq!(captured.description().opacity, 0.4);
        assert!(captured.description().visible_ids.contains(&1));
        let json = serde_json::to_value(captured.description()).unwrap();
        assert_eq!(json["group"], "manual");
        assert_eq!(json["style"], "fill");
        assert_eq!(json.as_object().unwrap().len(), 4);
        let hidden = state.illustration_masks(0).unwrap().unwrap();
        assert!(
            hidden
                .prepare(&source.frames()[1], 8, &|| false)
                .unwrap()
                .image
                .pixels
                .iter()
                .all(|p| p.a() == 0)
        );
        assert!(state.illustration_masks(1).unwrap().is_none());
    }
    #[test]
    fn mask_capture_keeps_missing_pixels_separate_and_rejects_reference_limits_and_cancel() {
        let state = state();
        let captured = state.illustration_masks(0).unwrap().unwrap();
        let source = captured.source();
        let mut image = (*source.frames()[1]).clone();
        let mut valid = vec![1; 64];
        valid[27] = 0;
        image.validity = Some(valid.into());
        let layer = captured.prepare(&image, 8, &|| false).unwrap();
        assert_eq!(layer.image.pixels.iter().filter(|p| p.a() != 0).count(), 63);
        assert_eq!(layer.image.pixels[27], egui::Color32::TRANSPARENT);
        assert!(captured.prepare(&image, 7, &|| false).is_err());
        assert!(captured.prepare(&image, 8, &|| true).is_err());
        image.geometry.as_mut().unwrap().frame_of_reference =
            Some("different generated reference".into());
        assert!(captured.prepare(&image, 8, &|| false).is_err());
    }
}
