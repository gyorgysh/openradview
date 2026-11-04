// SPDX-License-Identifier: GPL-3.0-or-later
use super::{Result, fence, measure};
use orv_core::{
    mask::MaskView,
    resources::MemoryBudget,
    volume_material::{MaterialPoint, VolumeMaterial},
    volume_view::{Projection, Quality, VolumeView},
};
use orv_render::{
    mask_projection::{GpuMaskProjector, MaskImage},
    material_volume::Lighting,
    opaque_material::{OpaqueMaterialImage, OpaqueMaterialRenderer, OpaqueMaterialScene},
};
use serde_json::{Value, json};
use std::{
    hint::black_box,
    sync::{Arc, mpsc},
    time::Duration,
};

pub fn run(
    projector: &mut GpuMaskProjector,
    mask: Arc<MaskImage>,
    before_gpu: u64,
) -> Result<Value> {
    let request = mask.request().clone();
    let gpu = request.labels.source();
    let device = gpu.device();
    let queue = gpu.queue();
    let mut renderer = OpaqueMaterialRenderer::new(gpu.clone(), wgpu::TextureFormat::Rgba8Unorm)?;
    let mut volume = VolumeView::fit(gpu.volume());
    volume.camera = request.camera.clone();
    volume.clip = request.clip.clone();
    volume.projection = Projection::Composite;
    volume.quality = if request.size[0] == 512 {
        Quality::Interactive
    } else {
        Quality::Refined
    };
    // Homogeneous source gives an independent closed-form optical reference.
    let material = Arc::new(VolumeMaterial::new(&[-1000., 1000.].map(|value| {
        MaterialPoint {
            value,
            luminance: 0.7,
            extinction_per_mm: 0.015,
        }
    }))?);
    let scene = OpaqueMaterialScene {
        mask,
        navigation: request.display.clone(),
        volume,
        material,
        lighting: Lighting::HEADLIGHT,
        mask_luminance: 0.8,
    };
    let side = request.size[0];
    let mut maximum = 0;
    let mut observe = || {
        maximum = maximum.max(MemoryBudget::gpu().used() - before_gpu);
    };
    let cached = measure(
        |_| {
            let image = renderer
                .render_rows(&scene, 0, 0..side, || false)?
                .ok_or("Incomplete opaque image")?;
            observe();
            black_box(image);
            Ok(())
        },
        if side == 512 { 120 } else { 4 },
    )?;
    let rows = measure(
        |_| {
            let mut complete = None;
            for row in (0..side).step_by(16) {
                let image = renderer.render_rows(&scene, 1, row..(row + 16).min(side), || false)?;
                if image.is_some() {
                    complete = image;
                }
            }
            black_box(complete.ok_or("Incomplete opaque row sequence")?);
            observe();
            Ok(())
        },
        if side == 512 { 30 } else { 4 },
    )?;
    let combined = measure(
        |_| {
            let mut encoder = device.create_command_encoder(&Default::default());
            let mask = projector
                .encode_rows(&mut encoder, &request, 0, 0..side)?
                .ok_or("Incomplete combined mask image")?;
            queue.submit([encoder.finish()]);
            let mut updated = scene.clone();
            updated.mask = mask;
            let image = renderer
                .render_rows(&updated, 0, 0..side, || false)?
                .ok_or("Incomplete combined radiance")?;
            observe();
            black_box(image);
            Ok(())
        },
        if side == 512 { 120 } else { 4 },
    )?;
    let verification = renderer
        .render_rows(&scene, 0, 0..side, || false)?
        .ok_or("Incomplete verification radiance")?;
    let sampled = verify(&verification)?;
    Ok(
        json!({"complete_composition_cached_mask":cached,"complete_composition_cached_mask_16_row_submissions":rows,
        "complete_projection_and_composition":combined,"maximum_observed_admitted_gpu_bytes_with_source":maximum,
        "source_luminance":0.7,"source_extinction_per_mm":0.015,"mask_luminance":0.8,"source_lighting":"headlight; generated constant field has no normal",
        "verification":{"finite_monochrome_alpha_one_pixels":side*side,"cpu_analytic_sampled_rays":sampled},
        "scope":"Fresh RGBA16F allocation, physical source integration to an opaque mask and final four-byte GPU status fence. Combined also allocates and computes fresh mask cell/depth images, with one final fence. Cached row sequence submits each 16-row batch and fences only the final status. Source/label upload, display, pixel readback and native input are excluded from timing. Untimed readback checks all pixels and 16x16 sampled rays against f64 cell traversal plus homogeneous Beer-Lambert optics."}),
    )
}
fn half(bits: u16) -> Result<f64> {
    let exponent = (bits >> 10) & 31;
    if bits & 0x8000 != 0 || exponent == 31 {
        return Err("Nonfinite or negative opaque output".into());
    }
    let f = f64::from(bits & 1023);
    Ok(if exponent == 0 {
        f * 2f64.powi(-24)
    } else {
        (1. + f / 1024.) * 2f64.powi(i32::from(exponent) - 15)
    })
}
fn verify(image: &OpaqueMaterialImage) -> Result<usize> {
    let gpu = image.scene().mask.request().labels.source();
    let device = gpu.device();
    let queue = gpu.queue();
    let texture = image.texture();
    let size = texture.size();
    let stride = (size.width * 8).div_ceil(256) * 256;
    let bytes = u64::from(stride) * u64::from(size.height);
    let _gpu = orv_render::GpuReservation::new(bytes, device, queue)?;
    let _cpu = MemoryBudget::volumes()
        .reserve(bytes)
        .ok_or("Opaque readback CPU admission refused")?;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("generated opaque verification"),
        size: bytes,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(stride),
                rows_per_image: Some(size.height),
            },
        },
        size,
    );
    queue.submit([encoder.finish()]);
    let (tx, rx) = mpsc::sync_channel(1);
    buffer.map_async(wgpu::MapMode::Read, .., move |r| {
        let _ = tx.send(r);
    });
    fence(device)?;
    rx.recv_timeout(Duration::from_secs(10))??;
    let data = buffer.get_mapped_range(..)?;
    for row in data.chunks_exact(stride as usize) {
        for pixel in row[..(size.width * 8) as usize].as_chunks::<8>().0 {
            half(u16::from_le_bytes([pixel[0], pixel[1]]))?;
            if pixel[..2] != pixel[2..4] || pixel[2..4] != pixel[4..6] || pixel[6..] != [0, 0x3c] {
                return Err("Opaque output is not monochrome with alpha one".into());
            }
        }
    }
    let scene = image.scene();
    let source = gpu.volume();
    let camera = scene.volume.camera.frame(
        &scene.navigation,
        f64::from(size.width) / f64::from(size.height),
    )?;
    let mask = MaskView::manual(
        scene.mask.request().labels.labels(),
        scene.mask.request().number,
    )?;
    let mut checked = 0;
    for gy in 0..16u32 {
        for gx in 0..16u32 {
            let x = (gx * size.width + size.width / 2) / 16;
            let y = (gy * size.height + size.height / 2) / 16;
            let (origin, direction) = camera.ray([
                (f64::from(x) + 0.5) / f64::from(size.width),
                (f64::from(y) + 0.5) / f64::from(size.height),
            ]);
            let hit = mask
                .ray(origin, direction, [-1e30, 1e30], None)?
                .first_eligible_hit(|| false)?;
            let local = source.world_to_voxel(origin);
            let delta = source.world_vector_to_voxel(direction);
            let mut near = f64::NEG_INFINITY;
            let mut far = f64::INFINITY;
            for a in 0..3 {
                let maximum = (source.dimensions()[a] - 1) as f64;
                if delta[a] == 0. {
                    if local[a] < 0. || local[a] > maximum {
                        near = f64::INFINITY;
                    }
                } else {
                    let p = -local[a] / delta[a];
                    let q = (maximum - local[a]) / delta[a];
                    near = near.max(p.min(q));
                    far = far.min(p.max(q));
                }
            }
            let distance =
                (far.min(hit.as_ref().map_or(f64::INFINITY, |h| h.range_mm[0])) - near).max(0.);
            let t = (-0.015 * distance).exp();
            let expected = 0.7 * (1. - t) + t * if hit.is_some() { 0.8 } else { 0.0030352698 };
            let index = (y * stride + x * 8) as usize;
            let actual = half(u16::from_le_bytes([data[index], data[index + 1]]))?;
            if (actual - expected).abs() > 0.001 {
                return Err(
                    format!("Opaque analytic mismatch at {x}/{y}: {actual} vs {expected}").into(),
                );
            }
            checked += 1;
        }
    }
    drop(data);
    buffer.unmap();
    Ok(checked)
}
