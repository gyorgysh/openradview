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
    pub(crate) fn open_sheet_export(&mut self, frame: &eframe::Frame) {
        self.pause_playback();
        let captured = (|| {
            let mut input = self.capture_illustration(frame)?;
            input.sheet = Some(match self.mode {
                ViewMode::Acquired => sheets::Sheet::Source {
                    marks: self.annotations.source_outlines()?,
                    total: input.base.series.frame_count(),
                },
                ViewMode::Mpr => {
                    let mpr = self.mpr.as_ref().ok_or("No completed MPR grid")?;
                    let (_, ready) = mpr.rendered_view().ok_or("No completed MPR grid")?;
                    if !ready {
                        return Err("Wait for all MPR planes to finish");
                    }
                    let snapshot = mpr.snapshot.as_ref().ok_or("No completed MPR grid")?;
                    if snapshot.images.iter().any(|image| image.slab.is_some()) {
                        return Err("MPR sheets currently need thin planes");
                    }
                    let mut panes = Vec::new();
                    panes
                        .try_reserve_exact(3)
                        .map_err(|_| "Not enough memory for MPR capture")?;
                    for index in 0..3 {
                        let image = snapshot.images[index].clone();
                        let mut view = mpr.views[index].clone();
                        view.slice = 0;
                        let outlines = mpr.annotations[index].outlines(&image, &view)?;
                        panes.push(FrozenInput {
                            base: super::super::Input {
                                series: input.base.series.clone(),
                                pixels: Pixels::Reformat(image, snapshot.volume.clone()),
                                scene: orv_core::scene::SceneState {
                                    schema_version: 1,
                                    source_sha256: "0".repeat(64),
                                    series_index: input.base.scene.series_index,
                                    view,
                                    crosshair_mm: Some(snapshot.point),
                                    reformat_side: snapshot.reformat_side,
                                    output_size: mpr.picture_sizes[index].unwrap_or([512, 512]),
                                    reconstruction: None,
                                },
                                device: input.base.device.clone(),
                                queue: input.base.queue.clone(),
                            },
                            outlines,
                            masks: input.masks.clone(),
                            mask_note: input.mask_note,
                            sheet: None,
                        });
                    }
                    sheets::Sheet::Mpr { panes }
                }
                _ => return Err("Sheets need source images or a completed thin MPR grid"),
            });
            Ok(input)
        })();
        self.export.close();
        self.export.illustration.open(captured);
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
            masks: masks.map(Arc::new),
            mask_note,
            sheet: None,
        })
    }
}
