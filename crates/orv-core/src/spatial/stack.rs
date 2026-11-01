// SPDX-License-Identifier: GPL-3.0-or-later
//! Header-only regular-volume geometry shared with materialized volumes.
use super::GeometryError;
use crate::{navigation::SpatialFrame, resources::MemoryBudget, study::Geometry, vector::*};

pub(crate) struct RegularGeometry {
    pub dimensions: [usize; 3],
    pub origin: V3,
    pub basis: [V3; 3],
    pub inverse: [V3; 3],
    pub step: f64,
}

impl RegularGeometry {
    pub fn prepare(
        count: usize,
        frame: impl Fn(usize) -> Option<SpatialFrame>,
        cancelled: impl Fn() -> bool,
    ) -> Result<Self, GeometryError> {
        if cancelled() {
            return Err(GeometryError::Cancelled);
        }
        if count < 2 {
            return Err(GeometryError::TooFew);
        }
        let first = frame(0).ok_or(GeometryError::Missing)?;
        let size = first.size;
        let reference = first.geometry.ok_or(GeometryError::Missing)?;
        reference.validate()?;
        if first.size.iter().any(|size| *size < 2) {
            return Err(GeometryError::Dimensions);
        }
        if reference
            .frame_of_reference
            .as_ref()
            .is_none_or(|id| id.is_empty())
        {
            return Err(GeometryError::Reference);
        }
        let bytes = (count as u64)
            .checked_mul(std::mem::size_of::<(f64, usize)>() as u64)
            .ok_or(GeometryError::Limit)?;
        let _memory = MemoryBudget::volumes()
            .reserve(bytes)
            .ok_or(GeometryError::Limit)?;
        let mut sorted = Vec::with_capacity(count);
        let normal = reference.normal();
        for index in 0..count {
            if cancelled() {
                return Err(GeometryError::Cancelled);
            }
            let item = frame(index).ok_or(GeometryError::Missing)?;
            if item.size != first.size {
                return Err(GeometryError::Dimensions);
            }
            let geometry = item.geometry.ok_or(GeometryError::Missing)?;
            geometry.validate()?;
            compatible(&reference, &geometry)?;
            let location = dot(geometry.position, normal);
            if !location.is_finite() {
                return Err(GeometryError::Missing);
            }
            sorted.push((location, index));
        }
        sorted.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
        if cancelled() {
            return Err(GeometryError::Cancelled);
        }
        let geometry_at = |index| {
            frame(index)
                .and_then(|item| item.geometry)
                .ok_or(GeometryError::Missing)
        };
        let first = geometry_at(sorted[0].1)?;
        let origin = first.position;
        let last = geometry_at(sorted[count - 1].1)?.position;
        let delta = mul(sub(last, origin), 1. / (count - 1) as f64);
        let dz = dot(delta, normal);
        if !dz.is_finite() || dz <= 1e-5 {
            return Err(GeometryError::Irregular);
        }
        for (order, (_, index)) in sorted.into_iter().enumerate() {
            if cancelled() {
                return Err(GeometryError::Cancelled);
            }
            let position = geometry_at(index)?.position;
            if norm(sub(position, add(origin, mul(delta, order as f64)))) > 0.01f64.max(dz * 0.01) {
                return Err(GeometryError::Irregular);
            }
        }
        let (u, v) = first.directions();
        let basis = [mul(u, first.spacing[1]), mul(v, first.spacing[0]), delta];
        let det = dot(basis[0], cross(basis[1], basis[2]));
        if !det.is_finite() || det.abs() < 1e-12 {
            return Err(GeometryError::Irregular);
        }
        let inverse = [
            mul(cross(basis[1], basis[2]), 1. / det),
            mul(cross(basis[2], basis[0]), 1. / det),
            mul(cross(basis[0], basis[1]), 1. / det),
        ];
        let dimensions = [size[0] as usize, size[1] as usize, count];
        for x in [0., (dimensions[0] - 1) as f64] {
            for y in [0., (dimensions[1] - 1) as f64] {
                for z in [0., (dimensions[2] - 1) as f64] {
                    if !finite(add(
                        origin,
                        add(mul(basis[0], x), add(mul(basis[1], y), mul(basis[2], z))),
                    )) {
                        return Err(GeometryError::Limit);
                    }
                }
            }
        }
        Ok(Self {
            dimensions,
            origin,
            basis,
            inverse,
            step: first.spacing[0].min(first.spacing[1]).min(dz),
        })
    }
}

fn compatible(reference: &Geometry, geometry: &Geometry) -> Result<(), GeometryError> {
    if geometry.frame_of_reference != reference.frame_of_reference
        || geometry.nifti_transform != reference.nifti_transform
    {
        return Err(GeometryError::Reference);
    }
    if geometry
        .orientation
        .iter()
        .zip(reference.orientation)
        .any(|(a, b)| (a - b).abs() > 1e-4)
    {
        return Err(GeometryError::Orientation);
    }
    if geometry
        .spacing
        .iter()
        .zip(reference.spacing)
        .any(|(a, b)| (a - b).abs() > 1e-5 * b.max(1.))
    {
        return Err(GeometryError::Spacing);
    }
    Ok(())
}
