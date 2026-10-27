// SPDX-License-Identifier: GPL-3.0-or-later
//! Shared source-bound grayscale parameters and bounded lookup textures.
use orv_core::{VoiFunction, WindowLevel, scalar::ScalarData, study::Frame};

pub(crate) const ATLAS_BYTES: u64 = 16 * 65536 * 4;
pub(crate) fn parameters(frame: &Frame, window: WindowLevel) -> Result<[f32; 4], &'static str> {
    if !frame.supports_window(window) {
        return Err("Display mapping is unavailable for this source");
    }
    // WGSL may flush subnormal arithmetic to zero. Keep that error far below
    // display quantization instead of accepting a vanishing denominator.
    if window.width < 1e-20 {
        return Err("Display width is below GPU arithmetic limits; choose a wider contrast range");
    }
    let kind = match window.function {
        VoiFunction::Linear => 0.,
        VoiFunction::LinearExact => 1.,
        VoiFunction::Sigmoid => 2.,
        VoiFunction::Lookup { .. } => 3.,
    };
    let VoiFunction::Lookup { index } = window.function else {
        return Ok([kind, 0., 0., 0.]);
    };
    let table = frame
        .scalar
        .as_ref()
        .and_then(|data| data.choices().luts().get(usize::from(index)))
        .ok_or("The source has no selected VOI table")?;
    Ok([
        kind,
        table.first() as f32,
        table.entries().len() as f32,
        f32::from(index),
    ])
}
pub(crate) fn signature(data: Option<&ScalarData>) -> Vec<[u8; 32]> {
    data.map(|data| {
        data.choices()
            .luts()
            .iter()
            .map(|lut| lut.digest())
            .collect()
    })
    .unwrap_or_default()
}
pub(crate) fn layout(binding: u32, visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: false },
            view_dimension: wgpu::TextureViewDimension::D2Array,
            multisampled: false,
        },
        count: None,
    }
}
pub(crate) fn upload(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    data: Option<&ScalarData>,
    cancelled: &impl Fn() -> bool,
) -> Result<wgpu::TextureView, &'static str> {
    let tables = data.map(|data| data.choices().luts()).unwrap_or_default();
    // The pinned OpenGL backend infers 2D/cube targets from the allocation,
    // without reinterpreting them for an array view. Keep a genuine array for
    // empty/single tables and avoid square six-layer cube-compatible extents.
    // Padding stays within the existing sixteen-table admission allowance.
    let mut layers = tables.len().max(2) as u32;
    if layers.is_multiple_of(6) {
        layers += 1;
    }
    let side = if tables.is_empty() { 1 } else { 256 };
    if side > device.limits().max_texture_dimension_2d
        || layers > device.limits().max_texture_array_layers
    {
        return Err("Source VOI tables exceed this GPU's texture limits");
    }
    let texture = crate::volume::allocation::texture(
        device,
        &wgpu::TextureDescriptor {
            label: Some("source grayscale tables"),
            size: wgpu::Extent3d {
                width: side,
                height: side,
                depth_or_array_layers: layers,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        },
    )?;
    for layer in 0..layers {
        if cancelled() {
            return Err("Cancelled");
        }
        let mut values = vec![0f32; (side * side) as usize];
        if let Some(table) = tables.get(layer as usize) {
            let maximum = if table.bits() == 8 { 255. } else { 65535. };
            for (value, entry) in values.iter_mut().zip(table.entries()) {
                *value = f32::from(*entry) / maximum;
            }
        }
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: 0,
                    y: 0,
                    z: layer,
                },
                aspect: wgpu::TextureAspect::All,
            },
            bytemuck::cast_slice(&values),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(side * 4),
                rows_per_image: Some(side),
            },
            wgpu::Extent3d {
                width: side,
                height: side,
                depth_or_array_layers: 1,
            },
        );
    }
    Ok(texture.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        ..Default::default()
    }))
}
