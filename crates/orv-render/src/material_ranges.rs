// SPDX-License-Identifier: GPL-3.0-or-later
//! Immutable source/material-bound tiles with conservative interpolation bounds.
use crate::{
    volume::{allocation, flush_upload},
    volume_source::GpuVolumeSource,
};
use orv_core::{
    resources::{MemoryBudget, Reservation},
    volume_material::VolumeMaterial,
    volume_ranges::{ScalarSummary, VolumeRanges},
};
use std::sync::Arc;

mod flags;
pub(crate) use flags::GpuRangeFlags;

pub struct GpuMaterialRanges {
    source: Arc<GpuVolumeSource>,
    flags: GpuRangeFlags,
    _metadata: Reservation,
}
impl GpuMaterialRanges {
    /// Worker-only classification of the exact dense GPU source and material.
    pub fn new(
        source: Arc<GpuVolumeSource>,
        ranges: Arc<VolumeRanges>,
        material: Arc<VolumeMaterial>,
        cancelled: impl Fn() -> bool,
    ) -> Result<Arc<Self>, &'static str> {
        if cancelled() {
            return Err("Cancelled");
        }
        if !Arc::ptr_eq(source.volume(), ranges.source()) || source.volume().is_color() {
            return Err("Scalar ranges require the exact scalar GPU source");
        }
        let metadata = MemoryBudget::volumes()
            .reserve((std::mem::size_of::<Self>() + 16) as u64)
            .ok_or("Not enough memory for dense range ownership")?;
        let flags = GpuRangeFlags::new(
            source.device(),
            source.queue(),
            ranges,
            material,
            |_| true,
            &cancelled,
        )?;
        Ok(Arc::new(Self {
            source,
            flags,
            _metadata: metadata,
        }))
    }
    pub fn source(&self) -> &Arc<GpuVolumeSource> {
        &self.source
    }
    pub fn ranges(&self) -> &Arc<VolumeRanges> {
        &self.flags.ranges
    }
    pub fn material(&self) -> &Arc<VolumeMaterial> {
        &self.flags.material
    }
    pub fn transparent_tiles(&self) -> usize {
        self.flags.transparent_count
    }
    pub(crate) fn layout(&self) -> &wgpu::BindGroupLayout {
        &self.flags.layout
    }
    pub(crate) fn bindings(&self) -> &wgpu::BindGroup {
        &self.flags.bindings
    }
}

/// The real trilinear result lies in the vertex range. Expanding its eight
/// weighted vertices gives at most 27 signed monomials, absolute sum <=27*M,
/// and fewer than 128 rounded +/* operations in any ordering. gamma_128 uses
/// full-ULP directed rounding, not IEEE nearest. The absolute term covers FTZ.
fn scalar_interval(s: ScalarSummary) -> [f32; 2] {
    // Every arithmetic term is zero for an all-zero stencil, including FTZ and
    // reassociation. This common background needs no artificial error interval.
    if s.minimum == 0. && s.maximum == 0. {
        return [0., 0.];
    }
    let u = 2f64.powi(-23);
    let gamma = 128. * u / (1. - 128. * u);
    let magnitude = f64::from(s.minimum.abs().max(s.maximum.abs()));
    let error = gamma * 27. * magnitude + 4096. * f64::from(f32::MIN_POSITIVE) * (1. + magnitude);
    // An extra outward f32 value also covers rounding in this f64 bound itself.
    [
        ((f64::from(s.minimum) - error) as f32).next_down(),
        ((f64::from(s.maximum) + error) as f32).next_up(),
    ]
}

/// Require zero coefficients on every touched segment, including ties. An
/// isolated zero knot preceded by a nonzero coefficient cannot certify a GPU
/// lerp: permitted division rounding need not yield exactly one at that knot.
fn zero_coefficients(material: &VolumeMaterial, bounds: [f32; 2]) -> bool {
    let points = material.points();
    if bounds[1] <= points[0].value {
        return points[0].extinction_per_mm == 0.;
    }
    if bounds[0] <= points[0].value && points[0].extinction_per_mm != 0. {
        return false;
    }
    if bounds[1] >= points[points.len() - 1].value
        && points[points.len() - 1].extinction_per_mm != 0.
    {
        return false;
    }
    points.windows(2).all(|p| {
        bounds[0] > p[1].value
            || bounds[1] < p[0].value
            || (p[0].extinction_per_mm == 0. && p[1].extinction_per_mm == 0.)
    })
}

#[cfg(test)]
mod tests;
