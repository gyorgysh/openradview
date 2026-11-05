// SPDX-License-Identifier: GPL-3.0-or-later
//! Complete generated scalar frames, including readback and required corrections.
#[path = "fixtures/optical.rs"]
mod optical;
use orv_core::{
    Plane,
    oblique::ObliquePlane,
    resources::MemoryBudget,
    slab::{Slab, SlabProjection},
    study::Frame,
    volume::{ReformatGrid, Volume},
};
use orv_render::{reslice::GpuReslicer, volume_source::GpuVolumeSource};
use serde_json::{Value, json};
use std::{
    error::Error,
    sync::Arc,
    time::{Duration, Instant},
};
type Result<T> = std::result::Result<T, Box<dyn Error>>;

fn grids(volume: &Volume, index: usize, slab: Option<Slab>) -> Result<[ReformatGrid; 3]> {
    let point = volume.voxel_to_world([
        200.25 + (index % 13) as f64 * 0.37,
        240.125 + (index % 11) as f64 * 0.31,
        58.75 + (index % 7) as f64 * 0.23,
    ]);
    [Plane::Axial, Plane::Coronal, Plane::Sagittal]
        .map(|plane| {
            if let Some(slab) = slab {
                volume.oblique_grid(&ObliquePlane::anatomical(plane, point)?, 512, Some(slab))
            } else {
                volume.reformat_grid(plane, point, 512)
            }
        })
        .into_iter()
        .collect::<std::result::Result<Vec<_>, _>>()?
        .try_into()
        .map_err(|_| "Three generated grids required".into())
}
fn cpu(volume: &Volume, grids: &[ReformatGrid; 3], count: usize) -> Result<Vec<Arc<Frame>>> {
    grids[..count]
        .iter()
        .map(|grid| Ok(volume.resample_grid(grid, || false)?))
        .collect()
}
fn summary(mut times: Vec<f64>) -> Value {
    times.sort_by(f64::total_cmp);
    let n = times.len();
    json!({"samples": n, "median_ms":times[n/2], "p95_ms":times[(n*95).div_ceil(100)-1], "max_ms": times[n-1]})
}
fn fence(device: &wgpu::Device) -> Result<()> {
    device.poll(wgpu::PollType::Wait {
        submission_index: None,
        timeout: Some(Duration::from_secs(30)),
    })?;
    Ok(())
}
fn main() -> Result<()> {
    if std::env::args().len() != 1 {
        return Err("This generated benchmark accepts no arguments".into());
    }
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: orv_render::configured_backends()?,
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapter = pollster::block_on(instance.request_adapter(&Default::default()))?;
    let info = adapter.get_info();
    let (device, queue) =
        pollster::block_on(adapter.request_device(&orv_render::device_descriptor(&adapter)))?;
    let mut cases = Vec::new();
    for kind in ["sparse", "missing"] {
        let volume = optical::source(kind)?;
        let start = Instant::now();
        let source = GpuVolumeSource::new(&device, &queue, volume.clone(), || false)?;
        let source_ms = start.elapsed().as_secs_f64() * 1000.;
        let mut renderer = GpuReslicer::new(source)?;
        let first = grids(&volume, 0, None)?;
        let start = Instant::now();
        drop(renderer.render_frames(first.each_ref().map(Some), || false)?);
        fence(&device)?;
        let pipeline_first_frame_ms = start.elapsed().as_secs_f64() * 1000.;
        for (name, slab, planes, samples) in [
            ("thin_three", None, 3, 12),
            ("thin_one_changed", None, 1, 12),
            (
                "maximum_65_three",
                Some(Slab {
                    projection: SlabProjection::Maximum,
                    thickness_mm: 20.,
                    intervals: 64,
                }),
                3,
                3,
            ),
            (
                "mean_257_three",
                Some(Slab {
                    projection: SlabProjection::Mean,
                    thickness_mm: 20.,
                    intervals: 256,
                }),
                3,
                3,
            ),
        ] {
            let warm = grids(&volume, 0, slab)?;
            let mut cpu_front = cpu(&volume, &warm, planes)?;
            let mut gpu_front = renderer.render_frames(
                std::array::from_fn(|i| (i < planes).then_some(&warm[i])),
                || false,
            )?;
            let (mut cpu_times, mut gpu_times) = (Vec::new(), Vec::new());
            let (mut corrections, mut pixels, mut readback, mut uploads) =
                (0usize, 0usize, 0u64, 0u64);
            for i in 0..samples {
                let run_cpu = |front: &mut Vec<Arc<Frame>>| -> Result<f64> {
                    let start = Instant::now();
                    let grids = grids(&volume, i + 1, slab)?;
                    *front = cpu(&volume, &grids, planes)?;
                    Ok(start.elapsed().as_secs_f64() * 1000.)
                };
                let mut run_gpu = |front: &mut _| -> Result<f64> {
                    let start = Instant::now();
                    let grids = grids(&volume, i + 1, slab)?;
                    *front = renderer.render_frames(
                        std::array::from_fn(|i| (i < planes).then_some(&grids[i])),
                        || false,
                    )?;
                    fence(&device)?;
                    Ok(start.elapsed().as_secs_f64() * 1000.)
                };
                if i % 2 == 0 {
                    cpu_times.push(run_cpu(&mut cpu_front)?);
                    gpu_times.push(run_gpu(&mut gpu_front)?);
                } else {
                    gpu_times.push(run_gpu(&mut gpu_front)?);
                    cpu_times.push(run_cpu(&mut cpu_front)?);
                }
                let stats = renderer.frame_statistics();
                corrections += stats.corrected_pixels;
                readback += stats.readback_bytes;
                uploads += stats.correction_uploaded_bytes;
                for (a, b) in cpu_front.iter().zip(gpu_front.iter().flatten()) {
                    if a.geometry != b.frame().geometry
                        || a.slab != b.frame().slab
                        || a.pixels.len() != b.frame().pixels.len()
                    {
                        return Err("Generated frame metadata differs".into());
                    }
                    pixels += a.pixels.len();
                    for (&a, &b) in a.pixels.iter().zip(b.frame().pixels.iter()) {
                        if a.is_finite() != b.is_finite()
                            || (a.is_finite() && (a - b).abs() > 0.0002 * a.abs())
                        {
                            return Err(
                                "Generated scalar frame differs from its CPU reference".into()
                            );
                        }
                    }
                }
            }
            cases.push(json!({"source":kind,"case":name,"requested_side":512,"planes":planes,
                "cpu":summary(cpu_times),"gpu_complete":summary(gpu_times),"corrected_pixels":corrections,
                "output_pixels":pixels,"readback_bytes":readback,"correction_uploaded_bytes":uploads,
                "verified_every_sample":true,"source_upload_ms":source_ms,"pipeline_first_three_frames_ms":pipeline_first_frame_ms,
                "app_cpu_admission_bytes_with_fronts":MemoryBudget::volumes().used(),
                "app_gpu_admission_bytes_with_fronts":MemoryBudget::gpu().used()}));
        }
        drop(renderer);
        fence(&device)?;
    }
    println!(
        "{}",
        serde_json::to_string_pretty(
            &json!({"schema":1,"adapter":info.name,"backend":format!("{:?}",info.backend),
        "source_dimensions":[512,512,128],"warmups":1,"order":"alternating CPU/GPU, same moving grids",
        "scope":"Grid setup, new complete frames with previous fronts retained, GPU dispatch/readback/validation/corrections/upload/fences, replacement/drop. Source generation/upload, first pipeline, CPU comparison, display/native presentation excluded. One-plane case excludes unchanged-plane presentation and retention.",
        "limits":{"relative_scalar_tolerance":0.0002,"validity":"exact","coordinates":"CPU geometry", "admission":"Application reservations, not driver memory or RSS"},"cases":cases})
        )?
    );
    Ok(())
}
