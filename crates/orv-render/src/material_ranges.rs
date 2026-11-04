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

pub struct GpuMaterialRanges {
    source: Arc<GpuVolumeSource>,
    ranges: Arc<VolumeRanges>,
    material: Arc<VolumeMaterial>,
    layout: wgpu::BindGroupLayout,
    bindings: wgpu::BindGroup,
    transparent_count: usize,
    _reservation: allocation::GpuReservation,
    _metadata: Reservation,
}
impl GpuMaterialRanges {
    /// Cancellable worker preparation. Bounds include rounded trilinear
    /// evaluation, so skipping changes no source value or sample position.
    /// Reclassify after any material change; geometry equality cannot bind sources.
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
        if material.points().iter().any(|p| p.value.is_subnormal())
            || material
                .points()
                .windows(2)
                .any(|p| f64::from(p[1].value) - f64::from(p[0].value) < 1e-20)
        {
            return Err(
                "Material scalar knots are too close or small for GPU range classification",
            );
        }
        let device = source.device();
        let queue = source.queue();
        let counts = ranges.counts().map(|n| n as u32);
        if counts
            .iter()
            .any(|n| *n > device.limits().max_texture_dimension_3d)
        {
            return Err("Scalar range grid exceeds this GPU's texture limit");
        }
        let row = counts[0].div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
            * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let staging_bytes = row as usize * counts[1] as usize * counts[2] as usize;
        let staging_charge = MemoryBudget::volumes()
            .reserve(staging_bytes as u64)
            .ok_or("Not enough memory for material range staging")?;
        let metadata = MemoryBudget::volumes()
            .reserve((std::mem::size_of::<Self>() + 2 * std::mem::size_of::<usize>()) as u64)
            .ok_or("Not enough memory for material range identity")?;
        let reservation = allocation::reserve_bytes(
            ranges.summaries().len() as u64 + staging_bytes as u64 + 16 + 512,
            device,
            queue,
        )?;
        let mut data = Vec::new();
        data.try_reserve_exact(staging_bytes)
            .map_err(|_| "Not enough memory for material range staging")?;
        data.resize(staging_bytes, 0);
        let mut transparent_count = 0;
        for z in 0..counts[2] as usize {
            for y in 0..counts[1] as usize {
                if cancelled() {
                    return Err("Cancelled");
                }
                for x in 0..counts[0] as usize {
                    let index = x + counts[0] as usize * (y + counts[1] as usize * z);
                    let bounds = scalar_interval(ranges.summaries()[index]);
                    let transparent = zero_coefficients(&material, bounds);
                    data[(z * counts[1] as usize + y) * row as usize + x] = u8::from(transparent);
                    transparent_count += usize::from(transparent);
                }
            }
        }
        let texture = allocation::texture(
            device,
            &wgpu::TextureDescriptor {
                label: Some("scalar zero extinction tiles"),
                size: wgpu::Extent3d {
                    width: counts[0],
                    height: counts[1],
                    depth_or_array_layers: counts[2],
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D3,
                format: wgpu::TextureFormat::R8Uint,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            },
        )?;
        let uniform = allocation::initialized_buffer(
            device,
            "scalar range cell side",
            bytemuck::cast_slice(&[ranges.side() as u32, 0, 0, 0]),
            wgpu::BufferUsages::UNIFORM,
        )?;
        let memory = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("scalar range bindings"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Uint,
                        view_dimension: wgpu::TextureViewDimension::D3,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(16),
                    },
                    count: None,
                },
            ],
        });
        let bindings = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("immutable scalar range flags"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(
                        &texture.create_view(&Default::default()),
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: uniform.as_entire_binding(),
                },
            ],
        });
        let invalid = pollster::block_on(validation.pop()).is_some();
        let exhausted = pollster::block_on(memory.pop()).is_some();
        if invalid || exhausted {
            return Err("The graphics device could not prepare material range bindings");
        }
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: Some(counts[1]),
            },
            texture.size(),
        );
        flush_upload(device, queue, &cancelled)?;
        drop(staging_charge);
        if cancelled() {
            return Err("Cancelled");
        }
        Ok(Arc::new(Self {
            source,
            ranges,
            material,
            layout,
            bindings,
            transparent_count,
            _reservation: reservation,
            _metadata: metadata,
        }))
    }
    pub fn source(&self) -> &Arc<GpuVolumeSource> {
        &self.source
    }
    pub fn ranges(&self) -> &Arc<VolumeRanges> {
        &self.ranges
    }
    pub fn material(&self) -> &Arc<VolumeMaterial> {
        &self.material
    }
    pub fn transparent_tiles(&self) -> usize {
        self.transparent_count
    }
    pub(crate) fn layout(&self) -> &wgpu::BindGroupLayout {
        &self.layout
    }
    pub(crate) fn bindings(&self) -> &wgpu::BindGroup {
        &self.bindings
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
