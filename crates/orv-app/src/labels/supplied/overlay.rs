// SPDX-License-Identifier: GPL-3.0-or-later
use super::super::overlay::{Mode, OverlayMap, SampledPlane};
use eframe::egui;
use orv_core::{
    segments::{SegmentPlane, Segments},
    study::Frame,
    volume::Volume,
};
use std::sync::Arc;
#[derive(Clone, PartialEq, Eq)]
pub(in crate::labels) struct Display {
    pub visible: u64,
    pub mode: Mode,
}
impl Default for Display {
    fn default() -> Self {
        Self {
            visible: u64::MAX,
            mode: Mode::Fill,
        }
    }
}
impl SampledPlane for SegmentPlane {
    fn dimensions(&self) -> [usize; 2] {
        SegmentPlane::dimensions(self)
    }
}
impl OverlayMap for Segments {
    type Display = Display;
    type Plane = SegmentPlane;
    fn source_arc(&self) -> Arc<Volume> {
        Segments::source_arc(self)
    }
    fn plane(
        &self,
        image: &Frame,
        cancel: &dyn Fn() -> bool,
    ) -> Result<SegmentPlane, &'static str> {
        self.sample_plane(image, cancel)
    }
    fn color(plane: &SegmentPlane, index: usize, display: &Display) -> egui::Color32 {
        let bits = plane.positive_bits()[index] & display.visible;
        if bits == 0 || plane.eligibility()[index] != 1 {
            return egui::Color32::TRANSPARENT;
        }
        let [width, height] = plane.dimensions();
        let (x, y) = (index % width, index / width);
        let boundary = || {
            x == 0
                || y == 0
                || x + 1 == width
                || y + 1 == height
                || [index - 1, index + 1, index - width, index + width]
                    .into_iter()
                    .any(|other| {
                        plane.positive_bits()[other] & display.visible != bits
                            || plane.eligibility()[other] != 1
                    })
        };
        let id = plane.numbers()[bits.trailing_zeros() as usize];
        let mark = match display.mode {
            Mode::Fill => true,
            Mode::Outline => boundary(),
            Mode::Hatched => boundary() || (x + y + id as usize) % 6 < 2,
        };
        if !mark {
            return egui::Color32::TRANSPARENT;
        }
        let gray = if bits.count_ones() > 1 {
            if (x + y) % 6 < 2 || (x + 6 - y % 6) % 6 < 2 {
                255
            } else {
                150
            }
        } else {
            160 + (u32::from(id) * 37 % 96) as u8
        };
        egui::Color32::from_gray(gray)
    }
}
