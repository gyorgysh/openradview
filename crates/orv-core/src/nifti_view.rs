// SPDX-License-Identifier: GPL-3.0-or-later
//! Numeric NIfTI subvolume identity; no header descriptions or paths.
use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComplexView {
    Scalar,
    Magnitude,
    PhaseRadians,
    Real,
    Imaginary,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NiftiView {
    pub version: u8,
    /// Axes 4..7: time, component and two additional nonspatial axes. Zero-based.
    pub indices: [u32; 4],
    pub dimensions: [u32; 4],
    pub complex: ComplexView,
}
impl NiftiView {
    pub fn label(&self) -> String {
        let mut parts = vec![format!("NIfTI-{}", self.version)];
        for (i, name) in ["Time", "Component", "Axis 6", "Axis 7"].iter().enumerate() {
            if self.dimensions[i] > 1 {
                parts.push(format!(
                    "{name} {}/{}",
                    self.indices[i] + 1,
                    self.dimensions[i]
                ));
            }
        }
        match self.complex {
            ComplexView::Scalar => {}
            ComplexView::Magnitude => parts.push("Magnitude".into()),
            ComplexView::PhaseRadians => parts.push("Phase (radians)".into()),
            ComplexView::Real => parts.push("Real".into()),
            ComplexView::Imaginary => parts.push("Imaginary".into()),
        }
        parts.join(" · ")
    }
}
