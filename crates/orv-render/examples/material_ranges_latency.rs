// SPDX-License-Identifier: GPL-3.0-or-later
//! Generated resident optical rendering; no scans, files or network.
use orv_core::{
    ViewState,
    resources::MemoryBudget,
    study::{Geometry, demo_study},
    volume::Volume,
    volume_material::{MaterialPoint, VolumeMaterial},
    volume_ranges::VolumeRanges,
    volume_view::{Projection, Quality, VolumeView},
};
use orv_render::{
    material_ranges::GpuMaterialRanges,
    material_volume::{Lighting, MaterialVolumeRenderer},
    volume::VolumeTarget,
    volume_source::GpuVolumeSource,
};
use serde_json::{Value, json};
use std::{
    cell::Cell,
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn source(kind: &str) -> Result<Arc<Volume>> {
    let template = demo_study().series.remove(0).frames.remove(0);
    let mut frames = Vec::new();
    for z in 0..128 {
        let mut f = (*template).clone();
        let mut pixels = Vec::with_capacity(512 * 512);
        let mut valid = Vec::new();
        for y in 0..512 {
            for x in 0..512 {
                let inside = ((x as f64 - 220.) / 100.).powi(2)
                    + ((y as f64 - 240.) / 80.).powi(2)
                    + ((z as f64 - 64.) / 43.).powi(2)
                    <= 1.;
                let plate = x == 257 && (180..340).contains(&y);
                let known = kind != "missing" || (z >= 40 && z % 17 != 0);
                let value = if !known {
                    if x % 2 == 0 { -1e20 } else { 1e20 }
                } else if kind == "dense" {
                    (x + 2 * y + 3 * z) as f32
                } else if inside || plate {
                    ((x * 13 + y * 7 + z * 17) % 700 + 100) as f32
                } else {
                    -1000.
                };
                pixels.push(value);
                if kind == "missing" {
                    valid.push(u8::from(known));
                }
            }
        }
        f.width = 512;
        f.height = 512;
        f.pixels = pixels.into();
        f.validity = (kind == "missing").then(|| valid.into());
        f.geometry = Some(Geometry {
            position: [z as f64 * 0.05, 0., z as f64 * 1.2],
            orientation: [1., 0., 0., 0., 1., 0.],
            spacing: [0.7, 0.7],
            frame_of_reference: Some("generated optical acceleration".into()),
            nifti_transform: None,
        });
        frames.push(Arc::new(f));
    }
    Ok(Volume::prepare(frames, || false)?)
}
fn fence(d: &wgpu::Device, q: &wgpu::Queue, encoder: wgpu::CommandEncoder) -> Result<()> {
    let index = q.submit([encoder.finish()]);
    d.poll(wgpu::PollType::Wait {
        submission_index: Some(index),
        timeout: Some(Duration::from_secs(30)),
    })?;
    Ok(())
}
fn compare(
    mut work: impl FnMut(usize) -> Result<()>,
    samples: usize,
    warmups: usize,
) -> Result<[Value; 3]> {
    for cycle in 0..warmups {
        for mode in 0..3 {
            work((mode + cycle) % 3)?;
        }
    }
    let mut times: [Vec<f64>; 3] = std::array::from_fn(|_| Vec::with_capacity(samples));
    for cycle in 0..samples {
        for order in 0..3 {
            let mode = (order + cycle) % 3;
            let start = Instant::now();
            work(mode)?;
            times[mode].push(start.elapsed().as_secs_f64() * 1000.);
        }
    }
    Ok(times.map(|mut values|{values.sort_by(f64::total_cmp);json!({"samples":samples,"warmups":warmups,
        "median_ms":values[samples/2],"p95_ms":values[(samples*95).div_ceil(100)-1],"max_ms":values[samples-1],"order":"rotating interleaved legacy/dense/accelerated"})}))
}
fn measure(mut work: impl FnMut() -> Result<()>, samples: usize, warmups: usize) -> Result<Value> {
    for _ in 0..warmups {
        work()?;
    }
    let mut times = Vec::new();
    for _ in 0..samples {
        let start = Instant::now();
        work()?;
        times.push(start.elapsed().as_secs_f64() * 1000.);
    }
    times.sort_by(f64::total_cmp);
    Ok(
        json!({"samples":samples,"warmups":warmups,"median_ms":times[samples/2],"p95_ms":times[(samples*95).div_ceil(100)-1],"max_ms":times[samples-1]}),
    )
}
struct Render<'a> {
    gpu: &'a GpuVolumeSource,
    state: &'a VolumeView,
    material: &'a VolumeMaterial,
    lighting: Lighting,
    size: [u32; 2],
    peak: &'a Cell<u64>,
}
impl Render<'_> {
    fn run(&self, renderer: &mut MaterialVolumeRenderer, rows: u32) -> Result<()> {
        for y in (0..self.size[1]).step_by(rows as usize) {
            let mut encoder = self
                .gpu
                .device()
                .create_command_encoder(&Default::default());
            renderer.render_target(
                &mut encoder,
                &ViewState::default(),
                self.state,
                self.material,
                self.lighting,
                VolumeTarget {
                    slot: 0,
                    size: self.size,
                    region: [[0, y], [self.size[0], rows.min(self.size[1] - y)]],
                },
            )?;
            self.peak
                .set(self.peak.get().max(MemoryBudget::gpu().used()));
            fence(self.gpu.device(), self.gpu.queue(), encoder)?;
        }
        Ok(())
    }
}
struct Readback {
    data: Vec<u8>,
    _charge: orv_core::resources::Reservation,
}
impl std::ops::Deref for Readback {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        &self.data
    }
}
impl PartialEq for Readback {
    fn eq(&self, other: &Self) -> bool {
        self.data == other.data
    }
}
fn read(gpu: &GpuVolumeSource, texture: &wgpu::Texture) -> Result<Readback> {
    let row = (texture.width() * 8).div_ceil(256) * 256;
    let size = u64::from(row) * u64::from(texture.height());
    let cpu = MemoryBudget::volumes()
        .reserve(size * 2)
        .ok_or("No readback CPU allowance")?;
    let charge = orv_render::GpuReservation::new(size, gpu.device(), gpu.queue())?;
    let buffer = gpu.device().create_buffer(&wgpu::BufferDescriptor {
        label: Some("generated optical benchmark readback"),
        size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = gpu.device().create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: Some(texture.height()),
            },
        },
        texture.size(),
    );
    fence(gpu.device(), gpu.queue(), encoder)?;
    let (tx, rx) = mpsc::sync_channel(1);
    buffer.map_async(wgpu::MapMode::Read, .., move |r| {
        let _ = tx.send(r);
    });
    gpu.device().poll(wgpu::PollType::Wait {
        submission_index: None,
        timeout: Some(Duration::from_secs(30)),
    })?;
    rx.recv_timeout(Duration::from_secs(30))??;
    let mapped = buffer.get_mapped_range(..)?;
    let mut result = Vec::new();
    result.try_reserve_exact(texture.width() as usize * texture.height() as usize * 8)?;
    for data in mapped.chunks_exact(row as usize) {
        result.extend_from_slice(&data[..texture.width() as usize * 8]);
    }
    drop(mapped);
    buffer.unmap();
    drop(charge);
    Ok(Readback {
        data: result,
        _charge: cpu,
    })
}
fn half(bits: u16) -> Result<f64> {
    let e = i32::from((bits >> 10) & 31);
    if e == 31 {
        return Err("Non-finite optical output".into());
    }
    let f = f64::from(bits & 1023);
    let v = if e == 0 {
        f * 2f64.powi(-24)
    } else {
        (1. + f / 1024.) * 2f64.powi(e - 15)
    };
    Ok(if bits & 32768 == 0 { v } else { -v })
}
fn pixel(data: &[u8], index: usize) -> Result<[f64; 4]> {
    let mut out = [0.; 4];
    for (i, v) in out.iter_mut().enumerate() {
        let offset = index * 8 + i * 2;
        *v = half(u16::from_le_bytes(data[offset..offset + 2].try_into()?))?;
    }
    if out[0] != out[1] || out[1] != out[2] || out[3] != 1. {
        return Err("Invalid optical color/alpha".into());
    }
    Ok(out)
}
fn reference(
    volume: &Volume,
    state: &VolumeView,
    material: &VolumeMaterial,
    size: [u32; 2],
    index: usize,
) -> Result<f64> {
    let frame = state.camera.frame(
        &ViewState::default(),
        f64::from(size[0]) / f64::from(size[1]),
    )?;
    let (origin, direction) = frame.ray([
        ((index % size[0] as usize) as f64 + 0.5) / f64::from(size[0]),
        ((index / size[0] as usize) as f64 + 0.5) / f64::from(size[1]),
    ]);
    let Some([near, far]) = volume.ray_interval(origin, direction) else {
        return Ok(0.0030352698);
    };
    let speed = volume
        .world_vector_to_voxel(direction)
        .into_iter()
        .map(f64::abs)
        .fold(0., f64::max);
    let count = ((far - near) * speed / state.quality.step_factor())
        .ceil()
        .clamp(1., f64::from(state.quality.max_steps())) as u32;
    let step = (far - near) / f64::from(count);
    let mut transmittance = 1.;
    let mut radiance = 0.;
    for i in 0..count {
        let t = near + (f64::from(i) + 0.5) * step;
        let point = std::array::from_fn(|a| origin[a] + direction[a] * t);
        if let Some(value) = volume.sample(point) {
            let sample = material.sample(value).ok_or("Missing reference material")?;
            let alpha = sample.opacity(step).ok_or("Invalid reference depth")?;
            radiance += transmittance * alpha * sample.luminance;
            transmittance *= 1. - alpha;
            if transmittance < 0.0001 {
                break;
            }
        }
    }
    Ok(radiance + transmittance * 0.0030352698)
}
fn main() -> Result<()> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: orv_render::configured_backends()?,
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapter = pollster::block_on(instance.request_adapter(&Default::default()))?;
    let info = adapter.get_info();
    let (d, q) =
        pollster::block_on(adapter.request_device(&orv_render::device_descriptor(&adapter)))?;
    let cpu = MemoryBudget::volumes();
    let gpu_budget = MemoryBudget::gpu();
    let cpu_base = cpu.used();
    let gpu_base = gpu_budget.used();
    let peak = Cell::new(gpu_base);
    let mut cases = Vec::new();
    let material = Arc::new(VolumeMaterial::new(&[
        MaterialPoint {
            value: -1200.,
            luminance: 0.,
            extinction_per_mm: 0.,
        },
        MaterialPoint {
            value: -800.,
            luminance: 0.,
            extinction_per_mm: 0.,
        },
        MaterialPoint {
            value: -799.,
            luminance: 0.2,
            extinction_per_mm: 0.005,
        },
        MaterialPoint {
            value: 0.,
            luminance: 0.4,
            extinction_per_mm: 0.01,
        },
        MaterialPoint {
            value: 2000.,
            luminance: 0.8,
            extinction_per_mm: 0.02,
        },
    ])?);
    for kind in ["sparse", "missing", "dense"] {
        let volume = source(kind)?;
        let start = Instant::now();
        let gpu = GpuVolumeSource::new(&d, &q, volume.clone(), || false)?;
        let source_upload_ms = start.elapsed().as_secs_f64() * 1000.;
        let start = Instant::now();
        let ranges = VolumeRanges::build(volume.clone(), 32, || false)?;
        let cpu_prepare_ms = start.elapsed().as_secs_f64() * 1000.;
        let start = Instant::now();
        let flags =
            GpuMaterialRanges::new(gpu.clone(), ranges.clone(), material.clone(), || false)?;
        let gpu_prepare_ms = start.elapsed().as_secs_f64() * 1000.;
        let start = Instant::now();
        let format = wgpu::TextureFormat::Rgba8UnormSrgb;
        let mut legacy = MaterialVolumeRenderer::new(gpu.clone(), format)?;
        let mut dense = MaterialVolumeRenderer::new_dense_reference(gpu.clone(), format)?;
        let mut accelerated = MaterialVolumeRenderer::new_accelerated(flags.clone(), format)?;
        let pipeline_ms = start.elapsed().as_secs_f64() * 1000.;
        let mut state = VolumeView::fit(&volume);
        state.projection = Projection::Composite;
        state.quality = Quality::Refined;
        state.camera.yaw_degrees = 25.;
        state.camera.pitch_degrees = 15.;
        state.camera.roll_degrees = 7.;
        for size in [[512, 512], [2048, 2048]] {
            for lighting in [Lighting::UNLIT, Lighting::HEADLIGHT] {
                let run = Render {
                    gpu: &gpu,
                    state: &state,
                    material: &material,
                    lighting,
                    size,
                    peak: &peak,
                };
                let (samples, warmups) = if size[0] == 512 { (20, 3) } else { (4, 1) };
                let [original, dense_result, skipped] = compare(
                    |mode| {
                        run.run(
                            match mode {
                                0 => &mut legacy,
                                1 => &mut dense,
                                _ => &mut accelerated,
                            },
                            size[1],
                        )
                    },
                    samples,
                    warmups,
                )?;
                let a = read(
                    &gpu,
                    accelerated
                        .output_texture(0)
                        .ok_or("Missing accelerated output")?,
                )?;
                let b = read(&gpu, dense.output_texture(0).ok_or("Missing dense output")?)?;
                let old = read(
                    &gpu,
                    legacy.output_texture(0).ok_or("Missing original output")?,
                )?;
                if a != b {
                    return Err("Accelerated output differs from the dense reference".into());
                }
                let mut legacy_error = 0f64;
                for i in 0..size[0] as usize * size[1] as usize {
                    let value = pixel(&a, i)?;
                    let original = pixel(&old, i)?;
                    legacy_error = legacy_error.max((value[0] - original[0]).abs());
                }
                let mut reference_error = 0f64;
                if lighting == Lighting::UNLIT {
                    for y in 0..16 {
                        for x in 0..16 {
                            let pixel_x = (2 * x + 1) * size[0] as usize / 32;
                            let pixel_y = (2 * y + 1) * size[1] as usize / 32;
                            let index = pixel_x + pixel_y * size[0] as usize;
                            reference_error = reference_error.max(
                                (pixel(&a, index)?[0]
                                    - reference(&volume, &state, &material, size, index)?)
                                .abs(),
                            );
                        }
                    }
                    if reference_error > 0.003 {
                        return Err(format!("Independent optical reference error {reference_error} for {kind} {size:?}").into());
                    }
                }
                let tiles = if size[0] == 512 {
                    Some(measure(|| run.run(&mut accelerated, 16), 4, 1)?)
                } else {
                    None
                };
                if size[0] == 512
                    && read(
                        &gpu,
                        accelerated
                            .output_texture(0)
                            .ok_or("Missing tiled output")?,
                    )? != b
                {
                    return Err("Tiled acceleration changed sample phase".into());
                }
                cases.push(json!({"kind":kind,"size":size,"lighting":format!("{lighting:?}"),"quality":"Refined","state":state,
                "tiles":ranges.summaries().len(),"transparent_tiles":flags.transparent_tiles(),"range_cell_side":32,
                "source_upload_ms":source_upload_ms,"cpu_range_prepare_ms":cpu_prepare_ms,"gpu_classification_upload_ms":gpu_prepare_ms,"three_pipeline_prepare_ms":pipeline_ms,
                "legacy":original,"dense_reference":dense_result,"accelerated":skipped,"accelerated_complete_fenced_16_row_sequence":tiles,
                "all_pixels_match_dense_reference":true,"max_legacy_radiance_difference":legacy_error,
                "independent_unlit_reference_rays":if lighting==Lighting::UNLIT{256}else{0},"max_independent_unlit_reference_error":reference_error}));
                eprintln!(
                    "Verified {kind} {} {}",
                    size[0],
                    if lighting == Lighting::UNLIT {
                        "unlit"
                    } else {
                        "headlight"
                    }
                );
            }
        }
        drop(accelerated);
        drop(dense);
        drop(legacy);
        drop(flags);
        drop(ranges);
        drop(gpu);
        drop(volume);
        fence(&d, &q, d.create_command_encoder(&Default::default()))?;
        if cpu.used() != cpu_base || gpu_budget.used() != gpu_base {
            return Err("Rendering reservations were not restored".into());
        }
    }
    println!(
        "{}",
        serde_json::to_string_pretty(
            &json!({"schema":1,"adapter":format!("{info:?}"),"limits":format!("{:?}",d.limits()),
        "dimensions":[512,512,128],"unique_scalar_bytes":134217728,"material":material.points().iter().map(|p|json!([p.value,p.luminance,p.extinction_per_mm])).collect::<Vec<_>>(),
        "scope":"resident complete GPU-fenced frames with immutable uniforms; source/range/pipeline prep, readback, native input/display and RSS excluded; tile sequence fences every 16 rows",
        "skip_scope":"certified zero-extinction tile runs with guarded per-sample fallback; original midpoint count/phase retained; no residency",
        "peak_gpu_admission_bytes":peak.get()-gpu_base,"reservations_restored":true,"cases":cases})
        )?
    );
    Ok(())
}
