// SPDX-License-Identifier: GPL-3.0-or-later
use orv_core::{
    ViewState,
    volume::Volume,
    volume_view::{Projection, TransferShape, VolumeView},
};

pub(crate) fn parameters(
    volume: &Volume,
    display: &ViewState,
    state: &VolumeView,
    size: [u32; 2],
    region: [[u32; 2]; 2],
) -> Result<[f32; 32], &'static str> {
    state.validate()?;
    display.validate()?;
    if size.iter().any(|d| !(1..=1024).contains(d))
        || region[1].contains(&0)
        || (0..2).any(|i| {
            region[0][i]
                .checked_add(region[1][i])
                .is_none_or(|end| end > size[i])
        })
    {
        return Err("Volume output or tile exceeds supported bounds");
    }
    if display.window.width > 1e6 || display.window.center.abs() > 1e9 {
        return Err("Volume display window exceeds supported bounds");
    }
    let rays = state
        .camera
        .frame(display, f64::from(size[0]) / f64::from(size[1]))?;
    let origin = volume.world_to_voxel(rays.origin_mm);
    let right = volume.world_vector_to_voxel(rays.right_mm);
    let down = volume.world_vector_to_voxel(rays.down_mm);
    let direction = volume.world_vector_to_voxel(rays.direction_unit);
    let invert = display.invert ^ volume.frames()[0].monochrome1;
    let mut values = [0.; 32];
    for (i, vector) in [origin, right, down, direction].into_iter().enumerate() {
        for axis in 0..3 {
            values[i * 4 + axis] = vector[axis] as f32;
        }
    }
    values[16..20].copy_from_slice(&[
        display.window.width,
        display.window.center,
        u8::from(invert) as f32,
        if state.projection == Projection::Composite {
            1.
        } else {
            0.
        },
    ]);
    values[20..24].copy_from_slice(&[
        state.transfer.low,
        state.transfer.high,
        state.transfer.opacity_per_mm,
        if state.transfer.shape == TransferShape::Band {
            1.
        } else {
            0.
        },
    ]);
    // Bound advance along every source-grid axis, including strong regular
    // shear. Minimum physical spacing alone can skip thin tilted features.
    let speed = direction.into_iter().map(f64::abs).fold(0.0, f64::max);
    values[24] = (state.quality.step_factor() / speed) as f32;
    values[25] = state.quality.max_steps() as f32;
    values[26] = u8::from(volume.is_color()) as f32;
    values[27] = u8::from(volume.has_missing_scalar()) as f32;
    values[28..32].copy_from_slice(&[
        region[0][0] as f32,
        region[0][1] as f32,
        region[1][0] as f32,
        region[1][1] as f32,
    ]);
    if values.iter().any(|v| !v.is_finite() || v.abs() > 1e20) || values[24] <= 0.0 {
        return Err("Physical volume exceeds GPU coordinate precision limits");
    }
    Ok(values)
}
