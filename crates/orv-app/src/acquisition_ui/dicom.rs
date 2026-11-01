// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use orv_core::acquisition::{CineMetadata, DicomAxis, DicomCatalog};
impl Controls {
    pub fn show_dicom(
        &mut self,
        ui: &mut egui::Ui,
        catalog: &DicomCatalog,
        selected: usize,
    ) -> Option<usize> {
        if catalog.axes(selected).is_none() && catalog.cine(selected).is_none() {
            return None;
        }
        egui::ScrollArea::vertical()
            .id_salt("dicom-acquisition-controls")
            .max_height(240.)
            .show(ui, |ui| {
                let choice = self.dicom_content(ui, catalog, selected);
                crate::dialogs::reveal_focus(ui);
                choice
            })
            .inner
    }
    fn dicom_content(
        &mut self,
        ui: &mut egui::Ui,
        catalog: &DicomCatalog,
        selected: usize,
    ) -> Option<usize> {
        let mut choice = None;
        if let Some(axes) = catalog.axes(selected) {
            if self.notice.is_some_and(|(owner, _)| owner != selected) {
                self.notice = None;
            }
            egui::CollapsingHeader::new("Acquisition axes").default_open(true).show(ui,|ui| {
                    let axes = match axes { Ok(axes) => axes, Err(error) => { ui.small(error.text()); return; } };
                    ui.small("Choices use declared dimensions. Changing acquisition clears temporary marks and closes reconstructed views. Labels stay with their original subseries.");
                    let mut ordinals = DicomAxis::ALL.map(|axis| axes.value(axis).map_or(0,|v| v.ordinal));
                    let original = ordinals;
                    for axis in DicomAxis::ALL {
                        let Some(current) = axes.value(axis) else { continue; };
                        let options = catalog.options(selected,axis);
                        let Ok(index) = options.binary_search_by_key(&current.ordinal,|v| v.ordinal) else { continue; };
                        let mut index = index+1;
                        ui.add_enabled_ui(options.len()>1,|ui| {
                            crate::toolbar::number(ui,match axis { DicomAxis::TemporalPosition => "Position choice", DicomAxis::EchoTime => "Echo choice" },egui::DragValue::new(&mut index).range(1..=options.len()).suffix(format!(" / {}",options.len())).speed(1));
                        });
                        if let Some(value) = options.get(index-1) { ordinals[axis.index()] = value.ordinal; }
                        let value = current.value.map_or_else(|| "no declared value".into(),|value| format!("{}{}",number(value),if axis==DicomAxis::EchoTime { " ms" } else { "" }));
                        ui.small(format!("{}: {value} · ordinal {}",axis.label(),current.ordinal));
                    }
                    ui.small("A temporal position number is not elapsed time. Ordinals and declared values can differ.");
                    if axes.other_dimensions() { ui.small("Other declared dimensions stay fixed."); }
                    if ui.is_enabled() && ordinals!=original {
                        match catalog.resolve(selected,ordinals) {
                            Ok(index) => { self.notice = None; choice = Some(index); },
                            Err(error) => { self.notice = Some((selected,error)); },
                        }
                    }
                    if self.notice.is_some() { ui.small("That acquisition is missing or ambiguous. The current series is unchanged; use the series list for an explicit choice."); }
                    crate::dialogs::reveal_focus(ui);
            });
        }
        if let Some(timing) = catalog.cine(selected) {
            egui::CollapsingHeader::new("Source timing").show(ui,|ui| {
                match timing {
                    CineMetadata::Absent => {},
                    CineMetadata::Unavailable(error) => { ui.small(error.text()); },
                    CineMetadata::Available(timing) => {
                        ui.small(if timing.variable() { "Declared per-frame intervals" } else { "Declared fixed frame interval" });
                        if !timing.variable() && let Some(interval) = timing.interval_before(1) { ui.small(format!("Interval: {} ms",number(interval.as_secs_f64()*1000.))); }
                        let trim = timing.trim(); ui.small(format!("Trim: images {}–{} of {}",trim[0]+1,trim[1]+1,timing.frame_count()));
                        if let Some(span) = timing.seconds_from_first(timing.frame_count()-1) { ui.small(format!("First to last frame: {} ms",number(span*1000.))); }
                        if let Some(delay) = timing.delay_ms() { ui.small(format!("Declared frame delay: {} ms",number(delay))); }
                        ui.small("Intervals follow original object frame order. Enable Source intervals below the image to use them for playback; manual fps remains the default.");
                    }
                }
            });
        }
        choice
    }
}
