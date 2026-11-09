// SPDX-License-Identifier: GPL-3.0-or-later
//! Complete generated paged optical images, including streaming and status fences.
#[path = "fixtures/optical.rs"]
mod optical;
#[path = "fixtures/paged_wide.rs"]
mod wide;
use orv_core::{
    ViewState,
    bricks::BrickStencil,
    resources::{MemoryBudget, Reservation},
    volume_material::{MaterialPoint, VolumeMaterial},
    volume_ranges::VolumeRanges,
    volume_view::{Projection, Quality, VolumeView},
};
use orv_render::{
    GpuReservation,
    brick_source::{GpuBrickAtlasPool, GpuBrickSource},
    material_volume::{Lighting, MaterialVolumeRenderer},
    paged_material::{
        PagedMaterialImage, PagedMaterialRenderer, PagedMaterialScene, PagedRenderStatistics,
    },
    paged_material_ranges::GpuPagedMaterialRanges,
    volume::VolumeTarget,
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
        timeout: Some(Duration::from_secs(30)),
    })?;
    Ok(())
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
fn read(d: &wgpu::Device, q: &wgpu::Queue, t: &wgpu::Texture) -> Result<(Vec<u8>, Reservation)> {
    let size = t.size();
    let stride = (size.width * 8).next_multiple_of(256);
    let bytes = u64::from(stride) * u64::from(size.height);
    let _gpu = GpuReservation::new(bytes, d, q)?;
    let cpu = MemoryBudget::volumes()
        .reserve(bytes * 2)
        .ok_or("Generated image readback admission")?;
    let buffer = d.create_buffer(&wgpu::BufferDescriptor {
        label: Some("generated complete image comparison"),
        size: bytes,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = d.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        t.as_image_copy(),
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
    q.submit([encoder.finish()]);
    let (tx, rx) = mpsc::sync_channel(1);
    buffer.map_async(wgpu::MapMode::Read, .., move |r| {
        let _ = tx.send(r);
    });
    fence(d)?;
    rx.recv_timeout(Duration::from_secs(10))??;
    let mapped = buffer.get_mapped_range(..)?;
    let mut out = Vec::new();
    out.try_reserve_exact(size.width as usize * size.height as usize * 8)?;
    for row in mapped.chunks(stride as usize) {
        for p in row[..size.width as usize * 8].as_chunks::<8>().0 {
            let values: [u16; 4] =
                std::array::from_fn(|i| u16::from_le_bytes([p[i * 2], p[i * 2 + 1]]));
            if values[..3]
                .iter()
                .any(|v| *v & 0x8000 != 0 || *v & 0x7c00 == 0x7c00)
                || values[3] != 0x3c00
            {
                return Err("Generated image is not finite opaque grayscale".into());
            }
            out.extend_from_slice(p);
        }
    }
    drop(mapped);
    buffer.unmap();
    Ok((out, cpu))
}
fn stats(s: PagedRenderStatistics) -> Value {
    json!({"tile_batches":s.tile_batches,"fenced_waves":s.fenced_waves,"source_pages_uploaded":s.source_pages_uploaded,"source_bytes_uploaded":s.source_bytes_uploaded,"atlas_pages_copied":s.atlas_pages_copied,"atlas_bytes_copied":s.atlas_bytes_copied})
}
fn complete(
    r: &mut PagedMaterialRenderer,
    s: &PagedMaterialScene,
) -> Result<Arc<PagedMaterialImage>> {
    Ok(r.render_complete(s, 0, || false)?)
}
fn main() -> Result<()> {
    let quick = std::env::args().skip(1).any(|v| v == "--quick");
    let classify = std::env::args().skip(1).any(|v| v == "--classified");
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        flags: orv_render::instance_flags(),
        backends: orv_render::configured_backends()?,
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapter = pollster::block_on(instance.request_adapter(&Default::default()))?;
    let info = adapter.get_info();
    let (d, q) =
        pollster::block_on(adapter.request_device(&orv_render::device_descriptor(&adapter)))?;
    if std::env::args().skip(1).any(|v| v == "--wide") {
        if classify {
            return Err(
                "Use --classified without --wide for the generated optical comparison".into(),
            );
        }
        println!(
            "{}",
            serde_json::to_string_pretty(
                &json!({"schema":1,"adapter":{"name":info.name,"backend":format!("{:?}",info.backend)},"wide":wide::run(&d,&q)?})
            )?
        );
        return Ok(());
    }
    let gpu = MemoryBudget::gpu();
    let ram = MemoryBudget::volumes();
    let before_gpu = gpu.used();
    let before_cpu = ram.used();
    let mut cases = Vec::new();
    let inputs = if quick {
        vec![("sparse", Lighting::UNLIT, 512)]
    } else {
        vec![
            ("sparse", Lighting::UNLIT, 512),
            ("sparse", Lighting::HEADLIGHT, 512),
            ("missing", Lighting::HEADLIGHT, 512),
            ("dense", Lighting::HEADLIGHT, 512),
            ("sparse", Lighting::HEADLIGHT, 2048),
        ]
    };
    for (kind, lighting, side) in inputs {
        let cpu = optical::source(kind)?;
        let source = GpuBrickSource::new(
            &d,
            &q,
            cpu.clone(),
            64,
            BrickStencil::CentralSecants,
            || false,
        )?;
        let material = Arc::new(VolumeMaterial::new(&[
            MaterialPoint {
                value: -1000.,
                luminance: 0.05,
                extinction_per_mm: 0.,
            },
            MaterialPoint {
                value: -700.,
                luminance: 0.1,
                extinction_per_mm: 0.,
            },
            MaterialPoint {
                value: 100.,
                luminance: 0.7,
                extinction_per_mm: 0.003,
            },
            MaterialPoint {
                value: 1400.,
                luminance: 0.2,
                extinction_per_mm: 0.025,
            },
        ])?);
        let mut volume = VolumeView::fit(&cpu);
        volume.projection = Projection::Composite;
        volume.quality = Quality::Refined;
        volume.camera.yaw_degrees = 23.;
        volume.camera.pitch_degrees = -17.;
        volume.camera.roll_degrees = 11.;
        let scene = PagedMaterialScene {
            source: source.clone(),
            navigation: ViewState::default(),
            volume,
            material,
            lighting,
            size: [side, side],
        };
        let (classification, preparation) = if classify {
            let start = Instant::now();
            let ranges = VolumeRanges::build(cpu.clone(), 32, || false)?;
            let ranges_ms = start.elapsed().as_secs_f64() * 1000.;
            let start = Instant::now();
            let flags = GpuPagedMaterialRanges::new(
                source.clone(),
                ranges.clone(),
                scene.material.clone(),
                || false,
            )?;
            let upload_ms = start.elapsed().as_secs_f64() * 1000.;
            let report = json!({"range_cell_side":32,"range_tiles":ranges.summaries().len(),"transparent_tiles":flags.transparent_tiles(),"required_source_pages":flags.required_pages(),"cpu_summary_build_ms_one_sample":ranges_ms,"gpu_classification_upload_and_fence_ms_one_sample":upload_ms});
            (Some(flags), report)
        } else {
            (None, Value::Null)
        };
        let dense_source = GpuVolumeSource::new(&d, &q, cpu.clone(), || false)?;
        let mut dense = MaterialVolumeRenderer::new_dense_reference(
            dense_source,
            wgpu::TextureFormat::Rgba8Unorm,
        )?;
        let dense_timing = measure(
            || {
                let mut e = d.create_command_encoder(&Default::default());
                dense.render_target(
                    &mut e,
                    &scene.navigation,
                    &scene.volume,
                    &scene.material,
                    lighting,
                    VolumeTarget {
                        slot: 0,
                        size: scene.size,
                        region: [[0, 0], scene.size],
                    },
                )?;
                q.submit([e.finish()]);
                fence(&d)
            },
            if quick {
                2
            } else if side == 512 {
                12
            } else {
                2
            },
            if side == 512 { 3 } else { 1 },
        )?;
        let (reference, _reference_cpu) = read(
            &d,
            &q,
            dense.output_texture(0).ok_or("Dense output missing")?,
        )?;
        let mut paths = Vec::new();
        let mut capacities = if side == 512 {
            vec![8, source.layout().len()]
        } else {
            vec![source.layout().len()]
        };
        if side == 512
            && let Some(flags) = &classification
            && flags.required_pages() > 8
            && flags.required_pages() < source.layout().len()
        {
            capacities.insert(1, flags.required_pages());
        }
        for (capacity, accelerated) in capacities.into_iter().flat_map(|c| {
            if classify {
                vec![(c, false), (c, true)]
            } else {
                vec![(c, false)]
            }
        }) {
            eprintln!(
                "Generated paged case {kind} side{side} slots{capacity} classified{accelerated}"
            );
            let pool = GpuBrickAtlasPool::new(source.clone(), capacity, || false)?;
            let atlas_admission = pool.admission_bytes();
            let mut renderer = if accelerated {
                PagedMaterialRenderer::new_accelerated(
                    pool,
                    classification
                        .as_ref()
                        .ok_or("Missing generated classification")?
                        .clone(),
                    wgpu::TextureFormat::Rgba8Unorm,
                    || false,
                )?
            } else {
                PagedMaterialRenderer::new(pool, wgpu::TextureFormat::Rgba8Unorm, || false)?
            };
            let start = Instant::now();
            let mut current = complete(&mut renderer, &scene)?;
            let cold_ms = start.elapsed().as_secs_f64() * 1000.;
            let cold_stats = stats(current.statistics());
            let (initial, _initial_cpu) = read(&d, &q, current.texture())?;
            if initial != reference {
                return Err("Cold paged image differs from dense reference".into());
            }
            let mut observed_gpu = gpu.used();
            let mut observed_cpu = ram.used();
            let warm = measure(
                || {
                    let next = complete(&mut renderer, &scene)?;
                    observed_gpu = observed_gpu.max(gpu.used());
                    observed_cpu = observed_cpu.max(ram.used());
                    current = next;
                    fence(&d)
                },
                if quick {
                    2
                } else if capacity < source.layout().len()
                    && (!accelerated
                        || classification
                            .as_ref()
                            .is_none_or(|r| r.required_pages() > capacity))
                {
                    3
                } else if side == 512 {
                    12
                } else {
                    2
                },
                if capacity < source.layout().len()
                    && (!accelerated
                        || classification
                            .as_ref()
                            .is_none_or(|r| r.required_pages() > capacity))
                {
                    1
                } else {
                    3
                },
            )?;
            let (final_pixels, _final_cpu) = read(&d, &q, current.texture())?;
            if final_pixels != reference {
                return Err("Warm paged image differs from dense reference".into());
            }
            let final_stats = stats(current.statistics());
            paths.push(json!({"classified":accelerated,"atlas_slots":capacity,"atlas_admission_bytes":atlas_admission,"cold_first_complete_image_ms":cold_ms,"cold_first_statistics":cold_stats,"warm_complete_images":warm,"last_warm_statistics":final_stats,"verified_pixels_each_initial_final":u64::from(side)*u64::from(side),"observed_after_complete_image_gpu_admission_bytes":observed_gpu,"observed_after_complete_image_cpu_admission_bytes":observed_cpu}));
            drop(current);
            drop(renderer);
            fence(&d)?;
        }
        cases.push(json!({"source_kind":kind,"source_dimensions":[512,512,128],"output_size":scene.size,"lighting":if lighting==Lighting::UNLIT{"unlit"}else{"headlight"},"core_cell_side":64,"source_layout_pages":source.layout().len(),"classification_preparation":preparation,"dense_cached_dispatch_and_fence":dense_timing,"paged_paths":paths}));
        drop((classification, dense, scene, source, cpu));
        fence(&d)?;
    }
    fence(&d)?;
    if gpu.used() != before_gpu || ram.used() != before_cpu {
        return Err("Paged example did not restore all app admission".into());
    }
    println!(
        "{}",
        serde_json::to_string_pretty(
            &json!({"schema":if classify {2}else{1},"quick":quick,"classified_comparison":classify,"adapter":{"name":info.name,"backend":format!("{:?}",info.backend)},"dense_scope":"cached pipeline, dense source and output; dispatch and fence only","paged_scope":"cached pipeline and page pool; new immutable output/state, one required-resident dispatch or constrained16-row waves, page uploads/cache replacement, status readback, prior-front drop and retirement fence; initial/final full pixel verification excluded","cold_scope":"first complete worker image after renderer/empty atlas construction; one sample","excludes":"source generation, dense upload, range classification, pipeline/pool construction, verification, native input/presentation; no native FPS claim","admission_scope":"app reservations sampled after complete images, excluding transient peaks, driver memory and RSS","before":{"gpu":before_gpu,"cpu":before_cpu},"after":{"gpu":gpu.used(),"cpu":ram.used()},"cases":cases})
        )?
    );
    Ok(())
}
