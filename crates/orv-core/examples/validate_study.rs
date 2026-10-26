// SPDX-License-Identifier: GPL-3.0-or-later
//! Explicit local import smoke check. Prints counts/geometry only; no raw tags or paths.
use orv_core::{Plane, index::index_paths, volume::Volume};
use std::{error::Error, path::PathBuf};

fn main() -> Result<(), Box<dyn Error>> {
    let paths = std::env::args_os().skip(1).map(PathBuf::from);
    for (case, path) in paths.enumerate() {
        let study = index_paths(&[path], || false, |_| {})?;
        println!(
            "Case {}: {} series, {} images, {} skipped files",
            case + 1,
            study.series.len(),
            study.frame_count(),
            study.skipped_files
        );
        for series in study.series {
            let count = series.frame_count();
            for index in [0, count / 2, count - 1] {
                let frame = series.read_frame(index, || false)?;
                if frame.pixels.iter().any(|v| !v.is_finite()) {
                    return Err("Non-finite decoded source pixels".into());
                }
            }
            let frames = series.volume_frames(|| false)?;
            match Volume::new(frames) {
                Ok(volume) => {
                    for plane in [Plane::Axial, Plane::Coronal, Plane::Sagittal] {
                        let image = volume.reformat(plane, volume.center(), 128, || false)?;
                        if !image.pixels.iter().any(|v| v.is_finite()) {
                            return Err("Empty center reformat".into());
                        }
                    }
                    println!(
                        "  {} {:?}: spatial stack and three MPR planes validated",
                        series.modality,
                        volume.dimensions()
                    );
                }
                Err(error) => println!(
                    "  {}: 2D available; 3D/MPR refused: {error}",
                    series.modality
                ),
            }
        }
    }
    Ok(())
}
