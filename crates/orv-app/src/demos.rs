// SPDX-License-Identifier: GPL-3.0-or-later
//! Offline, licensed public examples. The source data retain their own licenses.
use eframe::egui;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Demo {
    Ct,
    Mr,
    Xray,
}

impl Demo {
    pub const ALL: [Self; 3] = [Self::Ct, Self::Mr, Self::Xray];

    pub fn title(self) -> &'static str {
        match self {
            Self::Ct => "Chest CT · NLST",
            Self::Mr => "Breast MRI · I-SPY 1",
            Self::Xray => "Chest X-ray · COVID-19-AR",
        }
    }

    pub fn credit(self) -> &'static str {
        match self {
            Self::Ct => "NLST Research Team · TCIA · CC BY 4.0",
            Self::Mr => "Newitt / Hylton / I-SPY 1 Team · TCIA · CC BY 3.0",
            Self::Xray => "Desai et al. · TCIA · CC BY 4.0",
        }
    }

    pub fn bytes(self) -> &'static [u8] {
        match self {
            Self::Ct => include_bytes!("../../../assets/demos/nlst-ct.zip"),
            Self::Mr => include_bytes!("../../../assets/demos/ispy1-mr.zip"),
            Self::Xray => include_bytes!("../../../assets/demos/covid-xray.zip"),
        }
    }
}

pub fn credits(ctx: &egui::Context, open: &mut bool) {
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
        for (demo, count) in [(Demo::Ct, 80), (Demo::Mr, 64), (Demo::Xray, 1)] {
            let study = load_zip(demo.bytes(), &AtomicBool::new(false), |_| {}).unwrap();
            assert!(!study.synthetic);
            assert_eq!(study.skipped_files, 0);
            assert_eq!(study.series.len(), 1);
            assert_eq!(study.frame_count(), count);
            let name = match demo {
                Demo::Ct => "nlst-ct.zip",
                Demo::Mr => "ispy1-mr.zip",
                Demo::Xray => "covid-xray.zip",
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
            if demo == Demo::Xray {
                assert!(volume.is_err(), "A radiograph must never become a 3D stack");
            } else {
                let volume = volume.unwrap();
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
    fn cancelled_demo_import_stops_before_decoding() {
        assert!(matches!(
            load_zip(Demo::Ct.bytes(), &AtomicBool::new(true), |_| {}),
            Err(orv_core::load::LoadError::Cancelled)
        ));
    }
}
