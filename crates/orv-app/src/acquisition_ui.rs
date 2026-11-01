// SPDX-License-Identifier: GPL-3.0-or-later
//! Header-only axis controls return the same series choices as the local browser.
use eframe::egui;
mod dicom;
use orv_core::{
    acquisition::{NiftiCatalog, Sampling, SelectionError},
    nifti_view::ComplexView,
};

#[derive(Default)]
pub(super) struct Controls {
    notice: Option<(usize, SelectionError)>,
}
impl Controls {
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        catalog: &NiftiCatalog,
        selected: usize,
    ) -> Option<usize> {
        let axes = catalog.axes(selected)?;
        let view = axes.view();
        if view.dimensions == [1; 4] && view.complex == ComplexView::Scalar {
            return None;
        }
        if self.notice.is_some_and(|(owner, _)| owner != selected) {
            self.notice = None;
        }
        let mut choice = None;
        egui::CollapsingHeader::new("Acquisition axes").default_open(true).show(ui, |ui| {
            egui::ScrollArea::vertical().id_salt("acquisition-axis-controls").max_height(220.).show(ui, |ui| {
                ui.small("Choose a subseries from this input. Changing it clears temporary measurements; reconstructed views return to source images. Labels stay with their original subseries.");
                let mut indices = view.indices;
                for (axis, label) in ["Time index", "Component index", "Axis 6 index", "Axis 7 index"].into_iter().enumerate() {
                    let count = view.dimensions[axis];
                    if count > 1 {
                        let mut value = indices[axis] + 1;
                        crate::toolbar::number(ui, label, egui::DragValue::new(&mut value).range(1..=count).suffix(format!(" / {count}")).speed(1));
                        indices[axis] = value - 1;
                    }
                }
                let mut complex = view.complex;
                if complex != ComplexView::Scalar {
                    egui::ComboBox::from_id_salt("acquisition-complex-view").selected_text(complex_label(complex)).show_ui(ui, |ui| {
                        for value in [ComplexView::Magnitude, ComplexView::PhaseRadians, ComplexView::Real, ComplexView::Imaginary] { ui.selectable_value(&mut complex, value, complex_label(value)); }
                    });
                }
                match axes.sampling() {
                    Sampling::Seconds { step, .. } => {
                        if let Some(coordinate) = axes.coordinate() { ui.small(format!("Time coordinate {} s · step {} s", number(coordinate), number(step))); }
                    }
                    Sampling::Frequency { step, unit, .. } => {
                        if let Some(coordinate) = axes.coordinate() { ui.small(format!("Frequency coordinate {} {} · step {} {}. This is not elapsed time.", number(coordinate), unit.label(), number(step), unit.label())); }
                    }
                    Sampling::UnknownUnits => { ui.small("Time-axis units are unknown; only the index is available."); }
                    Sampling::Invalid(_) => { ui.small("Time-axis metadata is invalid; only the index is available."); }
                    Sampling::Absent => {}
                }
                if ui.is_enabled() && (indices != view.indices || complex != view.complex) {
                    match catalog.resolve(selected, indices, complex) {
                        Ok(index) => { self.notice = None; choice = Some(index); }
                        Err(error) => { self.notice = Some((selected,error)); }
                    }
                }
                if let Some((_, error)) = self.notice {
                    ui.small(match error {
                        SelectionError::OutOfRange => "That axis index is outside the source dimensions.",
                        SelectionError::Unavailable => "That acquisition is unavailable in this input. The current series is unchanged.",
                        SelectionError::Ambiguous => "That acquisition matches more than one series. Choose its series explicitly below.",
                    });
                }
                crate::dialogs::reveal_focus(ui);
            });
        });
        choice
    }
}
fn complex_label(value: ComplexView) -> &'static str {
    match value {
        ComplexView::Scalar => "Scalar",
        ComplexView::Magnitude => "Magnitude",
        ComplexView::PhaseRadians => "Phase (radians)",
        ComplexView::Real => "Real",
        ComplexView::Imaginary => "Imaginary",
    }
}
fn number(value: f64) -> String {
    if value == 0. {
        return "0".into();
    }
    if (0.0001..1_000_000.).contains(&value.abs()) {
        format!("{value:.6}")
            .trim_end_matches('0')
            .trim_end_matches('.')
            .into()
    } else {
        format!("{value:.6e}")
    }
}

#[cfg(test)]
mod tests;
