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
    for argument in std::env::args().skip(1) {
        if argument == "--cpu-only" {
            cpu_only = true;
        } else if let Some(value) = argument.strip_prefix("--depth=") {
            depth = value.parse()?;
        } else if let Some(value) = argument.strip_prefix("--repetitions=") {
            repetitions = value.parse()?;
        } else {
            return Err("Use --cpu-only, --depth=2..2048 or --repetitions=1..10".into());
        }
    }
    if !(2..=2048).contains(&depth) || !(1..=10).contains(&repetitions) {
        return Err("Capacity parameters exceed the supported range".into());
    }
    for repetition in 0..repetitions {
        let report = run(cpu_only, depth)?;
        println!(
            "{}",
            serde_json::json!({"repetition":repetition+1,"result":report})
        );
    }
    Ok(())
}
fn run(cpu_only: bool, depth: usize) -> Result<serde_json::Value, Box<dyn Error>> {
    use orv_core::resources::MemoryBudget;
    let cpu_before = MemoryBudget::volumes().used();
    let gpu_before = MemoryBudget::gpu().used();
    let started = Instant::now();
    let mut frames = Vec::new();
    frames.try_reserve_exact(depth)?;
    for z in 0..depth {
        let mut values = Vec::new();
        values.try_reserve_exact(1024 * 768)?;
        values.extend((0..1024 * 768).map(|index| z as f32 + (index % 1024) as f32 * 0.001));
        frames.push(Arc::new(Frame {
            slab: None,
            scalar: None,
            width: 1024,
            height: 768,
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
    let point = volume.voxel_to_world([512.5, 384.25, depth as f64 * 0.5 - 0.5]);
    let expected = depth as f64 * 0.5 - 0.5 + 0.5125;
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
            backends: orv_render::configured_backends()?,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = pollster::block_on(instance.request_adapter(&Default::default()))?;
        let info = adapter.get_info();
        let (device, queue) =
            pollster::block_on(adapter.request_device(&orv_render::device_descriptor(&adapter)))?;
        let start = Instant::now();
        let mut renderer = orv_render::volume::VolumeRenderer::new(
            &device,
            &queue,
            volume.clone(),
            wgpu::TextureFormat::Rgba8Unorm,
            || false,
        )?;
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
        gpu = serde_json::json!({"backend":format!("{:?}",info.backend),"adapter":info.name,"source_upload_ms":upload_ms,"ray_dispatch_completed":true,"retained_gpu_reservation_bytes":retained_gpu_reservation});
    }
    let report = serde_json::json!({
        "schema_version":1,"version":env!("CARGO_PKG_VERSION"),"optimized":!cfg!(debug_assertions),
        "scope":"Generated f32 volume admission, analytic sample, CPU MPR and optional real GPU upload/dispatch; no source-file decode or UI latency",
        "dimensions":volume.dimensions(),"retained_source_bytes":1024u64*768*depth as u64*4,
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
