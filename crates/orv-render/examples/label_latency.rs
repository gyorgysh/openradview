// SPDX-License-Identifier: GPL-3.0-or-later
//! Generated CPU-to-complete-GPU label revision timings; no input files or networking.
use orv_core::{
    labels::Labels,
    resources::MemoryBudget,
    study::{Geometry, demo_study},
    volume::Volume,
};
use orv_render::{
    label_source::{GpuLabels, UploadKind},
    volume_source::GpuVolumeSource,
};
use serde_json::{Value, json};
use std::{
    error::Error,
    hint::black_box,
    sync::Arc,
    time::{Duration, Instant},
};
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
    drop(blank);
    drop(source_gpu);
    fence(&device)?;
    if gpu_budget.used() != before_gpu || cpu_budget.used() != before_cpu {
        return Err("Label/source reservations did not return to baseline".into());
    }
    println!(
        "{}",
        serde_json::to_string_pretty(
            &json!({"schema_version":1,"version":env!("CARGO_PKG_VERSION"),"optimized":!cfg!(debug_assertions),"adapter":info.name,"backend":format!("{:?}",info.backend),"source_dimensions":[512,512,128],"source_fixture":"Source CPU frames share one constant plane; GPU source is a full uploaded 512x512x128 scalar texture. No decode or process RSS measurement.","scope":"Worker CPU checkpoint/paint, GPU target allocation, full prior-grid GPU copy, changed tile uploads and complete upload fence. No ray rendering, readback, native input/display or vsync.","admission_scope":"App-owned GPU source/label/staging reservations; not measured driver residency or free VRAM. Maximum observed admission includes retired versions not yet released.","warmups_per_case":4,"full_empty_map_upload":full,"edits":cases,"cpu_and_gpu_admission_restored":true})
        )?
    );
    Ok(())
}
