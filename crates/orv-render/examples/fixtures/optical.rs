// SPDX-License-Identifier: GPL-3.0-or-later
//! Generated unique scalar planes shared by local optical benchmarks.
use orv_core::{
    study::{Geometry, demo_study},
    volume::Volume,
};
use std::sync::Arc;
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
pub(super) fn source(kind: &str) -> Result<Arc<Volume>> {
    let template = demo_study().series.remove(0).frames.remove(0);
    let mut frames = Vec::new();
    for z in 0..128 {
        let mut f = (*template).clone();
        let mut pixels = Vec::with_capacity(512 * 512);
        let mut valid = Vec::new();
        for y in 0..512 {
            for x in 0..512 {
                let inside = ((x as f64 - 220.) / 100.).powi(2)
                    + ((y as f64 - 240.) / 80.).powi(2)
                    + ((z as f64 - 64.) / 43.).powi(2)
                    <= 1.;
                let plate = x == 257 && (180..340).contains(&y);
                let known = kind != "missing" || (z >= 40 && z % 17 != 0);
                let value = if !known {
                    if x % 2 == 0 { -1e20 } else { 1e20 }
                } else if kind == "dense" {
                    (x + 2 * y + 3 * z) as f32
                } else if inside || plate {
                    ((x * 13 + y * 7 + z * 17) % 700 + 100) as f32
                } else {
                    -1000.
                };
                pixels.push(value);
                if kind == "missing" {
                    valid.push(u8::from(known));
                }
            }
        }
        f.width = 512;
        f.height = 512;
        f.pixels = pixels.into();
        f.validity = (kind == "missing").then(|| valid.into());
        f.geometry = Some(Geometry {
            position: [z as f64 * 0.05, 0., z as f64 * 1.2],
            orientation: [1., 0., 0., 0., 1., 0.],
            spacing: [0.7, 0.7],
            frame_of_reference: Some("generated optical acceleration".into()),
            nifti_transform: None,
        });
        frames.push(Arc::new(f));
    }
    Ok(Volume::prepare(frames, || false)?)
}
