// SPDX-License-Identifier: GPL-3.0-or-later
//! Explicit local synthetic benchmark. Fences GPU work; no patient input, file I/O or network.
use orv_core::{
    Plane, ViewState, WindowLevel,
    oblique::ObliquePlane,
    slab::{Slab, SlabProjection},
    study::{Frame, Geometry},
    volume::Volume,
    volume_view::{Projection, Quality, VolumeView},
};
use orv_render::{
    reslice::GpuReslicer, slice::SliceRenderer, volume::VolumeRenderer,
    volume_source::GpuVolumeSource,
};
use serde_json::{Value, json};
use std::{
    error::Error,
    sync::Arc,
    time::{Duration, Instant},
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

fn source(padding: bool) -> Result<Arc<Volume>> {
    let frames = (0..128)
        .map(|z| {
            let pixels = (0..512)
                .flat_map(|y| {
                    (0..512).map(move |x| {
                        let r = ((x as f32 - 255.5) / 230.).powi(2)
                            + ((y as f32 - 255.5) / 190.).powi(2)
                            + ((z as f32 - 63.5) / 62.).powi(2);
                        if r > 1. {
                            -1000.
                        } else if r > 0.85 {
                            1000.
                        } else {
                            (x + y + z) as f32 - 300.
                        }
                    })
                })
                .collect::<Vec<_>>();
            Arc::new(Frame {
                presentation: None,
                calibration: None,
                projection: None,
                sop: None,
                slab: None,
                scalar: None,
                validity: padding.then(|| {
                    pixels
                        .iter()
                        .map(|v| u8::from(*v != -1000.))
                        .collect::<Vec<_>>()
                        .into()
                }),
                color: None,
                nifti: None,
                rgba: None,
                width: 512,
                height: 512,
                pixels: pixels.into(),
                instance: z,
                frame_index: 0,
                monochrome1: false,
                default_window: WindowLevel::BONE,
                geometry: Some(Geometry {
                    nifti_transform: None,
                    frame_of_reference: Some("synthetic benchmark".into()),
                    position: [0., 0., f64::from(z) * 1.2],
                    orientation: [1., 0., 0., 0., 1., 0.],
                    spacing: [0.7, 0.7],
                }),
            })
        })
        .collect();
    Ok(Arc::new(Volume::new(frames)?))
}
fn measure(mut work: impl FnMut(usize) -> Result<()>, count: usize) -> Result<Value> {
    for index in 0..4 {
        work(index)?;
    }
    let mut ms = Vec::with_capacity(count);
    for index in 0..count {
        let start = Instant::now();
        work(index + 4)?;
        ms.push(start.elapsed().as_secs_f64() * 1000.);
    }
    ms.sort_by(f64::total_cmp);
    Ok(
        json!({"samples":count,"median_ms":ms[count/2],"p95_ms":ms[(count*95).div_ceil(100)-1],"max_ms":ms[count-1]}),
    )
}
fn submit(device: &wgpu::Device, queue: &wgpu::Queue, encoder: wgpu::CommandEncoder) -> Result<()> {
    let index = queue.submit([encoder.finish()]);
    device.poll(wgpu::PollType::Wait {
        submission_index: Some(index),
        timeout: Some(Duration::from_secs(10)),
    })?;
    Ok(())
}
fn main() -> Result<()> {
    let padding = match std::env::args().nth(1).as_deref() {
        None => false,
        Some("--padding") => true,
        _ => return Err("Use no argument or --padding for this synthetic benchmark".into()),
    };
    let total = Instant::now();
    let backends = orv_render::configured_backends()?;
    let volume = source(padding)?;
    let generation_ms = total.elapsed().as_secs_f64() * 1000.;
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends,
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapter = pollster::block_on(instance.request_adapter(&Default::default()))?;
    let info = adapter.get_info();
    let (device, queue) =
        pollster::block_on(adapter.request_device(&orv_render::device_descriptor(&adapter)))?;
    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let output = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("synthetic benchmark target"),
        size: wgpu::Extent3d {
            width: 1600,
            height: 1000,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let target = output.create_view(&Default::default());
    let mut slice = SliceRenderer::new(&device, format);
    let mut display = ViewState {
        window: WindowLevel::BONE,
        ..Default::default()
    };
    let scalar = measure(
        |index| {
            let frame = &volume.frames()[index % volume.frames().len()];
            display.window.center = 300. + (index % 20) as f32;
            slice.upload(&device, &queue, frame)?;
            slice.set_view(&queue, frame, &display, 1.6)?;
            let mut encoder = device.create_command_encoder(&Default::default());
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("synthetic scalar paint"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &target,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                slice.paint(&mut pass);
            }
            submit(&device, &queue, encoder)
        },
        120,
    )?;
    let before_volume = orv_core::resources::MemoryBudget::gpu().used();
    let upload = Instant::now();
    let source = GpuVolumeSource::new(&device, &queue, volume.clone(), || false)?;
    let mut engine = VolumeRenderer::from_source(source.clone(), format, || false)?;
    let upload_ms = upload.elapsed().as_secs_f64() * 1000.;
    // Freeze display/camera state so each projection is measured on the same rays.
    display.window = WindowLevel::BONE;
    let mut cases = Vec::new();
    for projection in [Projection::Maximum, Projection::Composite] {
        let mut state = VolumeView::fit(&volume);
        state.projection = projection;
        state.camera.yaw_degrees = 25.;
        state.camera.pitch_degrees = 15.;
        state.quality = Quality::Interactive;
        let interactive = measure(
            |index| {
                state.camera.yaw_degrees = 25. + index as f64 * 0.1;
                let mut encoder = device.create_command_encoder(&Default::default());
                engine.render(&device, &mut encoder, &display, &state, [256, 256])?;
                {
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("synthetic volume paint"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &target,
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        ..Default::default()
                    });
                    engine.paint(&mut pass);
                }
                submit(&device, &queue, encoder)
            },
            120,
        )?;
        state.camera.yaw_degrees = 25.;
        state.quality = Quality::Refined;
        let tile = measure(
            |index| {
                let y = (index % 96) as u32 * 8;
                let mut encoder = device.create_command_encoder(&Default::default());
                engine.render_region(
                    &device,
                    &mut encoder,
                    &display,
                    &state,
                    [768, 768],
                    [[0, y], [768, 8]],
                )?;
                submit(&device, &queue, encoder)
            },
            120,
        )?;
        let complete = measure(
            |_| {
                let mut encoder = device.create_command_encoder(&Default::default());
                engine.render(&device, &mut encoder, &display, &state, [768, 768])?;
                submit(&device, &queue, encoder)
            },
            12,
        )?;
        let submitted_tiles = measure(
            |_| {
                for y in (0..768).step_by(8) {
                    let mut encoder = device.create_command_encoder(&Default::default());
                    engine.render_region(
                        &device,
                        &mut encoder,
                        &display,
                        &state,
                        [768, 768],
                        [[0, y], [768, 8]],
                    )?;
                    submit(&device, &queue, encoder)?;
                }
                Ok(())
            },
            12,
        )?;
        cases.push(json!({
            "projection": projection,
            "view": display,
            "refined_state": state,
            "interactive_size":[256,256],"refined_size":[768,768],
            "interactive_and_paint":interactive,
            "refined_8row_compute":tile,
            "refined_complete_compute":complete,
            "refined_complete_96_fenced_tiles":submitted_tiles,
            "tile_scope":"96 sequential submissions with completion fences; no native event/frame pacing"
        }));
    }
    let mut reslicer = GpuReslicer::new(source.clone())?;
    let mut reslice_cases = Vec::new();
    for (name, slab, count) in [
        ("thin", None, 120),
        (
            "maximum_65_samples",
            Some(Slab {
                projection: SlabProjection::Maximum,
                thickness_mm: 20.,
                intervals: 64,
            }),
            30,
        ),
        (
            "mean_257_samples",
            Some(Slab {
                projection: SlabProjection::Mean,
                thickness_mm: 60.,
                intervals: 256,
            }),
            12,
        ),
    ] {
        let planes = [Plane::Axial, Plane::Coronal, Plane::Sagittal];
        let grids = planes.map(|plane| {
            let plane = ObliquePlane::anatomical(plane, volume.center())?;
            volume.oblique_grid(&plane, 512, slab)
        });
        let dimensions = grids
            .into_iter()
            .map(|grid| grid.map(|g| g.dimensions()))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let timing = measure(
            |index| {
                let mut center = volume.center();
                // Move all planes; each submission computes the complete requested
                // grids. Reusing unchanged panes is deliberately outside this case.
                for coordinate in &mut center {
                    *coordinate += ((index % 20) as f64 - 10.) * 0.1;
                }
                let mut encoder = device.create_command_encoder(&Default::default());
                for (slot, plane) in planes.into_iter().enumerate() {
                    let plane = ObliquePlane::anatomical(plane, center)?;
                    let grid = volume.oblique_grid(&plane, 512, slab)?;
                    reslicer.render(&mut encoder, &grid, slot)?;
                }
                submit(&device, &queue, encoder)
            },
            count,
        )?;
        reslice_cases.push(json!({"kind":name,"requested_side":512,"grid_dimensions":dimensions,"slab":slab,"three_complete_planes":timing}));
    }
    let adapter_limits = adapter.limits();
    let device_limits = device.limits();
    let reserved = orv_core::resources::MemoryBudget::gpu().used();
    drop(engine);
    drop(reslicer);
    drop(source);
    submit(
        &device,
        &queue,
        device.create_command_encoder(&Default::default()),
    )?;
    let after_drop = orv_core::resources::MemoryBudget::gpu().used();
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "schema_version":3,"version":env!("CARGO_PKG_VERSION"),"optimized":!cfg!(debug_assertions),
            "adapter":info.name,"backend":format!("{:?}",info.backend),
            "adapter_limits":{"texture_2d":adapter_limits.max_texture_dimension_2d,"texture_3d":adapter_limits.max_texture_dimension_3d},
            "requested_device_limits":{"texture_2d":device_limits.max_texture_dimension_2d,"texture_3d":device_limits.max_texture_dimension_3d},
            "source_dimensions":[512,512,128],"retained_scalar_bytes":512*512*128*4,
            "retained_validity_bytes":if padding {512*512*128} else {0},
            "paint_target_size":[1600,1000],"warmup_per_case":4,
            "scope":"CPU encode/upload/submit through GPU completion fence; excludes OS input, display/vsync, file decode and PNG readback",
            "memory_scope":"Shared GPU admission reservations, not measured allocation, free VRAM or RSS; external paint target is additional",
            "gpu_reserved_before_volume_bytes":before_volume,
            "gpu_reserved_after_refinement_bytes":reserved,"gpu_reserved_after_renderer_drop_bytes":after_drop,
            "paint_target_bytes":1600*1000*4,
            "synthetic_generation_ms":generation_ms,"volume_upload_and_pipeline_ms":upload_ms,
            "scalar_scroll":scalar,"cases":cases,"reslice_cases":reslice_cases,
            "reslice_scope":"Three moving full-detail scalar grids on the shared uploaded source; no readback, contrast mapping, native presentation or input timing"
        }))?
    );
    Ok(())
}
