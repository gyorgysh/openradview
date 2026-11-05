// SPDX-License-Identifier: GPL-3.0-or-later
//! A common physical depth origin for projected source and region actors.
use orv_core::{
    ViewState,
    volume_view::{Camera, RayFrame},
};

pub(crate) fn centred_frame(
    camera: &Camera,
    display: &ViewState,
    size: [u32; 2],
    reference_mm: [f64; 3],
) -> Result<RayFrame, &'static str> {
    let mut frame = camera.frame(display, f64::from(size[0]) / f64::from(size[1]))?;
    let relative: [f64; 3] = std::array::from_fn(|a| frame.origin_mm[a] - reference_mm[a]);
    let along = relative
        .into_iter()
        .zip(frame.direction_unit)
        .map(|(a, b)| a * b)
        .sum::<f64>();
    frame.origin_mm = std::array::from_fn(|a| relative[a] - along * frame.direction_unit[a]);
    Ok(frame)
}
