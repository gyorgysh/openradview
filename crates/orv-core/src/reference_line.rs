// SPDX-License-Identifier: GPL-3.0-or-later
//! Finite intersections of two declared thin-image footprints, without registration.
use crate::{
    spatial::GeometryError,
    study::{Frame, Geometry},
    vector::*,
};

/// Coordinates use zero-based pixel centres in the target image. Footprint edges
/// lie at -0.5 and size-0.5; missing pixel values do not change declared geometry.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Intersection {
    Segment([[f64; 2]; 2]),
    /// No stable unique line at the supported angular resolution.
    Parallel,
    /// Nearly parallel planes agree within 1e-6 mm across the target footprint.
    Coplanar,
    /// The finite footprints have no line segment in common (including a point touch).
    Outside,
}
const ANGULAR_EPSILON: f64 = 1e-8;
const PLANE_EPSILON_MM: f64 = 1e-6;

/// Header-only, allocation-free. Both images must carry the same nonempty private
/// reference and NIfTI transform context. Projected slabs cannot act as planes.
/// Geometry validation is not registration or evidence that supplied tags are correct.
pub fn intersection(target: &Frame, other: &Frame) -> Result<Intersection, GeometryError> {
    if target.slab.is_some() || other.slab.is_some() {
        return Err(GeometryError::Pixels);
    }
    let size = [target.width, target.height];
    let other_size = [other.width, other.height];
    if size.contains(&0) || other_size.contains(&0) {
        return Err(GeometryError::Dimensions);
    }
    let target = target.geometry.as_ref().ok_or(GeometryError::Missing)?;
    let other = other.geometry.as_ref().ok_or(GeometryError::Missing)?;
    target.validate()?;
    other.validate()?;
    if target
        .frame_of_reference
        .as_ref()
        .is_none_or(|value| value.is_empty() || value.len() > 128)
        || target.frame_of_reference != other.frame_of_reference
        || target.nifti_transform != other.nifti_transform
    {
        return Err(GeometryError::Reference);
    }
    let normal = unit_normal(other);
    let target_normal = unit_normal(target);
    let (u, v) = target.directions();
    let u = mul(u, target.spacing[1]);
    let v = mul(v, target.spacing[0]);
    let delta = sub(target.position, other.position);
    let coefficients = [dot(normal, u), dot(normal, v)];
    let offset = dot(normal, delta);
    if !finite(delta)
        || !finite(u)
        || !finite(v)
        || !offset.is_finite()
        || coefficients.iter().any(|value| !value.is_finite())
    {
        return Err(GeometryError::Pixels);
    }
    // Work relative to the source origins. Subtracting two large world-space
    // endpoint positions would lose small image extents unnecessarily.
    let corners = [
        [-0.5, -0.5],
        [f64::from(size[0]) - 0.5, -0.5],
        [-0.5, f64::from(size[1]) - 0.5],
        [f64::from(size[0]) - 0.5, f64::from(size[1]) - 0.5],
    ];
    let distances =
        corners.map(|point| offset + coefficients[0] * point[0] + coefficients[1] * point[1]);
    if distances.iter().any(|value| !value.is_finite())
        || corners
            .iter()
            .any(|point| !finite(relative_point(delta, u, v, *point)))
    {
        return Err(GeometryError::Pixels);
    }
    if norm(cross(normal, target_normal)) <= ANGULAR_EPSILON {
        return Ok(
            if distances
                .iter()
                .all(|value| value.abs() <= PLANE_EPSILON_MM)
            {
                Intersection::Coplanar
            } else {
                Intersection::Parallel
            },
        );
    }
    if distances.iter().all(|value| *value > 0.) || distances.iter().all(|value| *value < 0.) {
        return Ok(Intersection::Outside);
    }
    let [a, b] = coefficients;
    let length = a.hypot(b);
    if !length.is_finite() || length <= 0. {
        return Err(GeometryError::Pixels);
    }
    let origin = if a.abs() >= b.abs() {
        [-offset / a, 0.]
    } else {
        [0., -offset / b]
    };
    let direction = [-b / length, a / length];
    if origin.iter().any(|value| !value.is_finite()) {
        return Err(GeometryError::Pixels);
    }
    let Some(range) = clip(origin, direction, size, [f64::NEG_INFINITY, f64::INFINITY]) else {
        return Ok(Intersection::Outside);
    };
    let endpoints = range.map(|t| point(origin, direction, t));
    if endpoints.iter().flatten().any(|value| !value.is_finite()) {
        return Err(GeometryError::Pixels);
    }
    let first = other_pixel(other, relative_point(delta, u, v, endpoints[0]))?;
    let last = other_pixel(other, relative_point(delta, u, v, endpoints[1]))?;
    let other_direction = [last[0] - first[0], last[1] - first[1]];
    if other_direction.iter().any(|value| !value.is_finite()) {
        return Err(GeometryError::Pixels);
    }
    let Some(range) = clip(first, other_direction, other_size, [0., 1.]) else {
        return Ok(Intersection::Outside);
    };
    let direction = [
        endpoints[1][0] - endpoints[0][0],
        endpoints[1][1] - endpoints[0][1],
    ];
    let endpoints = range.map(|t| point(endpoints[0], direction, t));
    if (endpoints[1][0] - endpoints[0][0]).hypot(endpoints[1][1] - endpoints[0][1]) <= 1e-9 {
        return Ok(Intersection::Outside);
    }
    Ok(Intersection::Segment(endpoints))
}
fn unit_normal(geometry: &Geometry) -> V3 {
    let normal = geometry.normal();
    mul(normal, 1. / norm(normal))
}
fn relative_point(delta: V3, u: V3, v: V3, pixel: [f64; 2]) -> V3 {
    add(delta, add(mul(u, pixel[0]), mul(v, pixel[1])))
}
fn point(origin: [f64; 2], direction: [f64; 2], t: f64) -> [f64; 2] {
    std::array::from_fn(|axis| origin[axis] + direction[axis] * t)
}
fn other_pixel(geometry: &Geometry, delta: V3) -> Result<[f64; 2], GeometryError> {
    if !finite(delta) {
        return Err(GeometryError::Pixels);
    }
    let (u, v) = geometry.directions();
    let uu = dot(u, u);
    let vv = dot(v, v);
    let uv = dot(u, v);
    let determinant = uu * vv - uv * uv;
    let du = dot(delta, u);
    let dv = dot(delta, v);
    // Validated directions may differ slightly from an orthonormal basis. Solve
    // their Gram system rather than assuming exact unit lengths/orthogonality.
    let result = [
        (vv * du - uv * dv) / determinant / geometry.spacing[1],
        (uu * dv - uv * du) / determinant / geometry.spacing[0],
    ];
    if result.iter().all(|value| value.is_finite()) {
        Ok(result)
    } else {
        Err(GeometryError::Pixels)
    }
}
fn clip(
    origin: [f64; 2],
    direction: [f64; 2],
    size: [u32; 2],
    mut range: [f64; 2],
) -> Option<[f64; 2]> {
    for axis in 0..2 {
        let bounds = [-0.5, f64::from(size[axis]) - 0.5];
        if direction[axis] == 0. {
            if origin[axis] < bounds[0] || origin[axis] > bounds[1] {
                return None;
            }
        } else {
            let mut a = (bounds[0] - origin[axis]) / direction[axis];
            let mut b = (bounds[1] - origin[axis]) / direction[axis];
            if a > b {
                std::mem::swap(&mut a, &mut b);
            }
            range[0] = range[0].max(a);
            range[1] = range[1].min(b);
            if range[0] > range[1] {
                return None;
            }
        }
    }
    (range[0].is_finite() && range[1].is_finite()).then_some(range)
}

#[cfg(test)]
mod tests;
