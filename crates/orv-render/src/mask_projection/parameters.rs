// SPDX-License-Identifier: GPL-3.0-or-later
use super::{MAX_SIDE, MaskProjection};
use std::ops::Range;
pub(super) fn values(
    request: &MaskProjection,
    rows: Range<u32>,
) -> Result<[f32; 48], &'static str> {
    if request.number == 0 {
        return Err("Background is not a mask segment");
    }
    if request.size.iter().any(|v| !(1..=MAX_SIDE).contains(v))
        || rows.start >= rows.end
        || rows.end > request.size[1]
    {
        return Err("Mask output or row batch exceeds supported bounds");
    }
    if let Some(clip) = &request.clip {
        clip.validate()?;
    }
    request.input.view(request.number)?;
    let source = request.input.source().volume();
    let rays = request.camera.frame(
        &request.display,
        f64::from(request.size[0]) / f64::from(request.size[1]),
    )?;
    let mut values = [0.; 48];
    for (index, vector) in [
        source.world_to_voxel(rays.origin_mm),
        source.world_vector_to_voxel(rays.right_mm),
        source.world_vector_to_voxel(rays.down_mm),
        source.world_vector_to_voxel(rays.direction_unit),
    ]
    .into_iter()
    .enumerate()
    {
        for axis in 0..3 {
            values[index * 4 + axis] = vector[axis] as f32;
        }
    }
    if let Some(clip) = &request.clip {
        let reference = source.voxel_to_world([0.; 3]);
        let relative = |point: [f64; 3]| std::array::from_fn(|axis| point[axis] - reference[axis]);
        let minimum = relative(clip.minimum_mm);
        let maximum = relative(clip.maximum_mm);
        if (0..3).any(|a| minimum[a] as f32 >= maximum[a] as f32) {
            return Err("Mask clip exceeds GPU coordinate precision");
        }
        for (index, vector) in [
            relative(rays.origin_mm),
            rays.right_mm,
            rays.down_mm,
            rays.direction_unit,
            minimum,
            maximum,
        ]
        .into_iter()
        .enumerate()
        {
            for axis in 0..3 {
                values[16 + index * 4 + axis] = vector[axis] as f32;
            }
        }
        values[46] = 1.;
    }
    values[40..44].copy_from_slice(&[
        0.,
        rows.start as f32,
        request.size[0] as f32,
        (rows.end - rows.start) as f32,
    ]);
    values[44] = f32::from(request.number);
    values[45] = u8::from(source.has_missing_scalar()) as f32;
    let (representation, maximum) = request.input.representation();
    values[47] = representation;
    for (slot, dimension) in [3, 7, 11].into_iter().zip(source.dimensions()) {
        values[slot] = dimension as f32;
    }
    values[15] = maximum;
    let speed = values[12..15]
        .iter()
        .copied()
        .map(f32::abs)
        .fold(0., f32::max);
    if speed == 0. || 1. / speed > 1e20 {
        return Err("Mask rays exceed GPU distance precision");
    }
    if values.iter().any(|v| !v.is_finite() || v.abs() > 1e20)
        || values[12..15].iter().all(|v| *v == 0.)
    {
        return Err("Mask rays exceed GPU arithmetic limits");
    }
    Ok(values)
}
