// SPDX-License-Identifier: GPL-3.0-or-later
//! Generated immutable geometry uploads and complete edit-to-upload timings.
use orv_core::{
    labels::Labels,
    mask::MaskOwner,
    resources::MemoryBudget,
    study::{Geometry, demo_study},
    surface::{CellSurface, SurfaceLimits},
    volume::Volume,
};
use orv_render::{
    GpuReservation,
    surface_source::{GpuCellSurface, UploadKind, UploadStatistics},
    volume_source::GpuVolumeSource,
};
use serde_json::{Value, json};
use std::{
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
fn source() -> Result<Arc<Volume>> {
    let frame = demo_study().series.remove(0).frames.remove(0);
    let plane: Arc<[f32]> = vec![0.; 512 * 512].into();
    Ok(Arc::new(Volume::new(
        (0..128)
            .map(|z| {
                let mut f = (*frame).clone();
                f.width = 512;
                f.height = 512;
                f.pixels = plane.clone();
                f.geometry = Some(Geometry {
                    position: [f64::from(z) * 0.05, 0., f64::from(z) * 1.2],
                    orientation: [1., 0., 0., 0., 1., 0.],
                    spacing: [0.7, 0.7],
                    frame_of_reference: Some("generated GPU geometry benchmark".into()),
                    nifti_transform: None,
                });
                Arc::new(f)
            })
            .collect(),
    )?))
}
fn fence(device: &wgpu::Device) -> Result<()> {
    device.poll(wgpu::PollType::Wait {
        submission_index: None,
        timeout: Some(Duration::from_secs(30)),
    })?;
    Ok(())
}
fn measure(mut work: impl FnMut(usize) -> Result<()>, samples: usize) -> Result<Value> {
    for i in 0..4 {
        work(i)?;
    }
    let mut times = Vec::new();
    for i in 4..samples + 4 {
        let start = Instant::now();
        work(i)?;
        times.push(start.elapsed().as_secs_f64() * 1000.);
    }
    times.sort_by(f64::total_cmp);
    Ok(
        json!({"samples":samples,"warmups":4,"median_ms":times[samples/2],"p95_ms":times[(samples*95).div_ceil(100)-1],"max_ms":times[samples-1]}),
    )
}
fn statistics(s: UploadStatistics) -> Value {
    json!({"kind":match s.kind {UploadKind::Full=>"full",UploadKind::Delta=>"delta",UploadKind::Reuse=>"reuse"},
        "uploaded_chunks":s.uploaded_chunks,"reused_chunks":s.reused_chunks,"removed_chunks":s.removed_chunks,
        "uploaded_bytes":s.uploaded_bytes,"gpu_target_admission_bytes":s.gpu_admission_bytes,
        "gpu_staging_admission_bytes":s.staging_admission_bytes,"new_cpu_metadata_admission_bytes":s.cpu_metadata_admission_bytes,
        "fenced_upload_batches":s.fenced_batches})
}
fn verify(gpu: &GpuCellSurface) -> Result<usize> {
    let device = gpu.source().device();
    let queue = gpu.source().queue();
    let mut count = 0;
    for chunk in gpu.chunks() {
        let bytes = chunk.data_bytes();
        let _gpu = GpuReservation::new(bytes.next_multiple_of(4096), device, queue)?;
        let _cpu = MemoryBudget::volumes()
            .reserve(bytes)
            .ok_or("Geometry readback exceeds CPU staging admission")?;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("generated geometry verification"),
            size: bytes,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.copy_buffer_to_buffer(chunk.buffer(), 0, &buffer, 0, bytes);
        queue.submit([encoder.finish()]);
        let (tx, rx) = mpsc::sync_channel(1);
        buffer.map_async(wgpu::MapMode::Read, .., move |r| {
            let _ = tx.send(r);
        });
        fence(device)?;
        rx.recv_timeout(Duration::from_secs(10))??;
        let data = buffer.get_mapped_range(..)?;
        let expected = gpu
            .surface()
            .chunk(chunk.cpu_chunk().coordinate())
            .ok_or("Generated CPU chunk missing")?
            .faces();
        if data.len() != expected.len() * 16 {
            return Err("Generated descriptor count mismatch".into());
        }
        for (value, face) in data.as_chunks::<16>().0.iter().zip(expected) {
            let value: [u32; 4] = std::array::from_fn(|i| {
                u32::from_le_bytes([
                    value[i * 4],
                    value[i * 4 + 1],
                    value[i * 4 + 2],
                    value[i * 4 + 3],
                ])
            });
            if value[..3] != face.cell()
                || value[3] & 255 != face.side() as u32
                || (value[3] & 256 != 0)
                    != (face.boundary() == orv_core::surface::FaceBoundary::Grid)
                || value[3] & !0x107 != 0
            {
                return Err("GPU descriptor differs from authoritative CPU geometry".into());
            }
            count += 1;
        }
        drop(data);
        buffer.unmap();
    }
    if count != gpu.surface().face_count() {
        return Err("Generated surface face count mismatch".into());
    }
    Ok(count)
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
    let cpu_budget = MemoryBudget::volumes();
    let gpu_budget = MemoryBudget::gpu();
    let before_cpu = cpu_budget.used();
    let before_gpu = gpu_budget.used();
    let cpu_source = source()?;
    let source = GpuVolumeSource::new(&device, &queue, cpu_source.clone(), || false)?;
    fence(&device)?;
    let source_admission = gpu_budget.used() - before_gpu;
    let mut cases = Vec::new();
    for (kind, slice, face_count) in [
        ("empty", None, 0),
        ("filled", None, 786432),
        ("one_cell_plane", Some(64), 526336),
    ] {
        let mut labels = Labels::empty(cpu_source.clone())?;
        if kind != "empty" {
            labels.threshold(slice, [-1., 1.], 1, || false)?;
        }
        let labels = Arc::new(labels);
        let surface = CellSurface::build(
            MaskOwner::Manual(labels.clone()),
            1,
            None,
            SurfaceLimits::default(),
            || false,
        )?
        .0;
        if surface.face_count() != face_count {
            return Err("Generated analytic face count differs".into());
        }
        let mut full_stats = None;
        let mut maximum_gpu = 0;
        let full = measure(
            |_| {
                let (upload, stats) =
                    GpuCellSurface::upload(source.clone(), surface.clone(), None, || false)?;
                full_stats = Some(stats);
                maximum_gpu = maximum_gpu.max(gpu_budget.used());
                drop(upload);
                fence(&device)
            },
            12,
        )?;
        let (base, _) = GpuCellSurface::upload(source.clone(), surface.clone(), None, || false)?;
        let verified_initial = verify(&base)?;
        fence(&device)?;
        let reuse = measure(
            |_| {
                let (same, stats) = GpuCellSurface::upload(
                    source.clone(),
                    surface.clone(),
                    Some(base.clone()),
                    || false,
                )?;
                if !Arc::ptr_eq(&same, &base)
                    || stats.kind != UploadKind::Reuse
                    || stats.uploaded_bytes != 0
                    || stats.cpu_metadata_admission_bytes != 0
                {
                    return Err("Unchanged GPU geometry was reallocated".into());
                }
                Ok(())
            },
            120,
        )?;
        let mut edits = Vec::new();
        for (name, point) in [("one_tile", [8., 8.]), ("four_tiles", [31., 31.])] {
            let mut current_labels = labels.clone();
            let mut current_surface = surface.clone();
            let mut current_gpu = base.clone();
            let mut last = None;
            let mut last_cpu = orv_core::surface::SurfaceUpdate::default();
            let mut total_bytes = 0;
            let mut max_uploaded_chunks = 0;
            let mut max_replaced_bytes = 0;
            let timing = measure(
                |i| {
                    let mut next = current_labels.try_clone(|| false)?;
                    let value = if kind == "empty" {
                        if i % 2 == 0 { 1 } else { 0 }
                    } else if i % 2 == 0 {
                        0
                    } else {
                        1
                    };
                    next.paint(64, &[point], 2, value, || false)?;
                    let next = Arc::new(next);
                    let (next_surface, cpu) = CellSurface::build(
                        MaskOwner::Manual(next.clone()),
                        1,
                        Some(&current_surface),
                        SurfaceLimits::default(),
                        || false,
                    )?;
                    let (next_gpu, stats) = GpuCellSurface::upload(
                        source.clone(),
                        next_surface.clone(),
                        Some(current_gpu.clone()),
                        || false,
                    )?;
                    if i >= 4 {
                        total_bytes += stats.uploaded_bytes;
                        max_uploaded_chunks = max_uploaded_chunks.max(stats.uploaded_chunks);
                        max_replaced_bytes = max_replaced_bytes.max(stats.uploaded_bytes);
                    }
                    maximum_gpu = maximum_gpu.max(gpu_budget.used());
                    current_gpu = next_gpu;
                    current_surface = next_surface;
                    current_labels = next;
                    last = Some(stats);
                    last_cpu = cpu;
                    fence(&device)
                },
                120,
            )?;
            // Verify an additional changed state, not only the final restored baseline.
            let mut check = current_labels.try_clone(|| false)?;
            check.paint(64, &[point], 2, if kind == "empty" { 1 } else { 0 }, || {
                false
            })?;
            let check = CellSurface::build(
                MaskOwner::Manual(Arc::new(check)),
                1,
                Some(&current_surface),
                SurfaceLimits::default(),
                || false,
            )?
            .0;
            let changed =
                GpuCellSurface::upload(source.clone(), check, Some(current_gpu.clone()), || false)?
                    .0;
            let verified_changed = verify(&changed)?;
            let verified_final = verify(&current_gpu)?;
            edits.push(json!({"case":name,"point":point,"radius_pixels":2,"checkpoint_brush_geometry_upload_and_fence":timing,
                "last_cpu_rebuilt_chunks":last_cpu.rebuilt_chunks,"last_cpu_reused_chunks":last_cpu.reused_chunks,
                "last_gpu_upload":statistics(last.ok_or("No generated edit statistics")?),"measured_total_uploaded_bytes":total_bytes,
                "max_uploaded_chunks":max_uploaded_chunks,"max_uploaded_bytes":max_replaced_bytes,
                "verified_changed_descriptors":verified_changed,"verified_final_descriptors":verified_final}));
        }
        cases.push(json!({"case":kind,"face_count":face_count,"fresh_gpu_upload_pack_fence_and_drop":full,
            "fresh_upload":statistics(full_stats.ok_or("No generated upload statistics")?),"unchanged_owner_reuse":reuse,
            "verified_initial_descriptors":verified_initial,"referenced_gpu_data_bytes":base.referenced_data_bytes(),
            "referenced_gpu_admission_bytes":base.referenced_gpu_admission_bytes(),"maximum_observed_gpu_admission_bytes":maximum_gpu,"edits":edits}));
        drop(base);
        drop(surface);
        drop(labels);
        fence(&device)?;
        if cpu_budget.used() != before_cpu || gpu_budget.used() != before_gpu + source_admission {
            return Err("Geometry reservations did not return to source baseline".into());
        }
    }
    drop(source);
    drop(cpu_source);
    fence(&device)?;
    if gpu_budget.used() != before_gpu || cpu_budget.used() != before_cpu {
        return Err("Generated reservations did not return to baseline".into());
    }
    println!(
        "{}",
        serde_json::to_string_pretty(
            &json!({"schema":1,"adapter":info.name,"backend":format!("{:?}",info.backend),
        "dimensions":[512,512,128],"logical_source_bytes":512*512*128*4,"shared_source_plane_bytes":512*512*4,
        "source_gpu_admission_bytes":source_admission,"max_buffer_size":device.limits().max_buffer_size,"descriptor_bytes":16,
        "cases":cases,"reservations_restored":true,
        "scope":"Fresh GPU geometry upload includes source-bound metadata, direct mapped packing, immutable chunk buffers, bounded staging, fence, drop and retirement fence. Core extraction and source upload are outside fresh timings. Edit timings include checkpoint, brush, CPU geometry, changed/equal chunk reuse, GPU upload and fence; they alternate add/erase or erase/restore. Uploads copy no old geometry buffers. Untimed readback compares every descriptor of initial/final/extra changed states to current CPU geometry. No 3D rasterization, ordered composition, smoothing, native input, presentation, file I/O or RSS measurement. Functional reuse timings are near clock resolution; admission is not measured driver memory."})
        )?
    );
    Ok(())
}
