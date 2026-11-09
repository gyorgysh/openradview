// SPDX-License-Identifier: GPL-3.0-or-later
//! Explicit synthetic capacity check. No files, study input, persistence or network.
use orv_core::{
    Plane, ViewState, WindowLevel,
    study::{Frame, Geometry},
    volume::Volume,
    volume_view::VolumeView,
};
use std::{
    error::Error,
    sync::Arc,
    time::{Duration, Instant},
};

fn main() -> Result<(), Box<dyn Error>> {
    let mut cpu_only = false;
    let mut depth = 683usize;
    let mut repetitions = 3usize;
    let mut size = [1024usize, 768];
    for argument in std::env::args().skip(1) {
        if argument == "--cpu-only" {
            cpu_only = true;
        } else if let Some(value) = argument.strip_prefix("--depth=") {
            depth = value.parse()?;
        } else if let Some(value) = argument.strip_prefix("--repetitions=") {
            repetitions = value.parse()?;
        } else if let Some(value) = argument.strip_prefix("--width=") {
            size[0] = value.parse()?;
        } else if let Some(value) = argument.strip_prefix("--height=") {
            size[1] = value.parse()?;
        } else {
            return Err(
                "Use --cpu-only, --depth=2..2048, --width/--height=2..8192 or --repetitions=1..10"
                    .into(),
            );
        }
    }
    if !(2..=2048).contains(&depth)
        || !(1..=10).contains(&repetitions)
        || size.iter().any(|side| !(2..=8192).contains(side))
    {
        return Err("Capacity parameters exceed the supported range".into());
    }
    for repetition in 0..repetitions {
        let report = run(cpu_only, depth, size)?;
        println!(
            "{}",
            serde_json::json!({"repetition":repetition+1,"result":report})
        );
    }
    Ok(())
}
fn run(
    cpu_only: bool,
    depth: usize,
    [width, height]: [usize; 2],
) -> Result<serde_json::Value, Box<dyn Error>> {
    use orv_core::resources::MemoryBudget;
    let cpu_before = MemoryBudget::volumes().used();
    let gpu_before = MemoryBudget::gpu().used();
    let started = Instant::now();
    let mut frames = Vec::new();
    frames.try_reserve_exact(depth)?;
    for z in 0..depth {
        let mut values = Vec::new();
        values.try_reserve_exact(width * height)?;
        values.extend((0..width * height).map(|index| z as f32 + (index % width) as f32 * 0.001));
        frames.push(Arc::new(Frame {
            memory: None,
            presentation: None,
            calibration: None,
            projection: None,
            sop: None,
            slab: None,
            scalar: None,
            width: width as u32,
            height: height as u32,
            pixels: values.into(),
            validity: None,
            color: None,
            nifti: None,
            rgba: None,
            geometry: Some(Geometry {
                nifti_transform: None,
                frame_of_reference: Some("synthetic-capacity".into()),
                position: [0., 0., z as f64],
                orientation: [1., 0., 0., 0., 1., 0.],
                spacing: [1.; 2],
            }),
            instance: z as i32,
            frame_index: 0,
            monochrome1: false,
            default_window: WindowLevel::BONE,
        }));
    }
    let generated_ms = started.elapsed().as_secs_f64() * 1000.;
    let validate = Instant::now();
    let volume = Volume::prepare(frames, || false)?;
    let cpu_reserved = MemoryBudget::volumes().used();
    let validation_ms = validate.elapsed().as_secs_f64() * 1000.;
    let point = volume.voxel_to_world([
        width as f64 * 0.5 + 0.5,
        height as f64 * 0.5 + 0.25,
        depth as f64 * 0.5 - 0.5,
    ]);
    let expected = depth as f64 * 0.5 - 0.5 + (width as f64 * 0.5 + 0.5) * 0.001;
    if (volume.sample(point).ok_or("Missing analytic sample")? as f64 - expected).abs() > 0.0001 {
        return Err("Capacity sample differs from the analytic reference".into());
    }
    let mut timings = Vec::new();
    for _ in 0..12 {
        let start = Instant::now();
        for plane in [Plane::Axial, Plane::Coronal, Plane::Sagittal] {
            std::hint::black_box(volume.reformat(plane, point, 512, || false)?);
        }
        timings.push(start.elapsed().as_secs_f64() * 1000.);
    }
    timings.sort_by(f64::total_cmp);
    let mut gpu = serde_json::Value::Null;
    if !cpu_only {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            flags: orv_render::instance_flags(),
            backends: orv_render::configured_backends()?,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = pollster::block_on(instance.request_adapter(&Default::default()))?;
        let info = adapter.get_info();
        let (device, queue) =
            pollster::block_on(adapter.request_device(&orv_render::device_descriptor(&adapter)))?;
        let start = Instant::now();
        // A refusal is a capacity result: report it with the CPU measurements.
        let renderer = orv_render::volume::VolumeRenderer::new(
            &device,
            &queue,
            volume.clone(),
            wgpu::TextureFormat::Rgba8Unorm,
            || false,
        );
        gpu = serde_json::json!({"backend":format!("{:?}",info.backend),"adapter":info.name,"max_texture_dimension_3d":device.limits().max_texture_dimension_3d});
        let mut renderer = match renderer {
            Ok(renderer) => renderer,
            Err(reason) => {
                gpu["refused"] = reason.into();
                gpu["bricks"] = bricks(&device, &queue, &volume)?;
                return finish(
                    volume,
                    depth,
                    [width, height],
                    gpu,
                    (generated_ms, validation_ms, &timings, cpu_reserved),
                    (cpu_before, gpu_before),
                );
            }
        };
        let upload_ms = start.elapsed().as_secs_f64() * 1000.;
        let mut encoder = device.create_command_encoder(&Default::default());
        renderer.render(
            &device,
            &mut encoder,
            &ViewState::default(),
            &VolumeView::fit(&volume),
            [64; 2],
        )?;
        let submission = queue.submit([encoder.finish()]);
        device.poll(wgpu::PollType::Wait {
            submission_index: Some(submission),
            timeout: Some(Duration::from_secs(30)),
        })?;
        let retained_gpu_reservation = MemoryBudget::gpu().used();
        drop(renderer);
        device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(Duration::from_secs(30)),
        })?;
        gpu["source_upload_ms"] = upload_ms.into();
        gpu["ray_dispatch_completed"] = true.into();
        gpu["retained_gpu_reservation_bytes"] = retained_gpu_reservation.into();
    }
    finish(
        volume,
        depth,
        [width, height],
        gpu,
        (generated_ms, validation_ms, &timings, cpu_reserved),
        (cpu_before, gpu_before),
    )
}
type Timings<'a> = (f64, f64, &'a [f64], u64);
fn finish(
    volume: Arc<Volume>,
    depth: usize,
    [width, height]: [usize; 2],
    gpu: serde_json::Value,
    (generated_ms, validation_ms, timings, cpu_reserved): Timings<'_>,
    (cpu_before, gpu_before): (u64, u64),
) -> Result<serde_json::Value, Box<dyn Error>> {
    use orv_core::resources::MemoryBudget;
    let report = serde_json::json!({
        "schema_version":1,"version":env!("CARGO_PKG_VERSION"),"optimized":!cfg!(debug_assertions),
        "scope":"Generated f32 volume admission, analytic sample, CPU MPR and optional real GPU upload/dispatch; no source-file decode or UI latency",
        "dimensions":volume.dimensions(),"retained_source_bytes":(width*height*depth) as u64*4,
        "generated_ms":generated_ms,"validation_ms":validation_ms,
        "three_plane_mpr_median_ms":timings[6],"three_plane_mpr_p95_ms":timings[11],"gpu":gpu,
        "cpu_reserved_bytes":cpu_reserved,
    });
    let weak = Arc::downgrade(&volume);
    drop(volume);
    if weak.upgrade().is_some()
        || MemoryBudget::volumes().used() != cpu_before
        || MemoryBudget::gpu().used() != gpu_before
    {
        return Err(
            "Capacity check retained source references or CPU/GPU reservations after closure"
                .into(),
        );
    }
    Ok(report)
}

