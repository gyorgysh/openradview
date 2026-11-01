// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use crate::workspace::{ViewMode, Workspace};

impl Workspace {
    pub(crate) fn open_illustration_export(&mut self, frame: &eframe::Frame) {
        self.pause_playback();
        let input = self.capture_illustration(frame);
        self.export.close();
        self.export.illustration.open(input);
    }
    fn capture_illustration(&self, frame: &eframe::Frame) -> Result<FrozenInput, &'static str> {
        let base = self
            .picture_input(frame)
            .ok_or("Wait for the displayed pane to finish")?;
        let image = match &base.pixels {
            Pixels::Image(image) | Pixels::Reformat(image, _) if image.slab.is_none() => image,
            _ => return Err("Illustrations currently need a source image or thin reformat"),
        };
        let view = &base.scene.view;
        let outlines = match self.mode {
            ViewMode::Acquired => self.annotations.outlines(image, view),
            ViewMode::Mpr => self
                .mpr
                .as_ref()
                .ok_or("No settled MPR view")
                .and_then(|mpr| mpr.annotations[mpr.active_index()].outlines(image, view)),
            ViewMode::Oblique => self
                .oblique
                .as_ref()
                .ok_or("No settled oblique view")
                .and_then(|session| session.illustration_outlines(image, view)),
            ViewMode::Compare => self
                .comparison
                .as_ref()
                .ok_or("No comparison pane")
                .and_then(|session| session.illustration_outlines(image, view)),
            ViewMode::Volume => return Err("Illustrations currently need a thin image plane"),
        }?;
        let (masks, mask_note) = if self.mode == ViewMode::Compare {
            (None, Some("Comparison panes have no applied mask layer"))
        } else {
            match self.labels.illustration_masks(self.selected) {
                Ok(Some(mask)) => {
                    if matches!(&base.pixels, Pixels::Reformat(_, volume) if !Arc::ptr_eq(volume, &mask.source()))
                    {
                        (None, Some("Masks belong to a different source volume"))
                    } else {
                        (Some(mask), None)
                    }
                }
                Ok(None) => (None, Some("No applied mask group is currently shown")),
                Err(reason) => (None, Some(reason)),
            }
        };
        Ok(FrozenInput {
            base,
            outlines,
            masks,
            mask_note,
        })
    }
}
