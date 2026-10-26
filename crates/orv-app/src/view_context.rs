// SPDX-License-Identifier: GPL-3.0-or-later
//! Source coordinate context; never infer a scanner/patient reference for a template.
use eframe::egui;
use orv_core::{inspection, study::Frame};

pub(crate) fn reference(frame: &Frame) -> Option<&'static str> {
    let transform = frame.geometry.as_ref().and_then(|g| g.nifti_transform);
    if frame.nifti.is_none() && transform.is_none() {
        return None;
    }
    Some(
        frame
            .geometry
            .as_ref()
            .filter(|g| g.validate().is_ok() && g.nifti_transform.is_some())
            .map(inspection::coordinate_reference)
            .unwrap_or("Unknown reference · source grid only"),
    )
}
pub(crate) fn show(ui: &mut egui::Ui, frame: &Frame) {
    if let Some(reference) = reference(frame) {
        ui.small(format!("Space: {reference}"))
            .on_hover_text("Coordinates and direction letters use the file's declared reference. Scanner, aligned and template spaces are different; the viewer does not register them.");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orv_core::study::{NiftiMethod, NiftiTransform};
    #[test]
    fn painted_context_keeps_template_and_scanner_spaces_distinct() {
        let mut frame = (*crate::mpr_worker::tests::frames()[0]).clone();
        assert_eq!(reference(&frame), None);
        let ctx = egui::Context::default();
        for (code, expected) in [
            (1, "Scanner"),
            (2, "Aligned"),
            (3, "Talairach"),
            (4, "MNI152"),
            (5, "Template"),
        ] {
            frame.geometry.as_mut().unwrap().nifti_transform = Some(NiftiTransform {
                method: NiftiMethod::Sform,
                code,
            });
            assert!(reference(&frame).unwrap().starts_with(expected));
            let mut out = ctx.run_ui(Default::default(), |ui| show(ui, &frame));
            out.textures_delta.clear();
            assert!(out.shapes.iter().any(|s| matches!(&s.shape, egui::epaint::Shape::Text(t) if t.galley.job.text.starts_with(&format!("Space: {expected}")))));
        }
    }
}
