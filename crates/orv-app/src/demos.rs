// SPDX-License-Identifier: GPL-3.0-or-later
//! Offline, licensed public examples. The source data retain their own licenses.
use eframe::egui;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Demo {
    Ct,
    Mr,
    Xray,
    Mammogram,
    Ultrasound,
    Pet,
    ProstateMask,
}

/// Picker groups, by what a person wants to try rather than by collection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    Volumes,
    Radiographs,
    UltrasoundPet,
    Masks,
}

impl Group {
    pub const ALL: [Self; 4] = [
        Self::Volumes,
        Self::Radiographs,
        Self::UltrasoundPet,
        Self::Masks,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Self::Volumes => "CT and MRI",
            Self::Radiographs => "X-ray and mammography",
            Self::UltrasoundPet => "Ultrasound and PET",
            Self::Masks => "Supplied masks",
        }
    }

    pub fn demos(self) -> impl Iterator<Item = Demo> {
        Demo::ALL
            .into_iter()
            .filter(move |demo| demo.group() == self)
    }
}

impl Demo {
    pub const ALL: [Self; 7] = [
        Self::Ct,
        Self::Mr,
        Self::Xray,
        Self::Mammogram,
        Self::Ultrasound,
        Self::Pet,
        Self::ProstateMask,
    ];

    pub fn group(self) -> Group {
        match self {
            Self::Ct | Self::Mr => Group::Volumes,
            Self::Xray | Self::Mammogram => Group::Radiographs,
            Self::Ultrasound | Self::Pet => Group::UltrasoundPet,
            Self::ProstateMask => Group::Masks,
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Ct => "Chest CT",
            Self::Mr => "Breast MRI",
            Self::Xray => "Chest X-ray",
            Self::Mammogram => "Mammogram",
            Self::Ultrasound => "Liver ultrasound",
            Self::Pet => "Whole-body PET",
            Self::ProstateMask => "Prostate MRI + AI mask",
        }
    }

    /// Collection and what the example is good for trying, without findings.
    pub fn summary(self) -> &'static str {
        match self {
            Self::Ct => "NLST · 80 slices · MPR, oblique, 3D",
            Self::Mr => "I-SPY 1 · 64 slices · MPR, projection",
            Self::Xray => "COVID-19-AR · one large radiograph",
            Self::Mammogram => "CMMD · 4 views · side and view labels",
            Self::Ultrasound => "CMB-MEL · 13 images · calibrated mm",
            Self::Pet => "ACRIN 6668 · 172 slices · Bq/mL, MIP",
            Self::ProstateMask => "ProstateX · 19 slices · mask in Labels…",
        }
    }

    pub fn credit(self) -> &'static str {
        match self {
            Self::Ct => "NLST Research Team · TCIA · CC BY 4.0",
            Self::Mr => "Newitt / Hylton / I-SPY 1 Team · TCIA · CC BY 3.0",
            Self::Xray => "Desai et al. · TCIA · CC BY 4.0",
            Self::Mammogram => "Cui et al. · CMMD · TCIA · CC BY 4.0",
            Self::Ultrasound => "Cancer Moonshot Biobank · CMB-MEL · TCIA · CC BY 4.0",
            Self::Pet => "Kinahan et al. · ACRIN 6668 · TCIA · CC BY 3.0",
            Self::ProstateMask => {
                "ProstateX · TCIA · CC BY 3.0 · AI mask: BAMF Health / IDC · CC BY 4.0"
            }
        }
    }

    pub fn bytes(self) -> &'static [u8] {
        match self {
            Self::Ct => include_bytes!("../../../assets/demos/nlst-ct.zip"),
            Self::Mr => include_bytes!("../../../assets/demos/ispy1-mr.zip"),
            Self::Xray => include_bytes!("../../../assets/demos/covid-xray.zip"),
            Self::Mammogram => include_bytes!("../../../assets/demos/cmmd-mg.zip"),
            Self::Ultrasound => include_bytes!("../../../assets/demos/cmb-mel-us.zip"),
            Self::Pet => include_bytes!("../../../assets/demos/acrin-pet.zip"),
            Self::ProstateMask => include_bytes!("../../../assets/demos/prostatex-mr.zip"),
        }
    }

    /// A supplied DICOM SEG produced for this exact series, loaded only on request.
    pub fn mask(self) -> Option<&'static [u8]> {
        match self {
            Self::ProstateMask => Some(include_bytes!(
                "../../../assets/demos/prostatex-aimi-seg.dcm"
            )),
            _ => None,
        }
    }
}

