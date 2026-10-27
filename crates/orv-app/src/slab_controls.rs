// SPDX-License-Identifier: GPL-3.0-or-later
use eframe::egui;
use orv_core::{
    slab::{Slab, SlabProjection},
    volume::Volume,
};
pub fn show(ui: &mut egui::Ui, slab: &mut Option<Slab>, volume: &Volume) -> bool {
    let before = *slab;
    crate::toolbar::row(ui, |ui| {
        let mut projection = slab.map(|s| s.projection);
        egui::ComboBox::from_id_salt("slab-projection")
            .selected_text(match projection {
                None => "Thin plane",
                Some(SlabProjection::Maximum) => "Slab MIP",
                Some(SlabProjection::Minimum) => "Slab MinIP",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut projection, None, "Thin plane");
                ui.selectable_value(&mut projection, Some(SlabProjection::Maximum), "Slab MIP");
                ui.selectable_value(&mut projection, Some(SlabProjection::Minimum), "Slab MinIP");
            });
        match projection {
            None => *slab = None,
            Some(projection) => {
                let mut thickness = slab.map_or(10., |s| s.thickness_mm);
                let changed = crate::toolbar::number(
                    ui,
                    "Thickness",
                    egui::DragValue::new(&mut thickness)
                        .range(0.001..=2000.)
                        .speed(0.5)
                        .suffix(" mm"),
                )
                .changed();
                if changed || slab.is_none_or(|s| s.projection != projection) {
                    *slab = Some(Slab::for_volume(volume, thickness, projection));
                }
                if let Some(slab) = slab {
                    ui.label(format!("{} samples · projection; measurement off", slab.intervals + 1))
                    .on_hover_text("Discrete extrema along the physical normal, including both boundaries and the center. At most 257 samples; thick slabs can miss features between samples. Preview uses fewer samples until interaction stops.");
                }
            }
        }
    });
    *slab != before
}
