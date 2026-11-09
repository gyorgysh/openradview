// SPDX-License-Identifier: GPL-3.0-or-later
//! Independent f64 plane intersections and completed CPU validity stencils.
use super::*;

pub(super) fn nearest(
    scene: &SectionScene,
    origin: [f64; 3],
    direction: [f64; 3],
    face: Option<&InterfaceHit>,
) -> Result<(u32, Option<f64>), &'static str> {
    let mut actor = u32::from(face.is_some());
    let mut nearest = face.map(|hit| hit.parameter_mm);
    for (slot, plane) in scene.planes.iter().enumerate() {
        let Some(plane) = plane else { continue };
        if let Some(depth) = intersection(plane, origin, direction, scene.volume.clip.as_ref())?
            // The two ray divisions can round an exactly shared physical
            // contact differently. Prove coincidence in the supplied plane
            // geometry without widening it by a physical epsilon.
            && !face.is_some_and(|hit| {
                let g = plane.image.grid().geometry();
                dot(std::array::from_fn(|a| g.position[a] - hit.position_mm[a]), g.normal()) == 0.
            })
            && nearest.is_none_or(|t| depth < t)
        {
            nearest = Some(depth);
            actor = slot as u32 + 2;
        }
    }
    Ok((actor, nearest))
}
pub(super) fn depth_matches(
    expected: Option<f64>,
    actual: f32,
    source: &orv_core::volume::Volume,
    direction: [f64; 3],
) -> bool {
    expected.map_or(actual == 1e30, |depth| {
        let speed = source
            .world_vector_to_voxel(direction)
            .into_iter()
            .map(f64::abs)
            .fold(0., f64::max);
        (f64::from(actual) - depth).abs() * speed <= 0.005
    })
}
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a.into_iter().zip(b).map(|(a, b)| a * b).sum()
}
fn intersection(
    plane: &SectionPlane,
    origin: [f64; 3],
    direction: [f64; 3],
    clip: Option<&orv_core::volume_view::PhysicalClip>,
) -> Result<Option<f64>, &'static str> {
    let geometry = plane.image.grid().geometry();
    let normal = geometry.normal();
    let denominator = dot(direction, normal);
    if denominator == 0. {
        return Ok(None);
    }
    let delta = std::array::from_fn(|a| geometry.position[a] - origin[a]);
    let depth = dot(delta, normal) / denominator;
    if !depth.is_finite() {
        return Err("The section probe plane ray is invalid");
    }
    let world = std::array::from_fn(|a| origin[a] + direction[a] * depth);
    let xy = geometry
        .world_to_pixel(world)
        .map_err(|_| "The section probe plane basis is invalid")?;
    let frame = plane.image.frame();
    if (0..2).any(|a| xy[a] < -0.5 || xy[a] >= f64::from([frame.width, frame.height][a]) - 0.5) {
        return Ok(None);
    }
    let (u, v) = geometry.directions();
    if clip.is_some_and(|clip| {
        (0..3).any(|a| {
            let local = u[a] * geometry.spacing[1] * xy[0] + v[a] * geometry.spacing[0] * xy[1];
            local + geometry.position[a] - clip.minimum_mm[a] < 0.
                || clip.maximum_mm[a] - geometry.position[a] - local < 0.
        })
    }) {
        return Ok(None);
    }
    let xy = xy.map(|v| {
        if (v - v.round()).abs() < 0.00001 {
            v.round()
        } else {
            v
        }
    });
    let base = xy.map(f64::floor);
    let blend = std::array::from_fn::<_, 2, _>(|a| xy[a] - base[a]);
    for y in 0..2 {
        for x in 0..2 {
            let weight = if x == 0 { 1. - blend[0] } else { blend[0] }
                * if y == 0 { 1. - blend[1] } else { blend[1] };
            if weight <= 0. {
                continue;
            }
            let x = (base[0] + x as f64).clamp(0., f64::from(frame.width - 1)) as usize;
            let y = (base[1] + y as f64).clamp(0., f64::from(frame.height - 1)) as usize;
            if !frame.has_sample(y * frame.width as usize + x) {
                return Ok(None);
            }
        }
    }
    Ok(Some(depth))
}
