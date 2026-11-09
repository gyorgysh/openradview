// SPDX-License-Identifier: GPL-3.0-or-later
use super::{Result, fence, measure, opaque, verify_mask};
use orv_core::{
    ViewState,
    resources::MemoryBudget,
    segments::{FractionalEncoding, FractionalKind, Segments},
};
use orv_render::{
    mask_input::MaskInput,
    mask_projection::{GpuMaskProjector, MaskProjection},
    segment_source::{GpuSegment, UNKNOWN_PLANE},
    volume_source::GpuVolumeSource,
};
use serde_json::{Value, json};
use std::{
    hint::black_box,
    sync::{Arc, mpsc},
    time::Duration,
};
fn fixture(source: &Arc<GpuVolumeSource>, fractional: bool, sparse: bool) -> Result<Arc<Segments>> {
    let mut frames = (0..128)
        .filter(|z| !sparse || [32, 64].contains(z))
        .map(|z| (65535, z))
        .collect::<Vec<_>>();
    frames.push((1, 32)); // Independent overlapping membership, retained on CPU.
    let pixels = frames.len() * 512 * 512;
    let bytes = if fractional {
        pixels
    } else {
        pixels.div_ceil(8).next_multiple_of(2)
    };
    let _input = MemoryBudget::volumes()
        .reserve(bytes as u64)
        .ok_or("Supplied fixture input admission refused")?;
    let payload = vec![if fractional { 100 } else { 255 }; bytes];
    Ok(Arc::new(if fractional {
        Segments::from_fractional(
            source.volume().clone(),
            &[65535, 1, 42],
            &frames,
            &payload,
            FractionalEncoding {
                kind: if sparse {
                    FractionalKind::Occupancy
                } else {
                    FractionalKind::Probability
                },
                maximum: 200,
            },
            Some(true),
            || false,
        )?
    } else {
        Segments::from_binary(
            source.volume().clone(),
            &[65535, 1, 42],
            &frames,
            &payload,
            Some(true),
            || false,
        )?
    }))
}
fn words(
    source: &GpuVolumeSource,
    texture: &wgpu::Texture,
    mut check: impl FnMut(usize, u32) -> Result<()>,
) -> Result<usize> {
    let device = source.device();
    let queue = source.queue();
    let size = texture.size();
    let stride = (size.width * 4).div_ceil(256) * 256;
    let bytes = u64::from(stride) * u64::from(size.height) * u64::from(size.depth_or_array_layers);
    let _gpu = orv_render::GpuReservation::new(bytes, device, queue)?;
    let _cpu = MemoryBudget::volumes()
        .reserve(bytes)
        .ok_or("Supplied readback CPU admission refused")?;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("generated supplied word verification"),
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
    let mut count = 0;
    for row in data.chunks_exact(stride as usize) {
        for v in row[..(size.width * 4) as usize].as_chunks::<4>().0 {
            check(count, u32::from_le_bytes(*v))?;
            count += 1;
        }
    }
    drop(data);
    buffer.unmap();
    Ok(count)
}
pub fn run(
    source: Arc<GpuVolumeSource>,
    projector: &mut GpuMaskProjector,
    before_gpu: u64,
) -> Result<Vec<Value>> {
    let device = source.device();
    let queue = source.queue();
    let mut cases = Vec::new();
    let cpu = MemoryBudget::volumes();
    let gpu = MemoryBudget::gpu();
    for fractional in [false, true] {
        for sparse in [true, false] {
            fence(device)?;
            let cpu_start = cpu.used();
            let gpu_start = gpu.used();
            let map = fixture(&source, fractional, sparse)?;
            let mut maximum = 0;
            let upload = measure(
                |_| {
                    let (uploaded, _) =
                        GpuSegment::upload(source.clone(), map.clone(), 65535, None, || false)?;
                    maximum = maximum.max(gpu.used() - before_gpu);
                    black_box(uploaded);
                    Ok(())
                },
                12,
            )?;
            let (uploaded, stats) =
                GpuSegment::upload(source.clone(), map.clone(), 65535, None, || false)?;
            if stats.supplied_planes != if sparse { 2 } else { 128 } {
                return Err("Wrong supplied upload workload".into());
            }
            let reuse = measure(
                |_| {
                    let (same, stats) = GpuSegment::upload(
                        source.clone(),
                        map.clone(),
                        65535,
                        Some(uploaded.clone()),
                        || false,
                    )?;
                    if !Arc::ptr_eq(&same, &uploaded)
                        || !stats.reused
                        || stats.uploaded_sample_bytes != 0
                        || stats.uploaded_address_bytes != 0
                    {
                        return Err("Supplied reuse performed work".into());
                    }
                    black_box(same);
                    Ok(())
                },
                120,
            )?;
            let address_count = words(&source, uploaded.address_texture(), |z, v| {
                let expected = if !sparse {
                    z as u32
                } else if z == 32 {
                    0
                } else if z == 64 {
                    1
                } else {
                    UNKNOWN_PLANE
                };
                if v != expected {
                    return Err("Wrong supplied source-plane address".into());
                }
                Ok(())
            })?;
            let sample_words = words(&source, uploaded.packed_texture(), |_, v| {
                if v != if fractional {
                    u32::from_le_bytes([100; 4])
                } else {
                    u32::MAX
                } {
                    return Err("Original supplied GPU samples changed".into());
                }
                Ok(())
            })?;
            let mut views = Vec::new();
            for side in [512, 2048] {
                let mut camera = orv_core::volume_view::Camera::fit(source.volume());
                camera.yaw_degrees = 31.;
                camera.pitch_degrees = 23.;
                camera.roll_degrees = 11.;
                let request = MaskProjection {
                    input: MaskInput::Supplied(uploaded.clone()),
                    number: 65535,
                    camera,
                    display: ViewState::default(),
                    clip: None,
                    size: [side, side],
                };
                let projection = measure(
                    |_| {
                        let mut encoder = device.create_command_encoder(&Default::default());
                        let image = projector
                            .encode_rows(&mut encoder, &request, 0, 0..side)?
                            .ok_or("Incomplete supplied projection")?;
                        queue.submit([encoder.finish()]);
                        fence(device)?;
                        maximum = maximum.max(gpu.used() - before_gpu);
                        black_box(image);
                        Ok(())
                    },
                    if side == 512 { 120 } else { 4 },
                )?;
                let mut encoder = device.create_command_encoder(&Default::default());
                let image = projector
                    .encode_rows(&mut encoder, &request, 0, 0..side)?
                    .ok_or("Incomplete supplied verification")?;
                queue.submit([encoder.finish()]);
                fence(device)?;
                let hits = verify_mask(device, queue, &image)?;
                if hits == 0 {
                    return Err("Supplied benchmark did not render a region".into());
                }
                let composition = opaque::run(projector, image, before_gpu)?;
                views.push(json!({"size":[side,side],"mask_projection":projection,"verified_eligible_hit_pixels":hits,"opaque_composition":composition}));
                drop(request);
                fence(device)?;
            }
            cases.push(json!({"encoding":if fractional {"fractional"}else{"binary"},"coverage":if sparse {"two_planes"}else{"full"},"selected_id":65535,
            "supplied_planes":stats.supplied_planes,"packed_texture_size":[uploaded.packed_texture().width(),uploaded.packed_texture().height(),uploaded.packed_texture().depth_or_array_layers()],
            "stored_sample_bytes":stats.stored_sample_bytes,"uploaded_sample_bytes":stats.uploaded_sample_bytes,"uploaded_address_bytes":stats.uploaded_address_bytes,
            "complete_upload":upload,"reuse":reuse,"verified_sample_words":sample_words,"verified_source_plane_addresses":address_count,"views":views,
            "maximum_observed_upload_and_projection_admission_with_source":maximum,
            "scope":"Immutable supplied-ID packing and upload includes CPU raw-row reads, word packing, fresh textures, padded staging and final fence; CPU source/mask construction excluded. Reuse checks source/object/revision/ID and uploads no bytes. Projection includes fresh full cell/depth target and fence; optical timings have their own scope. Untimed readbacks verify every source-plane address and packed word, every reported mask hit and optical output/sample references. No native input/display, file decode, process RSS or driver residency."}));
            drop(uploaded);
            drop(map);
            fence(device)?;
            if cpu.used() != cpu_start || gpu.used() != gpu_start {
                return Err("Supplied benchmark reservations did not return to baseline".into());
            }
        }
    }
    Ok(cases)
}
