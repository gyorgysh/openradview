// SPDX-License-Identifier: GPL-3.0-or-later
//! Actual native GPU checks. No adapter skip is considered success.
#[path = "volume_gpu/clipping.rs"]
mod clipping;
#[path = "volume_gpu/color.rs"]
mod color;
#[path = "volume_gpu/limits.rs"]
mod limits;
#[path = "volume_gpu/padding.rs"]
mod padding;
#[path = "volume_gpu/presentation.rs"]
mod presentation;
#[path = "volume_gpu/snapshot.rs"]
mod snapshot;
use orv_core::{
    ViewState, WindowLevel,
    study::{Frame, Geometry},
    volume::Volume,
    volume_view::*,
};
use orv_render::volume::VolumeRenderer;
use std::{
    sync::{Arc, mpsc},
    time::Duration,
};

fn volume(constant: Option<f32>, monochrome1: bool) -> Arc<Volume> {
    Arc::new(
        Volume::new(
            (0..4)
                .map(|z| {
                    Arc::new(Frame {
                        slab: None,
                        scalar: None,
                        validity: None,
                        color: None,
                        nifti: None,
                        rgba: None,
                        width: 4,
                        height: 4,
                        pixels: (0..4)
                            .flat_map(|y| {
                                (0..4)
                                    .map(move |x| constant.unwrap_or((x + 10 * y + 100 * z) as f32))
                            })
                            .collect::<Vec<_>>()
                            .into(),
                        geometry: Some(Geometry {
                            nifti_transform: None,
                            frame_of_reference: Some("synthetic".into()),
                            position: [0.25 * z as f64, 0.5 * z as f64, 2. * z as f64],
                            orientation: [1., 0., 0., 0., 1., 0.],
                            spacing: [1.5, 1.],
                        }),
                        instance: z,
                        frame_index: 0,
                        monochrome1,
                        default_window: WindowLevel {
                            function: Default::default(),
                            width: 512.,
                            center: 256.,
                        },
                    })
                })
                .collect(),
        )
        .unwrap(),
    )
}
fn device() -> (wgpu::Device, wgpu::Queue) {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: orv_render::configured_backends().expect("Native backend selection"),
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapter = pollster::block_on(instance.request_adapter(&Default::default()))
        .expect("A real GPU is required");
    pollster::block_on(adapter.request_device(&orv_render::device_descriptor(&adapter))).unwrap()
}
fn read(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    mut encoder: wgpu::CommandEncoder,
    size: [u32; 2],
) -> Vec<u8> {
    let stride = (size[0] * 4).div_ceil(256) * 256;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("volume golden readback"),
        size: u64::from(stride) * u64::from(size[1]),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(stride),
                rows_per_image: Some(size[1]),
            },
        },
        wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        },
    );
    queue.submit([encoder.finish()]);
    let (tx, rx) = mpsc::sync_channel(1);
    buffer.map_async(wgpu::MapMode::Read, .., move |result| {
        let _ = tx.send(result);
    });
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(Duration::from_secs(10)),
        })
        .unwrap();
    rx.recv_timeout(Duration::from_secs(10)).unwrap().unwrap();
    let mut result = Vec::new();
    {
        let mapped = buffer.get_mapped_range(..).unwrap();
        for row in mapped.chunks_exact(stride as usize) {
            result.extend_from_slice(&row[..size[0] as usize * 4]);
        }
    }
    buffer.unmap();
    result
}
fn rays(
    volume: &Volume,
    display: &ViewState,
    state: &VolumeView,
    size: [u32; 2],
    x: u32,
    y: u32,
) -> Option<([f64; 3], [f64; 3], [f64; 2])> {
    let frame = state
        .camera
        .frame(display, f64::from(size[0]) / f64::from(size[1]))
        .unwrap();
    let (origin, direction) = frame.ray([
        (f64::from(x) + 0.5) / f64::from(size[0]),
        (f64::from(y) + 0.5) / f64::from(size[1]),
    ]);
    volume
        .clipped_ray_interval(origin, direction, state.clip.as_ref())
        .map(|interval| (origin, direction, interval))
}
fn assert_gray(pixel: &[u8], expected: f64) {
    for channel in &pixel[..3] {
        assert!(
            (f64::from(*channel) - expected).abs() <= 1.1,
            "{pixel:?} != {expected}"
        );
    }
    assert_eq!(pixel[3], 255);
}

