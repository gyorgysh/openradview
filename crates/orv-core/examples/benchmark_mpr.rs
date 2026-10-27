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
        for plane in [Plane::Axial, Plane::Coronal, Plane::Sagittal] {
            let image = volume.reformat(plane, point, max_side, || false)?;
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
        serde_json::json!({"profile":if cfg!(debug_assertions){"debug"}else{"release"},"padding_mask":padded,"source":[side,side,depth],"planes":dimensions,"reformat_side":max_side,"samples":timings.len(),"median_ms":timings[10],"p95_ms":timings[18],"maximum_ms":timings[19],"scope":"CPU three-plane resampling; excludes upload, UI and input-to-photon"})
    );
    Ok(())
}
