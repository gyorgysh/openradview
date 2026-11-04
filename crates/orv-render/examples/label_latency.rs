// SPDX-License-Identifier: GPL-3.0-or-later
//! Generated CPU-to-complete-GPU label revision timings; no input files or networking.
use orv_core::{
    ViewState,
    labels::Labels,
    resources::MemoryBudget,
    study::{Geometry, demo_study},
    volume::Volume,
    volume_view::Camera,
};
use orv_render::{
    label_source::{GpuLabels, UploadKind},
    mask_input::MaskInput,
    mask_projection::{GpuMaskProjector, MaskImage, MaskProjection, RAY_ERROR},
    volume_source::GpuVolumeSource,
};
use serde_json::{Value, json};
use std::{
    error::Error,
    hint::black_box,
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};
#[path = "label_latency/opaque.rs"]
mod opaque;
#[path = "label_latency/supplied.rs"]
mod supplied;
type Result<T> = std::result::Result<T, Box<dyn Error>>;
fn source() -> Result<Arc<Volume>> {
    let template = demo_study().series.remove(0).frames.remove(0);
    let plane: Arc<[f32]> = vec![0.; 512 * 512].into();
    let frames = (0..128)
        .map(|z| {
            let mut frame = (*template).clone();
            frame.width = 512;
            frame.height = 512;
            frame.pixels = plane.clone();
            frame.geometry = Some(Geometry {
                position: [0.05 * f64::from(z), 0., 1.2 * f64::from(z)],
                orientation: [1., 0., 0., 0., 1., 0.],
                spacing: [0.7, 0.7],
                frame_of_reference: Some("generated GPU label benchmark".into()),
                nifti_transform: None,
            });
            Arc::new(frame)
        })
        .collect();
    Ok(Arc::new(Volume::new(frames)?))
}
fn measure(mut run: impl FnMut(usize) -> Result<()>, samples: usize) -> Result<Value> {
    for i in 0..4 {
        run(i)?;
    }
    let mut times = Vec::new();
    for i in 0..samples {
        let started = Instant::now();
        run(i + 4)?;
        times.push(started.elapsed().as_secs_f64() * 1000.);
    }
    times.sort_by(f64::total_cmp);
    Ok(
        json!({"samples":samples,"median_ms":times[samples/2],"p95_ms":times[(samples*95).div_ceil(100)-1],"max_ms":times[samples-1]}),
    )
}
fn fence(device: &wgpu::Device) -> Result<()> {
    device.poll(wgpu::PollType::Wait {
        submission_index: None,
        timeout: Some(Duration::from_secs(30)),
    })?;
    Ok(())
}
fn verify_mask(device: &wgpu::Device, queue: &wgpu::Queue, image: &MaskImage) -> Result<usize> {
    let texture = image.cells();
    let size = texture.size();
    let stride = (size.width * 16).div_ceil(256) * 256;
    let bytes = u64::from(stride) * u64::from(size.height);
    let _gpu = orv_render::GpuReservation::new(bytes, device, queue)?;
    let _cpu = MemoryBudget::volumes()
        .reserve(bytes)
        .ok_or("Readback exceeds CPU admission")?;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("generated mask status verification"),
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
    buffer.map_async(wgpu::MapMode::Read, .., move |result| {
        let _ = tx.send(result);
    });
    fence(device)?;
    rx.recv_timeout(Duration::from_secs(10))??;
    let data = buffer.get_mapped_range(..)?;
    let mut hits = 0;
    let mask = image.request().input.view(image.request().number)?;
    for row in data.chunks_exact(stride as usize) {
        for pixel in row[..size.width as usize * 16].as_chunks::<16>().0 {
            let cell: [u32; 4] = std::array::from_fn(|i| {
                u32::from_le_bytes([
                    pixel[i * 4],
                    pixel[i * 4 + 1],
                    pixel[i * 4 + 2],
                    pixel[i * 4 + 3],
                ])
            });
            if cell[3] == RAY_ERROR {
                return Err("Generated mask ray reported an arithmetic refusal".into());
            }
            if cell[3] != 0 {
                if cell[3] != u32::from(image.request().number)
                    || !mask
                        .sample([cell[0] as usize, cell[1] as usize, cell[2] as usize])
                        .is_some_and(|v| v.eligible())
                {
                    return Err("Generated mask hit is not an eligible source cell".into());
                }
                hits += 1;
            }
        }
    }
    drop(data);
    buffer.unmap();
    Ok(hits)
}
fn main() -> Result<()> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: orv_render::configured_backends()?,
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapter = pollster::block_on(instance.request_adapter(&Default::default()))?;
    let info = adapter.get_info();
    let (device, queue) =
        pollster::block_on(adapter.request_device(&orv_render::device_descriptor(&adapter)))?;
    let gpu_budget = MemoryBudget::gpu();
    let cpu_budget = MemoryBudget::volumes();
    let before_gpu = gpu_budget.used();
    let before_cpu = cpu_budget.used();
    let volume = source()?;
    let source_gpu = GpuVolumeSource::new(&device, &queue, volume.clone(), || false)?;
    let blank = Arc::new(Labels::empty(volume)?);
    let full = measure(
        |_| {
            let (upload, stats) =
                GpuLabels::upload(source_gpu.clone(), blank.clone(), None, || false)?;
            if stats.kind != UploadKind::Full || stats.uploaded_bytes != 512 * 512 * 128 {
                return Err("Wrong complete upload workload".into());
            }
            black_box(upload);
            Ok(())
        },
        12,
    )?;
    fence(&device)?;
    let mut cases = Vec::new();
    for (name, point, tiles) in [("one_tile", [8., 8.], 1), ("four_tiles", [31., 31.], 4)] {
        let (mut current, _) =
            GpuLabels::upload(source_gpu.clone(), blank.clone(), None, || false)?;
        let mut maximum_admitted = 0;
        let mut last = None;
        let timing = measure(
            |i| {
                let mut labels = current.labels().try_clone(|| false)?;
                labels.paint(32, &[point], 3, 1 + (i % 2) as u8, || false)?;
                let (next, stats) = GpuLabels::upload(
                    source_gpu.clone(),
                    Arc::new(labels),
                    Some(current.clone()),
                    || false,
                )?;
                if stats.kind != UploadKind::Delta || stats.uploaded_tiles != tiles {
                    return Err("Wrong dirty tile workload".into());
                }
                maximum_admitted = maximum_admitted.max(gpu_budget.used() - before_gpu);
                last = Some(stats);
                current = next;
                Ok(())
            },
            120,
        )?;
        let stats = last.ok_or("Missing upload result")?;
        let reused = measure(
            |_| {
                let (next, stats) = GpuLabels::upload(
                    source_gpu.clone(),
                    current.labels().clone(),
                    Some(current.clone()),
                    || false,
                )?;
                if stats.kind != UploadKind::Reuse {
                    return Err("Wrong reuse workload".into());
                }
                black_box(next);
                Ok(())
            },
            120,
        )?;
        cases.push(json!({"kind":name,"point":point,"radius_pixels":3,"tile_side":32,"build_and_fence":timing,"unchanged_revision":reused,"uploaded_tiles":stats.uploaded_tiles,"uploaded_bytes":stats.uploaded_bytes,"copied_bytes":stats.copied_bytes,"maximum_observed_admitted_gpu_bytes_with_source":maximum_admitted}));
        drop(current);
        fence(&device)?;
    }
    let mut projector = GpuMaskProjector::new(source_gpu.clone())?;
    let mut projections = Vec::new();
    for kind in ["empty", "one_cell_plane", "filled"] {
        let mut map = blank.try_clone(|| false)?;
        if kind == "one_cell_plane" {
            map.threshold(Some(64), [-1., 1.], 9, || false)?;
        }
        if kind == "filled" {
            map.threshold(None, [-1., 1.], 9, || false)?;
        }
        let (uploaded, _) = GpuLabels::upload(source_gpu.clone(), Arc::new(map), None, || false)?;
        let mut camera = Camera::fit(uploaded.labels().source());
        camera.yaw_degrees = 31.;
        camera.pitch_degrees = 23.;
        camera.roll_degrees = 11.;
        for side in [512, 2048] {
            let request = MaskProjection {
                input: MaskInput::Manual(uploaded.clone()),
                number: 9,
                camera: camera.clone(),
                display: ViewState::default(),
                clip: None,
                size: [side, side],
            };
            let mut maximum_admitted = 0;
            let full = measure(
                |_| {
                    let mut encoder = device.create_command_encoder(&Default::default());
                    let image = projector
                        .encode_rows(&mut encoder, &request, 0, 0..request.size[1])?
                        .ok_or("Incomplete full projection")?;
                    queue.submit([encoder.finish()]);
                    fence(&device)?;
                    maximum_admitted = maximum_admitted.max(gpu_budget.used() - before_gpu);
                    black_box(image);
                    Ok(())
                },
                if side == 512 { 120 } else { 4 },
            )?;
            let tiled = measure(
                |_| {
                    let mut complete = None;
                    for row in (0..request.size[1]).step_by(16) {
                        let mut encoder = device.create_command_encoder(&Default::default());
                        let image = projector.encode_rows(
                            &mut encoder,
                            &request,
                            1,
                            row..(row + 16).min(request.size[1]),
                        )?;
                        queue.submit([encoder.finish()]);
                        fence(&device)?;
                        if image.is_some() {
                            complete = image;
                        }
                    }
                    black_box(complete.ok_or("Incomplete row sequence")?);
                    Ok(())
                },
                if side == 512 { 30 } else { 4 },
            )?;
            let mut encoder = device.create_command_encoder(&Default::default());
            let verification = projector
                .encode_rows(&mut encoder, &request, 0, 0..side)?
                .ok_or("Incomplete verification image")?;
            queue.submit([encoder.finish()]);
            fence(&device)?;
            let verified_hits = verify_mask(&device, &queue, &verification)?;
            if (kind == "empty") != (verified_hits == 0) {
                return Err("Wrong generated mask projection coverage".into());
            }
            let composition = opaque::run(&mut projector, verification.clone(), before_gpu)?;
            drop(verification);
            fence(&device)?;
            projections.push(json!({"mask":kind,"verified_eligible_hit_pixels":verified_hits,"verification_scope":"Separate untimed full cell/ID readback: no arithmetic refusal, every reported hit has known membership and valid source. Not a full CPU ray comparison.","opaque_composition":composition,"size":request.size,"yaw_degrees":request.camera.yaw_degrees,"pitch_degrees":request.camera.pitch_degrees,"roll_degrees":request.camera.roll_degrees,"complete_image":full,"complete_fenced_16_row_sequence":tiled,"maximum_observed_admitted_gpu_bytes_with_source":maximum_admitted,
            "scope":"Mask-only first eligible cell and signed physical depth, target allocation/encoding and GPU fence. No source optical integration, display, picking readback or UI. Row sequence fences every 16-row batch; full image fences once."}));
            drop(request);
            fence(&device)?;
        }
        fence(&device)?;
    }
    let supplied = supplied::run(source_gpu.clone(), &mut projector, before_gpu)?;
    drop(projector);
    drop(blank);
    drop(source_gpu);
    fence(&device)?;
    if gpu_budget.used() != before_gpu || cpu_budget.used() != before_cpu {
        return Err("Label/source reservations did not return to baseline".into());
    }
    println!(
        "{}",
        serde_json::to_string_pretty(
            &json!({"schema_version":4,"version":env!("CARGO_PKG_VERSION"),"optimized":!cfg!(debug_assertions),"adapter":info.name,"backend":format!("{:?}",info.backend),"source_dimensions":[512,512,128],"source_fixture":"Source CPU frames share one constant plane; GPU source is a full uploaded 512x512x128 scalar texture. No decode or process RSS measurement.","scope":"Label edit cases include worker CPU checkpoint/paint, target allocation, prior-grid GPU copy, changed tile uploads and fence. Mask projection cases have their own scope. No native input/display or vsync.","admission_scope":"App-owned GPU source/label/staging reservations; not measured driver residency or free VRAM. Maximum observed admission includes retired versions not yet released.","warmups_per_case":4,"full_empty_map_upload":full,"edits":cases,"mask_projections":projections,"supplied_masks":supplied,"cpu_and_gpu_admission_restored":true})
        )?
    );
    Ok(())
}
