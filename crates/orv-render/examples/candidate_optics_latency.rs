// SPDX-License-Identifier: GPL-3.0-or-later
//! Generated-data benchmark for partial candidate/source composition.
use orv_core::{
    ViewState,
    mask::MaskOwner,
    segments::{FractionalEncoding, FractionalKind, Segments},
    surface::field::{
        CandidateAssembly, CandidateAssemblyLimits, CandidateGridLimits, CellGraphLimits,
        CellSeamLimits, ConformingSurfaceLimits, FacePartitionLimits, MaskField,
        PhysicalConformingLimits, PlanarRefinementLimits, PlanarTriangulationLimits,
        RefinementLimits, SurfaceEnclosureLimits,
    },
    volume::Volume,
    volume_material::{MaterialPoint, VolumeMaterial},
    volume_ranges::VolumeRanges,
    volume_view::{Projection, VolumeView},
};
use orv_render::{
    candidate_opaque::{CandidateOpaqueRenderer, CandidateOpaqueScene},
    candidate_projection::{CandidateProjection, GpuCandidateProjector},
    candidate_source::{CandidateUploadLimits, GpuCandidateSurface},
    material_ranges::GpuMaterialRanges,
    material_volume::Lighting,
    volume_source::GpuVolumeSource,
};
use serde_json::{Value, json};
use std::{
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};
#[path = "fixtures/optical.rs"]
mod optical;
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn geometry(source: Arc<Volume>) -> Result<Arc<CandidateAssembly>> {
    let [w, h, _] = source.dimensions();
    let mut payload = vec![97u8; w * h * 2];
    for z in 0..2 {
        for y in 0..3 {
            for x in 0..3 {
                if (x % 2 == 0 && y % 2 == 0 && z == 0) || (x == 1 && y == 1 && z == 1) {
                    payload[z * w * h + (239 + y) * w + 255 + x] = 113;
                }
            }
        }
    }
    let mask = Arc::new(Segments::from_fractional(
        source,
        &[1],
        &[(1, 63), (1, 64)],
        &payload,
        FractionalEncoding {
            kind: FractionalKind::Probability,
            maximum: 200,
        },
        Some(true),
        || false,
    )?);
    let field = MaskField::new(MaskOwner::Supplied(mask), 1)?;
    let seams = CellSeamLimits {
        partition: FacePartitionLimits {
            max_edges_per_cell: 1024,
            max_points: 2048,
            ..Default::default()
        },
        ..Default::default()
    };
    let limits = CandidateGridLimits {
        max_cells: 256,
        max_directory_bytes: 1024 * 1024,
        graph: CellGraphLimits::default(),
        planar_refinement: PlanarRefinementLimits {
            curves: RefinementLimits {
                precision_bits: 6,
                max_vertices: 4096,
                ..Default::default()
            },
            max_vertices: 4096,
            ..Default::default()
        },
        triangulation: PlanarTriangulationLimits::default(),
        enclosure: SurfaceEnclosureLimits::default(),
        surface: PhysicalConformingLimits {
            max_error_mm: 1000.,
            max_depth: 2,
            max_vertices: 2048,
            max_triangles: 2048,
            max_operations: 2_000_000,
            max_bytes: ConformingSurfaceLimits::default().max_bytes,
        },
        seams,
    };
    let grid = field.candidate_grid([255, 239, 63], [2, 2, 1], 2, limits, || false)?;
    Ok(Arc::new(CandidateAssembly::build(
        &[&grid],
        CandidateAssemblyLimits {
            max_cells: 256,
            max_directory_bytes: 1024 * 1024,
            seams,
        },
        || false,
    )?))
}
fn material() -> Result<Arc<VolumeMaterial>> {
    Ok(Arc::new(VolumeMaterial::new(&[
        MaterialPoint {
            value: -1200.,
            luminance: 0.2,
            extinction_per_mm: 0.,
        },
        MaterialPoint {
            value: -800.,
            luminance: 0.2,
            extinction_per_mm: 0.,
        },
        MaterialPoint {
            value: -799.,
            luminance: 0.2,
            extinction_per_mm: 0.005,
        },
        MaterialPoint {
            value: 2000.,
            luminance: 0.2,
            extinction_per_mm: 0.01,
        },
    ])?))
}
struct Readback {
    bytes: Vec<u8>,
    _reservation: orv_core::resources::Reservation,
}
impl std::ops::Deref for Readback {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        &self.bytes
    }
}
fn read(
    source: &GpuVolumeSource,
    texture: &wgpu::Texture,
    bytes_per_pixel: u32,
) -> Result<Readback> {
    let width = texture.width();
    let height = texture.height();
    let row = (width * bytes_per_pixel).div_ceil(256) * 256;
    let bytes = u64::from(row) * u64::from(height);
    let _gpu = orv_render::GpuReservation::new(bytes, source.device(), source.queue())?;
    let cpu = orv_core::resources::MemoryBudget::volumes()
        .reserve(bytes * 2)
        .ok_or("No readback CPU allowance")?;
    let buffer = source.device().create_buffer(&wgpu::BufferDescriptor {
        label: Some("generated candidate benchmark readback"),
        size: bytes,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = source.device().create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: Some(height),
            },
        },
        texture.size(),
    );
    let index = source.queue().submit([encoder.finish()]);
    source.device().poll(wgpu::PollType::Wait {
        submission_index: Some(index),
        timeout: Some(Duration::from_secs(30)),
    })?;
    let (sender, receiver) = mpsc::sync_channel(1);
    buffer.map_async(wgpu::MapMode::Read, .., move |result| {
        let _ = sender.send(result);
    });
    source.device().poll(wgpu::PollType::Wait {
        submission_index: None,
        timeout: Some(Duration::from_secs(30)),
    })?;
    receiver.recv_timeout(Duration::from_secs(30))??;
    let mapped = buffer.get_mapped_range(..)?;
    let mut result = Vec::new();
    result.try_reserve_exact((width * height * bytes_per_pixel) as usize)?;
    for data in mapped.chunks_exact(row as usize) {
        result.extend_from_slice(&data[..(width * bytes_per_pixel) as usize]);
    }
    drop(mapped);
    buffer.unmap();
    Ok(Readback {
        bytes: result,
        _reservation: cpu,
    })
}
fn half(word: u16) -> Result<f64> {
    let exponent = i32::from((word >> 10) & 31);
    if exponent == 31 {
        return Err("Non-finite pixel".into());
    }
    let fraction = f64::from(word & 1023);
    let value = if exponent == 0 {
        fraction * 2f64.powi(-24)
    } else {
        (1. + fraction / 1024.) * 2f64.powi(exponent - 15)
    };
    Ok(if word & 32768 == 0 { value } else { -value })
}
fn verify(a: &[u8], b: &[u8]) -> Result<Value> {
    if a.len() != b.len() || !a.len().is_multiple_of(8) {
        return Err("Mismatched image dimensions".into());
    }
    let mut maximum = 0f64;
    let mut unequal = 0usize;
    for (a, b) in a.as_chunks::<8>().0.iter().zip(b.as_chunks::<8>().0) {
        if a != b {
            unequal += 1;
        }
        let mut aa = [0.; 4];
        let mut bb = [0.; 4];
        for i in 0..4 {
            aa[i] = half(u16::from_le_bytes([a[2 * i], a[2 * i + 1]]))?;
            bb[i] = half(u16::from_le_bytes([b[2 * i], b[2 * i + 1]]))?;
            maximum = maximum.max((aa[i] - bb[i]).abs());
        }
        if aa[0] != aa[1]
            || aa[1] != aa[2]
            || aa[3] != 1.
            || bb[0] != bb[1]
            || bb[1] != bb[2]
            || bb[3] != 1.
        {
            return Err("Unexpected output color or alpha".into());
        }
    }
    if maximum > 0.001 {
        return Err(format!("Pixel difference {maximum} exceeds the declared bound").into());
    }
    Ok(
        json!({"pixels":a.len()/8,"unequal_pixels":unequal,"maximum_linear_channel_error":maximum,"tolerance":0.001}),
    )
}
fn measure(
    renderers: [&CandidateOpaqueRenderer; 2],
    scene: &CandidateOpaqueScene,
    samples: usize,
) -> Result<[Value; 2]> {
    for i in 0..4 {
        drop(renderers[i % 2].render(scene.clone(), || false)?);
    }
    let mut times: [Vec<f64>; 2] = std::array::from_fn(|_| Vec::new());
    for cycle in 0..samples {
        for order in 0..2 {
            let mode = (cycle + order) % 2;
            let start = Instant::now();
            drop(renderers[mode].render(scene.clone(), || false)?);
            times[mode].push(start.elapsed().as_secs_f64() * 1000.);
        }
    }
    Ok(times.map(|mut values| {
        values.sort_by(f64::total_cmp);
        json!({"samples":samples,"warmups_per_mode":2,"median_ms":values[samples/2],"p95_ms":values[(samples*95).div_ceil(100)-1],"max_ms":values[samples-1],"order":"interleaved alternating dense/accelerated"})
    }))
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
    let mut cases = Vec::new();
    for kind in ["sparse", "dense", "missing"] {
        let start = Instant::now();
        let cpu = optical::source(kind)?;
        let source_ms = start.elapsed().as_secs_f64() * 1000.;
        let start = Instant::now();
        let source = GpuVolumeSource::new(&device, &queue, cpu.clone(), || false)?;
        let upload_ms = start.elapsed().as_secs_f64() * 1000.;
        let start = Instant::now();
        let assembly = geometry(cpu.clone())?;
        let assembly_ms = start.elapsed().as_secs_f64() * 1000.;
        let start = Instant::now();
        let (surface, stats) = GpuCandidateSurface::upload(
            source.clone(),
            assembly,
            None,
            CandidateUploadLimits::default(),
            || false,
        )?;
        let geometry_upload_ms = start.elapsed().as_secs_f64() * 1000.;
        if stats.represented_triangles == 0 {
            return Err("No candidate triangles generated".into());
        }
        let material = material()?;
        let start = Instant::now();
        let summary = VolumeRanges::build(cpu.clone(), 32, || false)?;
        let cpu_classify_ms = start.elapsed().as_secs_f64() * 1000.;
        let start = Instant::now();
        let ranges = GpuMaterialRanges::new(source.clone(), summary, material.clone(), || false)?;
        let gpu_classify_ms = start.elapsed().as_secs_f64() * 1000.;
        if kind == "sparse" && ranges.transparent_tiles() == 0 {
            return Err("Sparse source produced no certified transparent tiles".into());
        }
        let projector = GpuCandidateProjector::new(source.clone())?;
        let dense = CandidateOpaqueRenderer::new(source.clone(), wgpu::TextureFormat::Rgba8Unorm)?;
        let fast = CandidateOpaqueRenderer::new_accelerated(
            ranges.clone(),
            wgpu::TextureFormat::Rgba8Unorm,
        )?;
        for side in [512u32, 1024] {
            if kind == "missing" && side == 1024 {
                continue;
            }
            let mut volume = VolumeView::fit(&cpu);
            volume.projection = Projection::Composite;
            volume.camera.target_mm = cpu.voxel_to_world([256., 240., 63.5]);
            volume.camera.vertical_span_mm = 120.;
            volume.camera.yaw_degrees = 31.;
            volume.camera.pitch_degrees = -23.;
            volume.camera.roll_degrees = 7.;
            let navigation = ViewState::default();
            let start = Instant::now();
            let candidate = projector.render(
                CandidateProjection {
                    surface: surface.clone(),
                    camera: volume.camera.clone(),
                    display: navigation.clone(),
                    clip: None,
                    size: [side; 2],
                },
                || false,
            )?;
            let project_ms = start.elapsed().as_secs_f64() * 1000.;
            let hits = read(&source, candidate.cells(), 16)?
                .as_chunks::<16>()
                .0
                .iter()
                .filter(|word| word[12..16] != [0; 4])
                .count();
            if hits == 0 {
                return Err("No candidate hits in benchmark view".into());
            }
            let scene = CandidateOpaqueScene {
                surface: candidate,
                navigation,
                volume,
                material: material.clone(),
                lighting: Lighting::UNLIT,
                surface_lighting: Lighting::HEADLIGHT,
                surface_luminance: 0.85,
            };
            let a = dense.render(scene.clone(), || false)?;
            let b = fast.render(scene.clone(), || false)?;
            let comparison = verify(
                &read(&source, a.texture(), 8)?,
                &read(&source, b.texture(), 8)?,
            )?;
            drop(a);
            drop(b);
            let timings = measure([&dense, &fast], &scene, if side == 512 { 8 } else { 4 })?;
            cases.push(json!({"source":kind,"size":[side,side],"candidate_triangles":stats.represented_triangles,"candidate_pixels":hits,"transparent_tiles":ranges.transparent_tiles(),"source_fixture_ms":source_ms,"source_gpu_upload_ms":upload_ms,"candidate_assembly_ms":assembly_ms,"candidate_upload_ms":geometry_upload_ms,"cpu_ranges_ms":cpu_classify_ms,"gpu_ranges_ms":gpu_classify_ms,"candidate_projection_ms":project_ms,"dense":timings[0],"accelerated":timings[1],"pixel_comparison":comparison}));
            eprintln!("Verified {kind} {side}");
        }
    }
    println!(
        "{}",
        serde_json::to_string_pretty(
            &json!({"adapter":info.name,"backend":format!("{:?}",info.backend),"dimensions":[512,512,128],"range_side":32,"scope":"Generated source. CPU and GPU preparation measured separately once per source. Composition timing includes fresh target allocation, 16-row fenced GPU batches and status readback with cached projected candidates; excludes readback verification, native UI, input/display latency, RSS and total dataset preparation. Candidate geometry is partial and is not a complete surface. Dense and accelerated images compared across every pixel to 0.001 linear units.","cases":cases})
        )?
    );
    Ok(())
}
