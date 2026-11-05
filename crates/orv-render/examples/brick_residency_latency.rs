// SPDX-License-Identifier: GPL-3.0-or-later
//! Generated source-page upload/reuse timings, without a bricked image renderer.
#[path = "fixtures/optical.rs"]
mod optical;
use orv_core::{bricks::BrickStencil, resources::MemoryBudget};
use orv_render::{
    GpuReservation,
    brick_source::{
        BRICK_SAMPLING_SHADER, GpuBrickResidency, GpuBrickSource, MAX_RESIDENT_BRICK_BYTES,
    },
};
use serde_json::{Value, json};
use std::{
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
fn fence(device: &wgpu::Device) -> Result<()> {
    device.poll(wgpu::PollType::Wait {
        submission_index: None,
        timeout: Some(Duration::from_secs(30)),
    })?;
    Ok(())
}
fn measure(
    mut work: impl FnMut(usize) -> Result<()>,
    samples: usize,
    warmups: usize,
) -> Result<Value> {
    for i in 0..warmups {
        work(i)?;
    }
    let mut times = Vec::new();
    for i in warmups..warmups + samples {
        let start = Instant::now();
        work(i)?;
        times.push(start.elapsed().as_secs_f64() * 1000.);
    }
    times.sort_by(f64::total_cmp);
    Ok(
        json!({"samples":samples,"warmups":warmups,"median_ms":times[samples/2],
        "p95_ms":times[(samples*95).div_ceil(100)-1],"max_ms":times[samples-1]}),
    )
}
struct Verify {
    layout: wgpu::BindGroupLayout,
    pipeline: wgpu::ComputePipeline,
    _gpu: GpuReservation,
}
impl Verify {
    fn new(source: &GpuBrickSource) -> Result<Self> {
        let device = source.device();
        let gpu = GpuReservation::new(65536, device, source.queue())?;
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("generated source page verification output"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: false },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let text = format!(
            "{BRICK_SAMPLING_SHADER}\n{}",
            include_str!("fixtures/brick_readback.wgsl")
        );
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("generated all-texel source page readback"),
            source: wgpu::ShaderSource::Wgsl(text.into()),
        });
        let p = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(source.binding_layout()), Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: None,
            layout: Some(&p),
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        Ok(Self {
            layout,
            pipeline,
            _gpu: gpu,
        })
    }
    fn all_texels(&self, resident: &GpuBrickResidency) -> Result<u64> {
        let source = resident.source();
        let device = source.device();
        let queue = source.queue();
        let mut verified = 0;
        for i in 0..source.layout().len() {
            let Some(page) = resident.page(i) else {
                continue;
            };
            let brick = page.brick();
            let count = brick.size.into_iter().product::<usize>();
            let bytes = count as u64 * 8;
            let _gpu = GpuReservation::new(bytes * 2, device, queue)?;
            let _cpu = MemoryBudget::volumes()
                .reserve(bytes)
                .ok_or("Source readback admission")?;
            let output = device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: bytes,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            });
            let read = device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: bytes,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &self.layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: output.as_entire_binding(),
                }],
            });
            let mut encoder = device.create_command_encoder(&Default::default());
            {
                let mut pass = encoder.begin_compute_pass(&Default::default());
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, page.bindings(), &[]);
                pass.set_bind_group(1, &group, &[]);
                pass.dispatch_workgroups(
                    (brick.size[0] as u32).div_ceil(4),
                    (brick.size[1] as u32).div_ceil(4),
                    (brick.size[2] as u32).div_ceil(4),
                );
            }
            encoder.copy_buffer_to_buffer(&output, 0, &read, 0, bytes);
            queue.submit([encoder.finish()]);
            let (tx, rx) = mpsc::sync_channel(1);
            read.map_async(wgpu::MapMode::Read, .., move |v| {
                let _ = tx.send(v);
            });
            fence(device)?;
            rx.recv_timeout(Duration::from_secs(10))??;
            let mapped = read.get_mapped_range(..)?;
            let sw = source.source().dimensions()[0];
            for (local, raw) in mapped.as_chunks::<8>().0.iter().enumerate() {
                let x = local % brick.size[0] + brick.origin[0];
                let y = local / brick.size[0] % brick.size[1] + brick.origin[1];
                let z = local / (brick.size[0] * brick.size[1]) + brick.origin[2];
                let frame = &source.source().frames()[z];
                let index = y * sw + x;
                let value = u32::from_le_bytes(raw[..4].try_into()?);
                let valid = u32::from_le_bytes(raw[4..].try_into()?);
                if value != frame.pixels[index].to_bits()
                    || valid != u32::from(frame.has_sample(index))
                {
                    return Err("A retained GPU page texel differs from its original source".into());
                }
            }
            verified += count as u64;
            drop(mapped);
            read.unmap();
        }
        Ok(verified)
    }
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
    let gpu = MemoryBudget::gpu();
    let ram = MemoryBudget::volumes();
    let before_gpu = gpu.used();
    let before_cpu = ram.used();
    let mut cases = Vec::new();
    for kind in ["sparse", "missing"] {
        let cpu = optical::source(kind)?;
        for (side, samples) in [(64, 40), (128, 12)] {
            let source = GpuBrickSource::new(
                &device,
                &queue,
                cpu.clone(),
                side,
                BrickStencil::CentralSecants,
                || false,
            )?;
            let baseline = gpu.used();
            let fresh = measure(
                |_| {
                    let resident = GpuBrickResidency::prepare(
                        source.clone(),
                        &[0, 1, 2, 3],
                        None,
                        MAX_RESIDENT_BRICK_BYTES,
                        || false,
                    )?;
                    if resident.statistics().uploaded_pages != 4
                        || resident.statistics().reused_pages != 0
                    {
                        return Err("Fresh source-page accounting differs".into());
                    }
                    drop(resident);
                    source.wait_submitted(|| false)?;
                    if gpu.used() != baseline {
                        return Err("Fresh page admission was not retired".into());
                    }
                    Ok(())
                },
                3,
                1,
            )?;
            let mut current = GpuBrickResidency::prepare(
                source.clone(),
                &[0, 1, 2, 3],
                None,
                MAX_RESIDENT_BRICK_BYTES,
                || false,
            )?;
            let verify = Verify::new(&source)?;
            let mut verified_texels = verify.all_texels(&current)?;
            let initial_bytes = current.statistics().uploaded_bytes;
            let reuse = measure(
                |_| {
                    let ids: &[usize] = if current.page(0).is_some() {
                        &[0, 1, 2, 3]
                    } else {
                        &[1, 2, 3, 4]
                    };
                    let next = GpuBrickResidency::prepare(
                        source.clone(),
                        ids,
                        Some(&current),
                        MAX_RESIDENT_BRICK_BYTES,
                        || false,
                    )?;
                    if next.statistics().reused_pages != 4 || next.statistics().uploaded_bytes != 0
                    {
                        return Err("Unchanged source pages were uploaded again".into());
                    }
                    current = next;
                    source.wait_submitted(|| false)?;
                    Ok(())
                },
                samples,
                3,
            )?;
            let mut observed_gpu = gpu.used();
            let mut observed_cpu = ram.used();
            let mut last_stats = current.statistics();
            let replace = measure(
                |_| {
                    let ids: &[usize] = if current.page(0).is_some() {
                        &[1, 2, 3, 4]
                    } else {
                        &[0, 1, 2, 3]
                    };
                    let next = GpuBrickResidency::prepare(
                        source.clone(),
                        ids,
                        Some(&current),
                        MAX_RESIDENT_BRICK_BYTES,
                        || false,
                    )?;
                    let stats = next.statistics();
                    if stats.reused_pages != 3 || stats.uploaded_pages != 1 {
                        return Err("One-page replacement did not reuse exactly three pages".into());
                    }
                    for i in [1, 2, 3] {
                        if !Arc::ptr_eq(
                            current.page(i).ok_or("Prior shared page")?,
                            next.page(i).ok_or("New shared page")?,
                        ) {
                            return Err("Shared pages changed during replacement".into());
                        }
                    }
                    observed_gpu = observed_gpu.max(gpu.used());
                    observed_cpu = observed_cpu.max(ram.used());
                    last_stats = stats;
                    current = next;
                    source.wait_submitted(|| false)?;
                    Ok(())
                },
                samples,
                3,
            )?;
            // The odd warmup+sample count ends on [1,2,3,4], proving the extra page.
            if current.page(4).is_none() {
                return Err("Expected final replacement page".into());
            }
            verified_texels += verify.all_texels(&current)?;
            let stats = json!({"reused_pages":last_stats.reused_pages,"uploaded_pages":last_stats.uploaded_pages,
                "raw_uploaded_bytes":last_stats.uploaded_bytes,"referenced_page_admission_bytes":last_stats.referenced_bytes});
            drop(current);
            drop(verify);
            source.wait_submitted(|| false)?;
            if gpu.used() != baseline {
                return Err("Source-page resources were not retired".into());
            }
            cases.push(json!({"source_kind":kind,"source_dimensions":[512,512,128],"core_cell_side":side,
                "layout_pages":source.layout().len(),"stored_halo_before":1,"stored_halo_after":3,
                "fresh_four_pages":fresh,"fresh_raw_upload_bytes":initial_bytes,
                "unchanged_four_pages":reuse,"replace_one_of_four_pages":replace,"last_replacement":stats,
                "verified_stored_texels":verified_texels,"observed_before_replacement_gpu_admission_bytes":observed_gpu,
                "observed_before_replacement_cpu_admission_bytes":observed_cpu}));
            drop(source);
            fence(&device)?;
        }
        drop(cpu);
    }
    fence(&device)?;
    if gpu.used() != before_gpu || ram.used() != before_cpu {
        return Err("Source-page example did not restore admission".into());
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({"schema":1,
            "adapter":{"name":info.name,"backend":format!("{:?}",info.backend)},
            "requested_max_texture_dimension_3d":device.limits().max_texture_dimension_3d,
            "scope":"complete worker upload/reuse/replacement and retirement fences; source/pipeline preparation, verification, images and native display excluded",
            "admission_scope":"app reservations, not RSS/VRAM; observed replacement values exclude transient staging peaks",
            "source_cpu_storage":"unique f32 planes, optional validity; dense CPU source retained, no dense GPU source",
            "before":{"gpu":before_gpu,"cpu":before_cpu},"after":{"gpu":gpu.used(),"cpu":ram.used()},"cases":cases
        }))?
    );
    Ok(())
}
