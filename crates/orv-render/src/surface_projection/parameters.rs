// SPDX-License-Identifier: GPL-3.0-or-later
use super::{MAX_SIDE, SurfaceProjection};
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a.into_iter().zip(b).map(|(a, b)| a * b).sum()
}
pub(super) struct Values {
    pub uniform: [f32; 84],
    pub reference_mm: [f64; 3],
    pub direction_unit: [f64; 3],
}
pub(super) fn values(request: &SurfaceProjection) -> Result<Values, &'static str> {
    if request.size.iter().any(|s| !(1..=MAX_SIDE).contains(s)) {
        return Err("Surface image exceeds its supported bounds");
    }
    if let Some(clip) = &request.clip {
        clip.validate()?;
    }
    let source = request.surface.source().volume();
    let frame = request.camera.frame(
        &request.display,
        f64::from(request.size[0]) / f64::from(request.size[1]),
    )?;
    let reference_mm = source.center();
    let dimensions = source.dimensions();
    let basis: [[_; 3]; 3] = std::array::from_fn(|a| {
        let mut u = [0.; 3];
        u[a] = 1.;
        source.voxel_vector_to_world(u)
    });
    let rr = dot(frame.right_mm, frame.right_mm);
    let dd = dot(frame.down_mm, frame.down_mm);
    if !rr.is_finite() || !dd.is_finite() || rr <= 0. || dd <= 0. {
        return Err("Surface camera span cannot be represented");
    }
    let depth_coeff = basis.map(|b| dot(b, frame.direction_unit));
    let radius = depth_coeff
        .into_iter()
        .zip(dimensions)
        .map(|(a, d)| a.abs() * d as f64 * 0.5)
        .sum::<f64>();
    let mut minimum_spacing = f64::INFINITY;
    for a in 0..3 {
        let inverse: [f64; 3] = std::array::from_fn(|i| {
            let mut u = [0.; 3];
            u[i] = 1.;
            source.world_vector_to_voxel(u)[a]
        });
        minimum_spacing = minimum_spacing.min(1. / dot(inverse, inverse).sqrt());
    }
    if !radius.is_finite() || radius <= 0. || 2.02 * radius / minimum_spacing > 1048576. {
        return Err("Surface depth range exceeds supported GPU precision");
    }
    let delta: [f64; 3] = std::array::from_fn(|a| reference_mm[a] - frame.origin_mm[a]);
    let mut all = [0f64; 84];
    for a in 0..3 {
        all[a] = 2. * dot(basis[a], frame.right_mm) / rr;
        all[4 + a] = -2. * dot(basis[a], frame.down_mm) / dd;
        all[8 + a] = depth_coeff[a] / (2.02 * radius);
        all[12 + a] = depth_coeff[a];
        for axis in 0..3 {
            all[16 + axis * 4 + a] = basis[a][axis];
        }
    }
    all[3] = 2. * dot(delta, frame.right_mm) / rr;
    all[7] = -2. * dot(delta, frame.down_mm) / dd;
    all[11] = 0.5;
    for side in 0..6 {
        let axis = side / 2;
        let sign = if side % 2 == 0 { -1. } else { 1. };
        let normal: [f64; 3] = std::array::from_fn(|a| {
            let mut u = [0.; 3];
            u[a] = 1.;
            source.world_vector_to_voxel(u)[axis] * sign
        });
        let normal = orv_core::volume_material::gradient_normal(normal)
            .ok_or("Surface normal cannot be represented")?;
        all[28 + side * 4..31 + side * 4].copy_from_slice(&normal);
    }
    for a in 0..3 {
        all[52 + a] = (dimensions[a] - 1) as f64 * 0.5;
    }
    if let Some(clip) = &request.clip {
        for a in 0..3 {
            all[56 + a] = clip.minimum_mm[a] - reference_mm[a];
            all[60 + a] = clip.maximum_mm[a] - reference_mm[a];
        }
        if (0..3).any(|a| all[56 + a] as f32 >= all[60 + a] as f32) {
            return Err("Surface clip exceeds GPU coordinate precision");
        }
        all[65] = 1.;
    }
    all[64] = f64::from(request.surface.surface().key().number);
    let origin = source.world_vector_to_voxel(delta.map(|v| -v));
    let right = source.world_vector_to_voxel(frame.right_mm);
    let down = source.world_vector_to_voxel(frame.down_mm);
    let direction = source.world_vector_to_voxel(frame.direction_unit);
    for (offset, vector) in [(68, origin), (72, right), (76, down), (80, direction)] {
        all[offset..offset + 3].copy_from_slice(&vector);
    }
    all[71] = 1. / f64::from(request.size[0]);
    all[75] = 1. / f64::from(request.size[1]);
    let uniform = all.map(|v| v as f32);
    if uniform.iter().any(|v| !v.is_finite() || v.abs() > 1e20) {
        return Err("Surface parameters exceed GPU arithmetic limits");
    }
    // Check the rounded linear projection, not a world-origin subtraction in f32.
    let row = |i: usize| std::array::from_fn::<_, 3, _>(|a| f64::from(uniform[i * 4 + a]));
    let [a, b, c] = [row(0), row(1), row(2)];
    let determinant = a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0])
        + a[2] * (b[0] * c[1] - b[1] * c[0]);
    if determinant == 0. || !determinant.is_finite() {
        return Err("Surface projection collapses at GPU precision");
    }
    Ok(Values {
        uniform,
        reference_mm,
        direction_unit: frame.direction_unit,
    })
}