/// Prototype residency check: upload the volume as admitted bricks within the
/// device's 3D texture side, one at a time. The dense CPU volume stays resident.
fn bricks(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    volume: &Volume,
) -> Result<serde_json::Value, Box<dyn Error>> {
    use orv_core::bricks::{BrickLayout, extract};
    let side = device.limits().max_texture_dimension_3d as usize;
    let layout = BrickLayout::plan(volume.dimensions(), side).map_err(|e| format!("{e:?}"))?;
    let started = Instant::now();
    let mut textures = Vec::new();
    let mut extract_ms = 0.;
    for index in 0..layout.len() {
        let brick = layout.brick(index).ok_or("Missing brick")?;
        let extracting = Instant::now();
        let (data, reservation) =
            extract(volume, brick, &|| false).map_err(|e| format!("{e:?}"))?;
        extract_ms += extracting.elapsed().as_secs_f64() * 1000.;
        let size = wgpu::Extent3d {
            width: brick.size[0] as u32,
            height: brick.size[1] as u32,
            depth_or_array_layers: brick.size[2] as u32,
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("capacity brick"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D3,
            format: wgpu::TextureFormat::R32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            bytemuck::cast_slice(&data.values),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(size.width * 4),
                rows_per_image: Some(size.height),
            },
            size,
        );
        // The CPU-side brick copy is transient; the texture keeps the reservation.
        drop(data);
        textures.push((texture, reservation));
    }
    device.poll(wgpu::PollType::Wait {
        submission_index: None,
        timeout: Some(Duration::from_secs(60)),
    })?;
    let upload_ms = started.elapsed().as_secs_f64() * 1000.;
    let reserved = orv_core::resources::MemoryBudget::gpu().used();
    drop(textures);
    Ok(serde_json::json!({
        "count":layout.len(),"max_side":side,
        "stored_voxels":layout.stored_voxels(),
        "dense_voxels":volume.dimensions().iter().map(|d| *d as u64).product::<u64>(),
        "extract_ms":extract_ms,"upload_total_ms":upload_ms,
        "peak_gpu_reservation_bytes":reserved,
        "scope":"RAM-only brick textures uploaded and released; dense CPU source retained; no bricked ray marching yet"
    }))
}