/// An equal-height example card: title and summary inside one keyboard-focusable
/// control, so rows line up whatever the text length.
pub fn card(ui: &mut egui::Ui, demo: Demo, width: f32, enabled: bool) -> egui::Response {
    let height = 52.0;
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, height), sense);
    response
        .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, demo.title()));
    if ui.is_rect_visible(rect) {
        let visuals = if enabled {
            *ui.style().interact(&response)
        } else {
            ui.style().visuals.widgets.noninteractive
        };
        let painter = ui.painter();
        painter.rect(
            rect,
            visuals.corner_radius,
            visuals.weak_bg_fill,
            visuals.bg_stroke,
            egui::StrokeKind::Inside,
        );
        let inner = rect.shrink2(egui::vec2(12.0, 8.0));
        let line = |text: &str, font: egui::FontId, color: egui::Color32| {
            let mut job = egui::text::LayoutJob::simple_singleline(text.into(), font, color);
            job.wrap = egui::text::TextWrapping::truncate_at_width(inner.width());
            ui.fonts_mut(|fonts| fonts.layout_job(job))
        };
        let title = line(
            demo.title(),
            egui::TextStyle::Body.resolve(ui.style()),
            visuals.text_color(),
        );
        let summary = line(
            demo.summary(),
            egui::TextStyle::Small.resolve(ui.style()),
            crate::theme::SILVER,
        );
        let summary_top = inner.bottom() - summary.size().y;
        painter.galley(inner.left_top(), title, visuals.text_color());
        painter.galley(
            egui::pos2(inner.left(), summary_top),
            summary,
            crate::theme::SILVER,
        );
        if response.has_focus() {
            painter.rect_stroke(
                rect.expand(2.0),
                visuals.corner_radius,
                ui.style().visuals.selection.stroke,
                egui::StrokeKind::Outside,
            );
        }
    }
    response
}

