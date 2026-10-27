// SPDX-License-Identifier: GPL-3.0-or-later
//! Synthetic CPU resampling timing; no files, patient data or telemetry.
use orv_core::{
    Plane, WindowLevel,
    study::{Frame, Geometry},
    volume::Volume,
};
use std::{sync::Arc, time::Instant};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let side = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "256".into())
        .parse::<u32>()?;
    if !(16..=512).contains(&side) {
        return Err("Use a side between16 and512".into());
    }
    let max_side = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "512".into())
        .parse::<u32>()?;
    let mode = std::env::args().nth(4);
    let (slab_mode, oblique_mode) = match mode.as_deref() {
        None => (false, false),
        Some("slab") => (true, false),
        Some("oblique") => (false, true),
        Some("slab-oblique") => (true, true),
        _ => return Err("Use slab, oblique or slab-oblique as the fourth argument".into()),
    };
    let depth = side / 2;
    let padded = std::env::args().nth(3).as_deref() == Some("padding");
    let mask: Option<Arc<[u8]>> = padded.then(|| {
        (0..side * side)
            .map(|i| {
                u8::from(i % side > 0 && i % side < side - 1 && i / side > 0 && i / side < side - 1)
            })
            .collect::<Vec<_>>()
            .into()
    });
    let frames = (0..depth)
        .map(|z| {
            Arc::new(Frame {
                slab: None,
                scalar: None,
                validity: mask.clone(),
                color: None,
                nifti: None,
                rgba: None,
                width: side,
                height: side,
                pixels: (0..side * side)
                    .map(|i| (i % side + i / side + z) as f32)
                    .collect::<Vec<_>>()
                    .into(),
                geometry: Some(Geometry {
                    nifti_transform: None,
                    frame_of_reference: Some("synthetic-timing".into()),
                    position: [0., 0., z as f64],
                    orientation: [1., 0., 0., 0., 1., 0.],
                    spacing: [1., 1.],
                }),
                instance: z as i32,
                frame_index: 0,
                monochrome1: false,
                default_window: WindowLevel {
                    function: Default::default(),
                    width: 256.,
                    center: 128.,
                },
            })
        })
        .collect();
    let volume = Volume::new(frames)?;
    let mut timings = Vec::new();
    let mut dimensions = Vec::new();
    for i in 0..21 {
        let start = Instant::now();
        let mut point = volume.center();
        point[2] += i as f64 * 0.1;
        let planes = [Plane::Axial, Plane::Coronal, Plane::Sagittal];
        for plane in if oblique_mode {
            &planes[..1]
        } else {
            &planes[..]
        } {
            let plane = *plane;
            let explicit_plane = if oblique_mode {
                orv_core::oblique::ObliquePlane::from_angles(point, [i as f64 * 0.5, 15., 7.])?
            } else {
                orv_core::oblique::ObliquePlane::anatomical(plane, point)?
            };
            let image = if slab_mode {
                let plane = explicit_plane;
                let mut slab = orv_core::slab::Slab::for_volume(
                    &volume,
                    10.,
                    orv_core::slab::SlabProjection::Maximum,
                );
                if max_side <= 128 {
                    slab = slab.preview();
                }
                volume.reformat_slab(&plane, slab, max_side, || false)?
            } else if oblique_mode {
                volume.reformat_oblique(&explicit_plane, max_side, || false)?
            } else {
                volume.reformat(plane, point, max_side, || false)?
            };
            std::hint::black_box(&image);
            if i == 0 {
                dimensions.push([image.width, image.height]);
            }
        }
        if i > 0 {
            timings.push(start.elapsed().as_secs_f64() * 1000.);
        }
    }
    timings.sort_by(f64::total_cmp);
    println!(
        "{}",
        serde_json::json!({"profile":if cfg!(debug_assertions){"debug"}else{"release"},"padding_mask":padded,"slab_10mm":slab_mode,"oblique":oblique_mode,"source":[side,side,depth],"planes":dimensions,"reformat_side":max_side,"samples":timings.len(),"median_ms":timings[10],"p95_ms":timings[18],"maximum_ms":timings[19],"scope":"CPU reformat set (three anatomical planes or one changing oblique plane); excludes upload, UI and input-to-photon"})
    );
    Ok(())
}
