// SPDX-License-Identifier: GPL-3.0-or-later
//! Local halfspace cuts through completed MPR positions. No derived cut caps.
use eframe::egui;
use orv_core::{volume::Volume, volume_view::PhysicalClip};

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Side {
    #[default]
    Off,
    Lower,
    Higher,
}
impl Side {
    fn name(self) -> &'static str {
        match self {
            Self::Off => "No linked cut",
            Self::Lower => "Keep ≤ point",
            Self::Higher => "Keep ≥ point",
        }
    }
}
#[derive(Default)]
pub(super) struct Cuts {
    sides: [Side; 3],
}
pub(super) enum CutError {
    Empty,
    Unsupported(&'static str),
}
impl CutError {
    pub fn message(&self) -> &'static str {
        match self {
            Self::Empty => {
                "The linked cuts and clipping bounds leave no volume. Adjust or reset the cuts in 3D settings."
            }
            Self::Unsupported(error) => error,
        }
    }
}
impl Cuts {
    pub fn active(&self) -> bool {
        self.sides.iter().any(|side| *side != Side::Off)
    }
    pub fn ui(&mut self, ui: &mut egui::Ui, point: [f64; 3]) {
        ui.collapsing("Cut at linked MPR planes", |ui| {
            ui.small("Keep one side of the completed MPR point in 3D. These cuts intersect the separate Clipping bounds. Source images, labels and physical brush scope stay unchanged; no cap is added.");
            for (axis, name) in [(2, "Axial / S"), (1, "Coronal / P"), (0, "Sagittal / L")] {
                ui.horizontal_wrapped(|ui| {
                    ui.label(format!("{name}: {:.3} mm", point[axis]));
                    egui::ComboBox::from_id_salt(("linked-MPR-cut", axis))
                        .selected_text(self.sides[axis].name()).width(135.)
                        .show_ui(ui, |ui| {
                            for side in [Side::Off, Side::Lower, Side::Higher] {
                                if ui.selectable_value(&mut self.sides[axis], side, side.name()).clicked() { ui.close(); }
                            }
                        });
                });
            }
            ui.small("L, P and S are source left, posterior and superior coordinates. Lower/higher refers to these numbers.");
            if ui.button("Reset linked cuts").clicked() { self.sides.fill(Side::Off); }
        });
    }
    pub fn clip(
        &self,
        source: &Volume,
        point: [f64; 3],
        manual: Option<&PhysicalClip>,
    ) -> Result<Option<PhysicalClip>, CutError> {
        if !self.active() {
            return Ok(manual.cloned());
        }
        if !point.iter().all(|value| value.is_finite()) {
            return Err(CutError::Unsupported(
                "Linked cuts require a finite completed MPR point",
            ));
        }
        // The mask's half-cell domain is larger than the source-centre sampling
        // box. A generous source-cell margin prevents unselected axes from
        // clipping its outer faces, including under affine shear and rounding.
        let dimensions = source.dimensions();
        let mut minimum = [f64::INFINITY; 3];
        let mut maximum = [f64::NEG_INFINITY; 3];
        for bits in 0..8 {
            let corner = source.voxel_to_world(std::array::from_fn(|axis| {
                if bits & (1 << axis) == 0 {
                    -0.5
                } else {
                    dimensions[axis] as f64 - 0.5
                }
            }));
            for axis in 0..3 {
                minimum[axis] = minimum[axis].min(corner[axis]);
                maximum[axis] = maximum[axis].max(corner[axis]);
            }
        }
        let margin = (0..3)
            .map(|axis| {
                source
                    .voxel_vector_to_world(std::array::from_fn(
                        |index| if index == axis { 1. } else { 0. },
                    ))
                    .iter()
                    .map(|value| value * value)
                    .sum::<f64>()
                    .sqrt()
            })
            .fold(0f64, f64::max)
            * 2.;
        let mut clip = PhysicalClip {
            minimum_mm: minimum.map(|value| value - margin),
            maximum_mm: maximum.map(|value| value + margin),
        };
        if let Some(manual) = manual {
            manual.validate().map_err(CutError::Unsupported)?;
            for axis in 0..3 {
                clip.minimum_mm[axis] = clip.minimum_mm[axis].max(manual.minimum_mm[axis]);
                clip.maximum_mm[axis] = clip.maximum_mm[axis].min(manual.maximum_mm[axis]);
            }
        }
        for (axis, side) in self.sides.iter().enumerate() {
            match side {
                Side::Off => (),
                Side::Lower => clip.maximum_mm[axis] = clip.maximum_mm[axis].min(point[axis]),
                Side::Higher => clip.minimum_mm[axis] = clip.minimum_mm[axis].max(point[axis]),
            }
        }
        if (0..3).any(|axis| clip.minimum_mm[axis] >= clip.maximum_mm[axis]) {
            return Err(CutError::Empty);
        }
        clip.validate().map_err(CutError::Unsupported)?;
        Ok(Some(clip))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    fn source() -> Volume {
        let template = orv_core::study::demo_study().series[0].frames[0].clone();
        Volume::new(
            (0..6)
                .map(|z| {
                    let mut frame = (*template).clone();
                    frame.width = 4;
                    frame.height = 5;
                    frame.pixels = vec![1.; 20].into();
                    let geometry = frame.geometry.as_mut().unwrap();
                    geometry.position = [
                        17. + z as f64 * 0.2,
                        -31. + z as f64 * 0.3,
                        40. + z as f64 * 1.5,
                    ];
                    geometry.orientation = [-1., 0., 0., 0., 1., 0.];
                    geometry.spacing = [0.7, 2.];
                    Arc::new(frame)
                })
                .collect(),
        )
        .unwrap()
    }
    #[test]
    fn inactive_cuts_preserve_exact_manual_bounds_and_selected_halfspaces_intersect_them() {
        let source = source();
        let point = source.center();
        let manual = PhysicalClip {
            minimum_mm: [10., -40., 39.],
            maximum_mm: [22., -25., 50.],
        };
        let mut cuts = Cuts::default();
        let original = cuts
            .clip(&source, point, Some(&manual))
            .ok()
            .unwrap()
            .unwrap();
        assert_eq!(original, manual);
        cuts.sides = [Side::Lower, Side::Higher, Side::Lower];
        let clip = cuts
            .clip(&source, point, Some(&manual))
            .ok()
            .unwrap()
            .unwrap();
        assert_eq!(clip.minimum_mm, [10., point[1], 39.]);
        assert_eq!(clip.maximum_mm, [point[0], -25., point[2]]);
        assert_eq!(manual.minimum_mm, [10., -40., 39.]);
        assert_eq!(manual.maximum_mm, [22., -25., 50.]);
    }
    #[test]
    fn unselected_axes_contain_every_affine_half_cell_corner_with_shear_and_reflection() {
        let source = source();
        let cuts = Cuts {
            sides: [Side::Off, Side::Off, Side::Lower],
        };
        let clip = cuts
            .clip(&source, source.center(), None)
            .ok()
            .unwrap()
            .unwrap();
        // Independently expand the generated fixture's three affine columns.
        for x in [-0.5, 3.5] {
            for y in [-0.5, 4.5] {
                for z in [-0.5, 5.5] {
                    let point = [
                        17. - 2. * x + 0.2 * z,
                        -31. + 0.7 * y + 0.3 * z,
                        40. + 1.5 * z,
                    ];
                    for (axis, value) in point.iter().enumerate().take(2) {
                        assert!(clip.minimum_mm[axis] < *value && *value < clip.maximum_mm[axis]);
                    }
                }
            }
        }
        assert_eq!(clip.maximum_mm[2], source.center()[2]);
    }
    #[test]
    fn empty_intersections_and_unsupported_coordinates_are_explicit_instead_of_falling_back() {
        let source = source();
        let cuts = Cuts {
            sides: [Side::Higher, Side::Off, Side::Off],
        };
        let point = source.center();
        let manual = PhysicalClip {
            minimum_mm: [0., -100., 0.],
            maximum_mm: [point[0], 100., 100.],
        };
        assert!(matches!(
            cuts.clip(&source, point, Some(&manual)),
            Err(CutError::Empty)
        ));
        assert!(matches!(
            cuts.clip(&source, [f64::NAN, 0., 0.], None),
            Err(CutError::Unsupported(_))
        ));
        assert!(matches!(
            cuts.clip(&source, [2e7, 0., 0.], None),
            Err(CutError::Empty) | Err(CutError::Unsupported(_))
        ));
    }
}