#[test]
fn mip_matches_affine_analytic_endpoints_and_display_gamma() {
    let (device, queue) = device();
    let size = [7, 5];
    for monochrome1 in [false, true] {
        let volume = volume(None, monochrome1);
        let mut engine = VolumeRenderer::new(
            &device,
            &queue,
            volume.clone(),
            wgpu::TextureFormat::Rgba8UnormSrgb,
            || false,
        )
        .unwrap();
        let mut display = ViewState {
            window: WindowLevel {
                function: Default::default(),
                width: 512.,
                center: 256.,
            },
            ..Default::default()
        };
        let mut state = VolumeView::fit(&volume);
        state.camera.vertical_span_mm = 7.;
        for (yaw, pitch, roll) in [(0., 0., 0.), (90., 0., 0.), (23., -18., 11.)] {
            state.camera.yaw_degrees = yaw;
            state.camera.pitch_degrees = pitch;
            state.camera.roll_degrees = roll;
            display.invert = yaw == 90.;
            let mut encoder = device.create_command_encoder(&Default::default());
            engine
                .render(&device, &mut encoder, &display, &state, size)
                .unwrap();
            let target = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("volume golden display"),
                size: wgpu::Extent3d {
                    width: size[0],
                    height: size[1],
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let view = target.create_view(&Default::default());
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: None,
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
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
            let pixels = read(&device, &queue, &target, encoder, size);
            let mut visible = 0;
            let mut outside = 0;
            for y in 0..size[1] {
                for x in 0..size[0] {
                    let expected = if let Some((origin, direction, interval)) =
                        rays(&volume, &display, &state, size, x, y)
                    {
                        visible += 1;
                        let scalar = |t: f64| {
                            let p = volume.world_to_voxel(std::array::from_fn(|i| {
                                origin[i] + direction[i] * t
                            }));
                            p[0] + 10. * p[1] + 100. * p[2]
                        };
                        let mut gray = display
                            .window
                            .normalize(scalar(interval[0]).max(scalar(interval[1])) as f32);
                        if display.invert ^ monochrome1 {
                            gray = 1. - gray;
                        }
                        f64::from(gray) * 255.
                    } else {
                        outside += 1;
                        10.
                    };
                    let offset = ((y * size[0] + x) * 4) as usize;
                    assert_gray(&pixels[offset..offset + 4], expected);
                }
            }
            assert!(visible > 0 && outside > 0);
        }
    }
}

#[test]
fn composite_opacity_is_physical_and_independent_of_sampling_quality() {
    let (device, queue) = device();
    let size = [7, 5];
    let volume = volume(Some(200.), false);
    let mut engine = VolumeRenderer::new(
        &device,
        &queue,
        volume.clone(),
        wgpu::TextureFormat::Rgba8Unorm,
        || false,
    )
    .unwrap();
    let display = ViewState {
        window: WindowLevel {
            function: Default::default(),
            width: 256.,
            center: 128.,
        },
        ..Default::default()
    };
    let mut state = VolumeView::fit(&volume);
    state.projection = Projection::Composite;
    state.camera.vertical_span_mm = 7.;
    state.camera.yaw_degrees = 27.;
    state.camera.pitch_degrees = 13.;
    state.transfer = TransferFunction {
        low: 0.,
        high: 100.,
        opacity_per_mm: 0.2,
        shape: TransferShape::Ramp,
    };
    for quality in [Quality::Interactive, Quality::Refined] {
        state.quality = quality;
        let mut encoder = device.create_command_encoder(&Default::default());
        engine
            .render(&device, &mut encoder, &display, &state, size)
            .unwrap();
        let pixels = read(
            &device,
            &queue,
            engine.output_texture().unwrap(),
            encoder,
            size,
        );
        for y in 0..size[1] {
            for x in 0..size[0] {
                let expected = if let Some((_, _, [near, far])) =
                    rays(&volume, &display, &state, size, x, y)
                {
                    let alpha =
                        1. - (-f64::from(state.transfer.opacity_per_mm) * (far - near)).exp();
                    200. * alpha + 10. * (1. - alpha)
                } else {
                    10.
                };
                let offset = ((y * size[0] + x) * 4) as usize;
                assert_gray(&pixels[offset..offset + 4], expected);
            }
        }
    }
    state.transfer.opacity_per_mm = 0.;
    let mut encoder = device.create_command_encoder(&Default::default());
    engine
        .render(&device, &mut encoder, &display, &state, size)
        .unwrap();
    let pixels = read(
        &device,
        &queue,
        engine.output_texture().unwrap(),
        encoder,
        size,
    );
    for pixel in pixels.as_chunks::<4>().0 {
        assert_gray(pixel, 10.);
    }
}

#[test]
fn tiled_dispatches_keep_immutable_parameters_and_obey_bounds_and_cancel() {
    let (device, queue) = device();
    let size = [7, 5];
    let volume = volume(Some(200.), false);
    assert!(
        VolumeRenderer::new(
            &device,
            &queue,
            volume.clone(),
            wgpu::TextureFormat::Rgba8Unorm,
            || true
        )
        .is_err()
    );
    let mut engine = VolumeRenderer::new(
        &device,
        &queue,
        volume.clone(),
        wgpu::TextureFormat::Rgba8Unorm,
        || false,
    )
    .unwrap();
    let mut display = ViewState {
        window: WindowLevel {
            function: Default::default(),
            width: 256.,
            center: 128.,
        },
        ..Default::default()
    };
    let mut state = VolumeView::fit(&volume);
    state.camera.vertical_span_mm = 7.;
    let mut encoder = device.create_command_encoder(&Default::default());
    assert!(
        engine
            .render_region(
                &device,
                &mut encoder,
                &display,
                &state,
                size,
                [[0, 0], [8, 5]]
            )
            .is_err()
    );
    engine
        .render_region(
            &device,
            &mut encoder,
            &display,
            &state,
            size,
            [[0, 0], [3, 5]],
        )
        .unwrap();
    display.invert = true;
    engine
        .render_region(
            &device,
            &mut encoder,
            &display,
            &state,
            size,
            [[3, 0], [4, 5]],
        )
        .unwrap();
    let pixels = read(
        &device,
        &queue,
        engine.output_texture().unwrap(),
        encoder,
        size,
    );
    for y in 0..size[1] {
        for x in 0..size[0] {
            let expected = if rays(&volume, &display, &state, size, x, y).is_some() {
                if x < 3 { 200. } else { 55. }
            } else {
                10.
            };
            let offset = ((y * size[0] + x) * 4) as usize;
            assert_gray(&pixels[offset..offset + 4], expected);
        }
    }
}

#[test]
fn refined_steps_follow_voxel_speed_in_a_strongly_sheared_volume() {
    let (device, queue) = device();
    let frames = (0..4)
        .map(|z| {
            Arc::new(Frame {
                slab: None,
                scalar: None,
                validity: None,
                color: None,
                nifti: None,
                rgba: None,
                width: 4,
                height: 4,
                pixels: (0..4)
                    .flat_map(|_| [0., 255., 0., 0.])
                    .collect::<Vec<_>>()
                    .into(),
                geometry: Some(Geometry {
                    nifti_transform: None,
                    frame_of_reference: Some("synthetic-shear".into()),
                    position: [10. * z as f64, 0., z as f64],
                    orientation: [1., 0., 0., 0., 1., 0.],
                    spacing: [1., 1.],
                }),
                instance: z,
                frame_index: 0,
                monochrome1: false,
                default_window: WindowLevel {
                    function: Default::default(),
                    width: 256.,
                    center: 128.,
                },
            })
        })
        .collect();
    let volume = Arc::new(Volume::new(frames).unwrap());
    let display = ViewState {
        window: WindowLevel {
            function: Default::default(),
            width: 256.,
            center: 128.,
        },
        ..Default::default()
    };
    let mut state = VolumeView::fit(&volume);
    state.camera.pitch_degrees = 90.;
    let mut engine = VolumeRenderer::new(
        &device,
        &queue,
        volume,
        wgpu::TextureFormat::Rgba8Unorm,
        || false,
    )
    .unwrap();
    let mut encoder = device.create_command_encoder(&Default::default());
    engine
        .render(&device, &mut encoder, &display, &state, [1, 1])
        .unwrap();
    let pixels = read(
        &device,
        &queue,
        engine.output_texture().unwrap(),
        encoder,
        [1, 1],
    );
    // Half-voxel sampling is approximate, but must see this interior peak.
    // A spacing-only ray step sampled just the two zero-valued endpoints.
    assert!(
        pixels[0] >= 190,
        "Sheared interior peak was skipped: {}",
        pixels[0]
    );
}