pub fn credits(ctx: &egui::Context, open: &mut bool) {
    crate::dialogs::escape(ctx, open);
    crate::dialogs::window("Public scan credits", ctx)
        .open(open)
        .default_width(610.0)
        .default_height(440.0)
        .vscroll(true)
        .show(ctx, |ui| {
            ui.label("Public, de-identified research images. Collection membership is not a diagnosis. Educational examples, not for clinical decisions.");
            ui.separator();
            // Plain text: opening credits never launches a link or uses the network.
            ui.label(include_str!("../../../assets/demos/CREDITS.txt"));
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use orv_core::{Plane, load::load_zip, volume::Volume};
    use std::sync::atomic::AtomicBool;

    #[test]
    fn public_scans_decode_and_spatial_stacks_reformat() {
        for (demo, count) in [
            (Demo::Ct, 80),
            (Demo::Mr, 64),
            (Demo::Xray, 1),
            (Demo::Mammogram, 4),
            (Demo::Ultrasound, 13),
            (Demo::Pet, 172),
            (Demo::ProstateMask, 19),
        ] {
            let study = load_zip(demo.bytes(), &AtomicBool::new(false), |_| {}).unwrap();
            assert!(!study.synthetic);
            assert_eq!(study.skipped_files, 0);
            assert_eq!(study.series.len(), 1);
            assert_eq!(study.frame_count(), count);
            let name = match demo {
                Demo::Ct => "nlst-ct.zip",
                Demo::Mr => "ispy1-mr.zip",
                Demo::Xray => "covid-xray.zip",
                Demo::Mammogram => "cmmd-mg.zip",
                Demo::Ultrasound => "cmb-mel-us.zip",
                Demo::Pet => "acrin-pet.zip",
                Demo::ProstateMask => "prostatex-mr.zip",
            };
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../assets/demos")
                .join(name);
            let indexed = orv_core::index::index_paths(&[path], || false, |_| {}).unwrap();
            assert_eq!(indexed.frame_count(), count);
            assert_eq!(indexed.skipped_files, 0);
            for index in [0, count / 2, count - 1] {
                let resident = study.series[0].read_frame(index, || false).unwrap();
                let lazy = indexed.series[0].read_frame(index, || false).unwrap();
                assert_eq!(resident.pixels, lazy.pixels);
                assert_eq!(resident.geometry, lazy.geometry);
                assert_eq!(resident.default_window, lazy.default_window);
                assert_eq!(resident.monochrome1, lazy.monochrome1);
            }
            let frames = study.series[0].frames.clone();
            assert!(
                frames
                    .iter()
                    .all(|f| f.pixels.iter().all(|p| p.is_finite()))
            );
            let volume = Volume::new(frames);
            if matches!(demo, Demo::Xray | Demo::Mammogram | Demo::Ultrasound) {
                assert!(
                    volume.is_err(),
                    "A projection or 2D frame must never become a 3D stack"
                );
            } else {
                let volume = volume.unwrap();
                if demo == Demo::Ct {
                    // The bundled instances' ImagePositionPatient Z values,
                    // not an inferred zero origin or a distance from the head.
                    let [min, max] = volume.bounds();
                    assert!((min[2] - (-1122.80005)).abs() < 0.001);
                    assert!((max[2] - (-870.)).abs() < 0.001);
                    assert!((volume.center()[2] - (-996.400025)).abs() < 0.001);
                }
                for plane in [Plane::Axial, Plane::Coronal, Plane::Sagittal] {
                    let image = volume
                        .reformat(plane, volume.center(), 128, || false)
                        .unwrap();
                    assert!(image.pixels.iter().any(|p| p.is_finite()));
                }
            }
        }
    }

    #[test]
    fn new_examples_carry_the_declarations_their_features_rely_on() {
        let load = |demo: Demo| load_zip(demo.bytes(), &AtomicBool::new(false), |_| {}).unwrap();
        let mammogram = load(Demo::Mammogram);
        let mut views: Vec<String> = mammogram.series[0]
            .frames
            .iter()
            .map(|frame| {
                let labels = frame.projection.as_ref().unwrap();
                assert!(labels.edges().is_some());
                labels.summary().unwrap()
            })
            .collect();
        views.sort();
        assert_eq!(views, ["L · CC", "L · MLO", "R · CC", "R · MLO"]);
        let ultrasound = load(Demo::Ultrasound);
        assert!(
            ultrasound.series[0]
                .frames
                .iter()
                .all(|frame| frame.calibration.is_some())
        );
        let pet = load(Demo::Pet);
        assert_eq!(
            pet.series[0].frames[0]
                .scalar
                .as_ref()
                .unwrap()
                .info()
                .unit
                .label(),
            "Declared Bq/mL"
        );
        // Only the prostate example has a supplied mask, and it matches only that series.
        assert!(
            Demo::ALL
                .iter()
                .filter(|demo| demo.mask().is_some())
                .count()
                == 1
        );
        let prostate = load(Demo::ProstateMask);
        let mask = Demo::ProstateMask.mask().unwrap();
        let masks =
            orv_core::index::read_segmentation_bytes(mask, &prostate.series[0], || false).unwrap();
        let infos: Vec<_> = masks.infos().collect();
        assert_eq!(infos.len(), 1);
        assert_eq!((infos[0].number, infos[0].supplied_planes), (1, 11));
        assert_eq!(infos[0].positive_cells, 16995);
        assert!(
            orv_core::index::read_segmentation_bytes(mask, &load(Demo::Mr).series[0], || false)
                .is_err()
        );
        for group in Group::ALL {
            assert!(group.demos().count() >= 1);
        }
        assert_eq!(
            Group::ALL.iter().map(|g| g.demos().count()).sum::<usize>(),
            Demo::ALL.len()
        );
    }

    #[test]
    fn cancelled_demo_import_stops_before_decoding() {
        assert!(matches!(
            load_zip(Demo::Ct.bytes(), &AtomicBool::new(true), |_| {}),
            Err(orv_core::load::LoadError::Cancelled)
        ));
    }
}
