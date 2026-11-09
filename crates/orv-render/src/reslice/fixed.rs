// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
pub(super) const FIXED_WORDS: usize = 28 + 257 * 8;
const SCALE: f64 = 1099511627776.;

/// Signed 24.40 coordinates, accumulated with exact u32 carry arithmetic in the
/// shader. The preflight bounds every partial sum well inside signed i64.
pub(super) fn fixed_parameters(
    source: &GpuVolumeSource,
    grid: &ReformatGrid,
) -> Result<[u32; FIXED_WORDS], &'static str> {
    let volume = source.volume();
    let g = grid.geometry();
    let (u, v) = g.directions();
    let mul = |v: [f64; 3], s: f64| v.map(|x| x * s);
    let vectors = [
        volume.world_to_voxel(g.position),
        volume.world_vector_to_voxel(mul(u, g.spacing[1])),
        volume.world_vector_to_voxel(mul(v, g.spacing[0])),
    ];
    let normal = volume.world_vector_to_voxel(g.normal());
    let mut offsets = [[0f64; 3]; 257];
    let samples = grid.slab().map_or(1, |s| usize::from(s.intervals) + 1);
    if let Some(slab) = grid.slab() {
        for (i, offset) in offsets[..samples].iter_mut().enumerate() {
            let distance = slab.thickness_mm * (i as f64 / f64::from(slab.intervals) - 0.5);
            *offset = mul(normal, distance);
        }
    }
    let max_index = grid.dimensions().map(|n| f64::from(n - 1));
    let mut result = [0; FIXED_WORDS];
    let mut errors = [[0f64; 3]; 3];
    let mut exact = [true; 3];
    let mut magnitudes = [[0u64; 3]; 3];
    for (i, vector) in vectors.iter().enumerate() {
        for (a, &value) in vector.iter().enumerate() {
            let offset_max = offsets[..samples]
                .iter()
                .map(|v| v[a].abs())
                .fold(0., f64::max);
            let magnitude = vectors[0][a].abs()
                + vectors[1][a].abs() * max_index[0]
                + vectors[2][a].abs() * max_index[1]
                + offset_max;
            if !magnitude.is_finite() || magnitude > 4194304. {
                return Err("Reformat coordinates exceed fixed-point precision limits");
            }
            let fixed = (value * SCALE).round() as i64;
            result[i * 8 + a] = fixed as u32;
            result[i * 8 + 4 + a] = (fixed as u64 >> 32) as u32;
            errors[i][a] = (value - fixed as f64 / SCALE).abs();
            magnitudes[i][a] = fixed.unsigned_abs();
            exact[a] &= errors[i][a] == 0.;
        }
    }
    let mut offset_error = [0f64; 3];
    let mut offset_magnitude = [0u64; 3];
    for (i, offset) in offsets[..samples].iter().enumerate() {
        for (a, &value) in offset.iter().enumerate() {
            let fixed = (value * SCALE).round() as i64;
            result[28 + i * 8 + a] = fixed as u32;
            result[28 + i * 8 + 4 + a] = (fixed as u64 >> 32) as u32;
            let error = (value - fixed as f64 / SCALE).abs();
            offset_error[a] = offset_error[a].max(error);
            offset_magnitude[a] = offset_magnitude[a].max(fixed.unsigned_abs());
            exact[a] &= error == 0.;
        }
    }
    for a in 0..3 {
        let integer_magnitude = u128::from(magnitudes[0][a])
            + u128::from(magnitudes[1][a]) * u128::from(grid.dimensions()[0] - 1)
            + u128::from(magnitudes[2][a]) * u128::from(grid.dimensions()[1] - 1)
            + u128::from(offset_magnitude[a]);
        exact[a] &= integer_magnitude <= 1u128 << 53;
        let magnitude = vectors[0][a].abs()
            + vectors[1][a].abs() * max_index[0]
            + vectors[2][a].abs() * max_index[1]
            + offsets[..samples]
                .iter()
                .map(|v| v[a].abs())
                .fold(0., f64::max);
        let error = errors[0][a]
            + errors[1][a] * max_index[0]
            + errors[2][a] * max_index[1]
            + offset_error[a]
            + if exact[a] {
                0.
            } else {
                8. * f64::EPSILON * magnitude
            };
        let ticks = if error == 0. {
            0.
        } else {
            (error * SCALE).ceil() + 1.
        };
        if ticks > f64::from(u32::MAX / 3) {
            return Err("Reformat coordinate uncertainty exceeds fixed-point limits");
        }
        result[24 + a] = ticks as u32;
    }
    Ok(result)
}
