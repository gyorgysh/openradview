// SPDX-License-Identifier: GPL-3.0-or-later
//! Explicit local synthetic benchmark. Fences GPU work; no patient input, file I/O or network.
use orv_core::{
    ViewState, WindowLevel,
    study::{Frame, Geometry},
    volume::Volume,
    volume_view::{Quality, VolumeView},
};
use orv_render::{slice::SliceRenderer, volume::VolumeRenderer};
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
    let volume = source(padding)?;
    let generation_ms = total.elapsed().as_secs_f64() * 1000.;
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: orv_render::native_backends(),
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapter = pollster::block_on(instance.request_adapter(&Default::default()))?;
    let info = adapter.get_info();
    let (device, queue) = pollster::block_on(adapter.request_device(&Default::default()))?;
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
    let upload = Instant::now();
    let mut engine = VolumeRenderer::new(&device, &queue, volume.clone(), format, || false)?;
    let upload_ms = upload.elapsed().as_secs_f64() * 1000.;
    let mut state = VolumeView::fit(&volume);
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
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                engine.paint(&mut pass);
            }
            submit(&device, &queue, encoder)
        },
        120,
    )?;
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
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "schema_version":1,"version":env!("CARGO_PKG_VERSION"),"optimized":!cfg!(debug_assertions),
            "adapter":info.name,"backend":format!("{:?}",info.backend),
            "source_dimensions":[512,512,128],"retained_scalar_bytes":512*512*128*4,
            "retained_mask_bytes":if padding {512*512*128} else {0},
            "output_size":[1600,1000],"warmup_per_case":4,
            "scope":"CPU encode/upload/submit through GPU completion fence; excludes OS input, display/vsync, file decode and PNG readback",
            "synthetic_generation_ms":generation_ms,"volume_upload_and_pipeline_ms":upload_ms,
            "scalar_scroll":scalar,"volume_interactive_256_and_paint":interactive,"refined_768_8row_compute":tile
        }))?
    );
    Ok(())
}
