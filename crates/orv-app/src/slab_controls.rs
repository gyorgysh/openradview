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
                Some(SlabProjection::Mean) => "Slab mean",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut projection, None, "Thin plane");
                ui.selectable_value(&mut projection, Some(SlabProjection::Maximum), "Slab MIP");
                ui.selectable_value(&mut projection, Some(SlabProjection::Minimum), "Slab MinIP");
                ui.selectable_value(&mut projection, Some(SlabProjection::Mean), "Slab mean");
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
                    let explanation = String::from(
                        "Discrete extrema or mean of valid samples along the physical normal, including both boundaries and the center. The mean is a sample average, not an exact volume integral. At most 257 samples; thick slabs can miss features between samples.",
                    );
                    ui.label(format!(
                        "{} samples · projection; measurement off",
                        slab.intervals + 1
                    ))
                    .on_hover_text(explanation);
                }
            }
        }
    });
    *slab != before
}

/// Describe the completed image, not the settings still being rendered.
pub fn pending_sampling(displayed: Option<Slab>, requested: Option<Slab>) -> Option<String> {
    if displayed == requested {
        return None;
    }
    let shown = match displayed {
        None => "thin plane".to_owned(),
        Some(slab) => {
            let projection = match slab.projection {
                SlabProjection::Maximum => "MIP",
                SlabProjection::Minimum => "MinIP",
                SlabProjection::Mean => "mean",
            };
            format!(
                "{projection} ({} mm, {} samples)",
                slab.thickness_mm,
                slab.intervals + 1
            )
        }
    };
    Some(format!("Showing {shown}; updating…"))
}

pub fn status_line(ui: &mut egui::Ui, status: Option<&str>) {
    // Keep image layout stable as requests arrive and finish.
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 18.), egui::Sense::hover());
    if let Some(status) = status {
        ui.put(
            rect,
            egui::Label::new(
                egui::RichText::new(status)
                    .size(11.)
                    .color(crate::theme::SILVER),
            )
            .truncate(),
        )
        .on_hover_text(status);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_status_uses_the_displayed_sampling_even_when_the_request_changes() {
        let mip = Slab {
            projection: SlabProjection::Maximum,
            thickness_mm: 80.,
            intervals: 250,
        };
        let mean = Slab {
            projection: SlabProjection::Mean,
            ..mip
        };
        assert_eq!(pending_sampling(Some(mip), Some(mip)), None);
        assert_eq!(
            pending_sampling(None, Some(mip)).as_deref(),
            Some("Showing thin plane; updating…")
        );
        assert_eq!(
            pending_sampling(Some(mip), Some(mean)).as_deref(),
            Some("Showing MIP (80 mm, 251 samples); updating…")
        );
        assert_eq!(
            pending_sampling(Some(mean), None).as_deref(),
            Some("Showing mean (80 mm, 251 samples); updating…")
        );
        assert!(
            pending_sampling(
                Some(mip),
                Some(Slab {
                    thickness_mm: 10.,
                    ..mip
                })
            )
            .is_some()
        );
    }

    #[test]
    fn pending_status_is_accessible_without_moving_the_image_area() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let status = "Showing MIP (80 mm, 251 samples); updating…";
        let mut image_areas = Vec::new();
        for message in [None, Some(status), None] {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(420., 400.),
                    )),
                    ..Default::default()
                },
                |ui| {
                    status_line(ui, message);
                    image_areas.push(ui.available_rect_before_wrap());
                },
            );
            if message.is_some() {
                let update = output.platform_output.accesskit_update.take().unwrap();
                assert!(
                    update
                        .nodes
                        .iter()
                        .any(|(_, node)| node.label() == Some(status)
                            || node.value() == Some(status))
                );
            }
            output.textures_delta.clear();
        }
        assert!(image_areas.windows(2).all(|areas| areas[0] == areas[1]));
    }
}
