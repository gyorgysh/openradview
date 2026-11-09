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

fn grids(
    volume: &Volume,
    index: usize,
    slab: Option<Slab>,
    start_z: f64,
) -> Result<[ReformatGrid; 3]> {
    let point = volume.voxel_to_world([
        200.25 + (index % 13) as f64 * 0.37,
        240.125 + (index % 11) as f64 * 0.31,
        start_z + (index % 7) as f64 * 0.23,
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
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    let (mpr_33, crossing) = match arguments.as_slice() {
        [] => (false, false),
        [argument] if argument == "--mpr-33" => (true, false),
        [argument] if argument == "--mpr-33-crossing" => (true, true),
        _ => return Err("Use no arguments, --mpr-33 or --mpr-33-crossing".into()),
    };
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        flags: orv_render::instance_flags(),
        backends: orv_render::configured_backends()?,
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapter = pollster::block_on(instance.request_adapter(&Default::default()))?;
    let info = adapter.get_info();
    let (device, queue) =
        pollster::block_on(adapter.request_device(&orv_render::device_descriptor(&adapter)))?;
    let mut cases = Vec::new();
    for &kind in if crossing {
        &["missing"][..]
    } else {
        &["sparse", "missing"][..]
    } {
        let start_z = if crossing { 51. } else { 58.75 };
        let volume = optical::source(kind)?;
        let start = Instant::now();
        let source = GpuVolumeSource::new(&device, &queue, volume.clone(), || false)?;
        let source_ms = start.elapsed().as_secs_f64() * 1000.;
        let mut renderer = GpuReslicer::new(source)?;
        let first = grids(&volume, 0, None, start_z)?;
        let start = Instant::now();
        drop(renderer.render_frames(first.each_ref().map(Some), || false)?);
        fence(&device)?;
        let pipeline_first_frame_ms = start.elapsed().as_secs_f64() * 1000.;
        let standard = [
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
        ];
        let mpr_cases = [
            (
                "maximum_33_one_changed",
                Some(Slab {
                    projection: SlabProjection::Maximum,
                    thickness_mm: 10.,
                    intervals: 32,
                }),
                1,
                12,
            ),
            (
                "mean_33_one_changed",
                Some(Slab {
                    projection: SlabProjection::Mean,
                    thickness_mm: 10.,
                    intervals: 32,
                }),
                1,
                12,
            ),
        ];
        for &(name, slab, planes, samples) in if mpr_33 {
            &mpr_cases[..]
        } else {
            &standard[..]
        } {
            let warm = grids(&volume, 0, slab, start_z)?;
            let mut cpu_front = cpu(&volume, &warm, planes)?;
            let mut gpu_front = renderer.render_frames(
                std::array::from_fn(|i| (i < planes).then_some(&warm[i])),
                || false,
            )?;
            let (mut cpu_times, mut gpu_times) = (Vec::new(), Vec::new());
            let (mut corrections, mut pixels, mut readback, mut uploads) =
                (0usize, 0usize, 0u64, 0u64);
            let mut cpu_fingerprint = 0xcbf29ce484222325u64;
            for i in 0..samples {
                let run_cpu = |front: &mut Vec<Arc<Frame>>| -> Result<f64> {
                    let start = Instant::now();
                    let grids = grids(&volume, i + 1, slab, start_z)?;
                    *front = cpu(&volume, &grids, planes)?;
                    Ok(start.elapsed().as_secs_f64() * 1000.)
                };
                let mut run_gpu = |front: &mut _| -> Result<f64> {
                    let start = Instant::now();
                    let grids = grids(&volume, i + 1, slab, start_z)?;
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
                        cpu_fingerprint =
                            (cpu_fingerprint ^ u64::from(a.to_bits())).wrapping_mul(0x100000001b3);
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
            cases.push(json!({"source":kind,"case":name,"requested_side":512,"planes":planes,"generated_start_z":start_z,
                "cpu":summary(cpu_times),"gpu_complete":summary(gpu_times),"corrected_pixels":corrections,
                "output_pixels":pixels,"readback_bytes":readback,"correction_uploaded_bytes":uploads,
                "cpu_fingerprint":format!("{cpu_fingerprint:016x}"),
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
