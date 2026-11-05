// SPDX-License-Identifier: GPL-3.0-or-later
//! Generated, complete source-section images; excludes native input/presentation.
#[path = "fixtures/optical.rs"]
mod optical;
use orv_core::{
    Plane, ViewState, WindowLevel,
    resources::{MemoryBudget, Reservation},
    volume_material::{MaterialPoint, VolumeMaterial},
    volume_view::{Projection, Quality, VolumeView},
};
use orv_render::{
    GpuReservation,
    material_volume::Lighting,
    reslice::GpuReslicer,
    section_volume::{SectionImage, SectionPlane, SectionRenderer, SectionScene},
    volume_source::GpuVolumeSource,
};
use serde_json::{Value, json};
use std::{
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
fn fence(d: &wgpu::Device) -> Result<()> {
    d.poll(wgpu::PollType::Wait {
        submission_index: None,
        timeout: Some(Duration::from_secs(60)),
    })?;
    Ok(())
}
struct Readback {
    pixels: Vec<[u16; 4]>,
    _cpu: Reservation,
}
fn read(source: &GpuVolumeSource, image: &SectionImage) -> Result<Readback> {
    let size = image.texture().size();
    let stride = (size.width * 8).next_multiple_of(256);
    let bytes = u64::from(stride) * u64::from(size.height);
    let _gpu = GpuReservation::new(bytes + 65536, source.device(), source.queue())?;
    let cpu = MemoryBudget::volumes()
        .reserve(bytes * 2)
        .ok_or("Section verification staging refused")?;
    let buffer = source.device().create_buffer(&wgpu::BufferDescriptor {
        label: Some("generated section benchmark readback"),
        size: bytes,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = source.device().create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        image.texture().as_image_copy(),
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
    source.queue().submit([encoder.finish()]);
    let (tx, rx) = mpsc::sync_channel(1);
    buffer.map_async(wgpu::MapMode::Read, .., move |r| {
        let _ = tx.send(r);
    });
    fence(source.device())?;
    rx.recv_timeout(Duration::from_secs(10))??;
    let mapped = buffer.get_mapped_range(..)?;
    let mut result = Vec::new();
    result.try_reserve_exact((size.width * size.height) as usize)?;
    for row in mapped.chunks_exact(stride as usize) {
        for p in row[..size.width as usize * 8].as_chunks::<8>().0 {
            let pixel = std::array::from_fn(|a| u16::from_le_bytes([p[a * 2], p[a * 2 + 1]]));
            if pixel[0] != pixel[1]
                || pixel[0] != pixel[2]
                || pixel[0] > 0x3c00
                || pixel[3] != 0x3c00
            {
                return Err("Invalid generated section radiance".into());
            }
            result.push(pixel);
        }
    }
    drop(mapped);
    buffer.unmap();
    Ok(Readback {
        pixels: result,
        _cpu: cpu,
    })
}
fn render(
    renderer: &mut SectionRenderer,
    scene: &SectionScene,
    rows: u32,
    source: &GpuVolumeSource,
) -> Result<Arc<SectionImage>> {
    let mut image = None;
    for y in (0..scene.size[1]).step_by(rows as usize) {
        image = renderer.render_rows(scene, 0, y..(y + rows).min(scene.size[1]), || false)?;
        if image.is_none() {
            fence(source.device())?;
        }
    }
    image.ok_or_else(|| "Section image did not complete".into())
}
fn measure(
    source: &GpuVolumeSource,
    renderer: &mut SectionRenderer,
    scene: &SectionScene,
    rows: u32,
    samples: usize,
    warmups: usize,
) -> Result<(Value, Arc<SectionImage>)> {
    let mut previous = None;
    let mut times = Vec::new();
    for i in 0..warmups + samples {
        let start = Instant::now();
        let next = render(renderer, scene, rows, source)?;
        previous = Some(next);
        fence(source.device())?;
        if i >= warmups {
            times.push(start.elapsed().as_secs_f64() * 1000.);
        }
    }
    times.sort_by(f64::total_cmp);
    Ok((
        json!({"samples":samples,"warmups":warmups,"median_ms":times[samples/2],"p95_ms":times[(samples*95).div_ceil(100)-1],
        "max_ms":times[samples-1],"row_batches":scene.size[1].div_ceil(rows)}),
        previous.ok_or("Section timing image unavailable")?,
    ))
}
fn main() -> Result<()> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: orv_render::configured_backends()?,
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapter = pollster::block_on(instance.request_adapter(&Default::default()))?;
    let (device, queue) =
        pollster::block_on(adapter.request_device(&orv_render::device_descriptor(&adapter)))?;
    let before = [MemoryBudget::volumes().used(), MemoryBudget::gpu().used()];
    let mut cases = Vec::new();
    for kind in ["dense", "missing"] {
        let cpu = optical::source(kind)?;
        let start = Instant::now();
        let gpu = GpuVolumeSource::new(&device, &queue, cpu.clone(), || false)?;
        let upload_ms = start.elapsed().as_secs_f64() * 1000.;
        let start = Instant::now();
        let mut reslice = GpuReslicer::new(gpu.clone())?;
        let grids = [Plane::Axial, Plane::Coronal, Plane::Sagittal]
            .map(|p| cpu.reformat_grid(p, cpu.center(), 512))
            .into_iter()
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let planes = reslice
            .render_frames([Some(&grids[0]), Some(&grids[1]), Some(&grids[2])], || {
                false
            })?;
        let prepare_planes_ms = start.elapsed().as_secs_f64() * 1000.;
        let start = Instant::now();
        let mut renderer = SectionRenderer::new(gpu.clone(), wgpu::TextureFormat::Rgba8UnormSrgb)?;
        let pipeline_ms = start.elapsed().as_secs_f64() * 1000.;
        let material = Arc::new(VolumeMaterial::new(&[-1e20, 1e20].map(|value| {
            MaterialPoint {
                value,
                luminance: 0.3,
                extinction_per_mm: 0.02,
            }
        }))?);
        let mut scene = SectionScene {
            planes: planes.map(|image| {
                image.map(|image| SectionPlane {
                    image,
                    window: WindowLevel::source_range(-1000., 1800.),
                    invert: false,
                })
            }),
            navigation: ViewState::default(),
            volume: VolumeView {
                projection: Projection::Composite,
                quality: Quality::Refined,
                ..VolumeView::fit(&cpu)
            },
            material,
            lighting: Lighting::UNLIT,
            size: [512, 512],
        };
        scene.volume.camera.yaw_degrees = 23.;
        scene.volume.camera.pitch_degrees = -17.;
        scene.volume.camera.roll_degrees = 11.;
        let mut sizes = Vec::new();
        for (side, samples, warmups) in [(512, 12, 3), (2048, 3, 1)] {
            scene.size = [side, side];
            let (full, a) = measure(&gpu, &mut renderer, &scene, side, samples, warmups)?;
            let (tiled, b) = measure(
                &gpu,
                &mut renderer,
                &scene,
                16,
                if side == 512 { 4 } else { 2 },
                1,
            )?;
            let a = read(&gpu, &a)?;
            let b = read(&gpu, &b)?;
            if a.pixels != b.pixels {
                return Err("Section row composition differs from complete dispatch".into());
            }
            sizes.push(json!({"size":[side,side],"full":full,"rows16":tiled,"verified_pixels":a.pixels.len(),"all_row_pixels_equal":true,
                "admission_after_complete":{"cpu_bytes":MemoryBudget::volumes().used(),"gpu_bytes":MemoryBudget::gpu().used()}}));
        }
        cases.push(json!({"source":kind,"dimensions":[512,512,128],"unique_scalar_planes":true,"source_upload_ms":upload_ms,"completed_mpr_preparation_ms":prepare_planes_ms,
            "pipeline_ms":pipeline_ms,"sizes":sizes}));
    }
    fence(&device)?;
    let after = [MemoryBudget::volumes().used(), MemoryBudget::gpu().used()];
    if before != after {
        return Err("Section benchmark did not release its reservations".into());
    }
    println!(
        "{}",
        serde_json::to_string_pretty(
            &json!({"schema":1,"adapter":adapter.get_info().name,"cases":cases,
        "scope":"resident corrected thin plane textures; complete immutable source composition, uniforms, status readback, previous-front retention, old-front drop and retirement fence; individually fenced 16-row sequences; source/plane/pipeline preparation separate; verification readback and native input/presentation excluded",
        "verification":"all pixels finite monochrome [0,1] with opaque alpha; full and complete row images bit-equal; independent f64 optical/plane references are exercised by section_volume GPU tests",
        "admission_before":before,"admission_after":after,"admission_is_rss_or_driver_memory":false})
        )?
    );
    Ok(())
}
