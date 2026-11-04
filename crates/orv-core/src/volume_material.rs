// SPDX-License-Identifier: GPL-3.0-or-later
//! Derived volume materials and physical gradients, separate from source display mapping.
use crate::{vector::*, volume::Volume};

pub const MAX_MATERIAL_POINTS: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MaterialPoint {
    pub value: f32,
    /// Linear light, not an encoded display code or a source sample.
    pub luminance: f32,
    pub extinction_per_mm: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MaterialSample {
    pub luminance: f64,
    pub extinction_per_mm: f64,
}
impl MaterialSample {
    /// Physical attenuation, independent of the number of integration intervals.
    pub fn opacity(self, distance_mm: f64) -> Option<f64> {
        if !distance_mm.is_finite()
            || distance_mm < 0.
            || !self.extinction_per_mm.is_finite()
            || !(0. ..=10.).contains(&self.extinction_per_mm)
        {
            return None;
        }
        Some(-(-self.extinction_per_mm * distance_mm).exp_m1())
    }
}

/// A bounded, immutable piecewise linear optical material. It is not a saved
/// image scene and cannot change source intensities or ordinary snapshots.
#[derive(Clone, Debug, PartialEq)]
pub struct VolumeMaterial {
    points: Vec<MaterialPoint>,
}
impl VolumeMaterial {
    pub fn new(points: &[MaterialPoint]) -> Result<Self, &'static str> {
        if !(2..=MAX_MATERIAL_POINTS).contains(&points.len())
            || points.iter().any(|p| {
                !p.value.is_finite()
                    || p.value.abs() > 1e20
                    || !p.luminance.is_finite()
                    || !(0. ..=1.).contains(&p.luminance)
                    || !p.extinction_per_mm.is_finite()
                    || !(0. ..=10.).contains(&p.extinction_per_mm)
            })
            || points.windows(2).any(|p| p[0].value >= p[1].value)
        {
            return Err(
                "Material needs 2–64 increasing finite scalar points and valid optical values",
            );
        }
        let mut retained = Vec::new();
        retained
            .try_reserve_exact(points.len())
            .map_err(|_| "Not enough memory for volume material")?;
        retained.extend_from_slice(points);
        Ok(Self { points: retained })
    }
    pub fn points(&self) -> &[MaterialPoint] {
        &self.points
    }
    /// Outside values use the endpoint material. Missing values have no material.
    pub fn sample(&self, value: f32) -> Option<MaterialSample> {
        if !value.is_finite() {
            return None;
        }
        let index = self.points.partition_point(|p| p.value < value);
        let b = self.points[index.min(self.points.len() - 1)];
        let a = self.points[index.saturating_sub(1)];
        let t = if a.value == b.value {
            0.
        } else {
            ((f64::from(value) - f64::from(a.value)) / (f64::from(b.value) - f64::from(a.value)))
                .clamp(0., 1.)
        };
        let mix = |a: f32, b: f32| f64::from(a) * (1. - t) + f64::from(b) * t;
        Some(MaterialSample {
            luminance: mix(a.luminance, b.luminance),
            extinction_per_mm: mix(a.extinction_per_mm, b.extinction_per_mm),
        })
    }
}

/// Worker/reference sampler: central secants clipped to the source domain at
/// its edges. Missing stencil samples produce no gradient, never an invented
/// boundary normal. The inverse transpose preserves regularly sheared grids.
pub fn physical_gradient(volume: &Volume, world: [f64; 3]) -> Option<[f64; 3]> {
    let v = volume.world_to_voxel(world);
    volume.sample_voxel(v)?;
    let maximum = volume.dimensions().map(|n| (n - 1) as f64);
    let v: V3 = std::array::from_fn(|axis| v[axis].clamp(0., maximum[axis]));
    let mut index_gradient = [0.; 3];
    for axis in 0..3 {
        let mut lo = v;
        let mut hi = v;
        lo[axis] = (v[axis] - 1.).max(0.);
        hi[axis] = (v[axis] + 1.).min(maximum[axis]);
        let width = hi[axis] - lo[axis];
        if width <= 0. {
            return None;
        }
        index_gradient[axis] =
            (f64::from(volume.sample_voxel(hi)?) - f64::from(volume.sample_voxel(lo)?)) / width;
    }
    let gradient: V3 = std::array::from_fn(|axis| {
        let mut unit = [0.; 3];
        unit[axis] = 1.;
        dot(index_gradient, volume.world_vector_to_voxel(unit))
    });
    finite(gradient).then_some(gradient)
}

/// Zero/unknown gradients have no normal. Scaling first avoids an arbitrary
/// intensity-unit cutoff and overflow/underflow during normalization.
pub fn gradient_normal(gradient: [f64; 3]) -> Option<[f64; 3]> {
    if !finite(gradient) {
        return None;
    }
    let scale = gradient.into_iter().map(f64::abs).fold(0., f64::max);
    if scale == 0. {
        return None;
    }
    // Component-wise division also handles finite subnormal gradient scales.
    let scaled = gradient.map(|v| v / scale);
    let length = norm(scaled);
    Some(scaled.map(|v| v / length))
}

#[cfg(test)]
mod tests;
