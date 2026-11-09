// SPDX-License-Identifier: GPL-3.0-or-later
//! Actual callback preparation/paint order: siblings must retain independent textures and uniforms.
use super::*;
use egui_wgpu::CallbackTrait;
use orv_core::WindowLevel;

#[test]
fn recovered_slice_requests_one_repaint_to_clear_the_old_error() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        flags: orv_render::instance_flags(),
        backends: orv_render::configured_backends().expect("Native backend selection"),
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapter = pollster::block_on(instance.request_adapter(&Default::default()))
        .expect("Real GPU required");
    let (device, queue) =
        pollster::block_on(adapter.request_device(&orv_render::device_descriptor(&adapter)))
            .unwrap();
    let mut resources = egui_wgpu::CallbackResources::default();
    let mut encoder = device.create_command_encoder(&Default::default());
    let screen = egui_wgpu::ScreenDescriptor {
        size_in_pixels: [2, 2],
        pixels_per_point: 1.,
    };
    let repaint = egui::Context::default();
    let repaint_count = Arc::new(AtomicUsize::new(0));
    let count = repaint_count.clone();
    repaint.set_request_repaint_callback(move |_| {
        count.fetch_add(1, Ordering::Relaxed);
    });
    let mut callback = SliceCallback {
        slot: 1,
        repaint: repaint.clone(),
        image: Arc::new(Frame {
            memory: None,
            presentation: None,
            calibration: None,
            projection: None,
            sop: None,
            slab: None,
            scalar: None,
            validity: None,
            color: None,
            nifti: None,
            rgba: None,
            width: 0,
            height: 2,
            pixels: vec![0.; 4].into(),
            geometry: None,
            instance: 0,
            frame_index: 0,
            monochrome1: false,
            default_window: WindowLevel::SOFT_TISSUE,
        }),
        gpu_image: None,
        view: ViewState::default(),
        aspect: 1.,
        format: wgpu::TextureFormat::Rgba8Unorm,
    };
    let _ = repaint.run_logic(&egui::RawInput::default(), |_| {});
    callback.prepare(&device, &queue, &screen, &mut encoder, &mut resources);
    assert_eq!(
        resources.get::<ViewportRenderers>().unwrap().slots[&1].error,
        Some("Image dimensions exceed GPU limits")
    );
    assert!(repaint_count.load(Ordering::Relaxed) > 0);

    let _ = repaint.run_logic(&egui::RawInput::default(), |_| {});
    let _ = repaint.run_logic(&egui::RawInput::default(), |_| {});
    repaint_count.store(0, Ordering::Relaxed);
    Arc::make_mut(&mut callback.image).width = 2;
    callback.prepare(&device, &queue, &screen, &mut encoder, &mut resources);
    assert_eq!(
        resources.get::<ViewportRenderers>().unwrap().slots[&1].error,
        None
    );
    assert!(
        repaint_count.load(Ordering::Relaxed) > 0,
        "The UI must redraw to remove the previous error overlay"
    );

    let _ = repaint.run_logic(&egui::RawInput::default(), |_| {});
    let _ = repaint.run_logic(&egui::RawInput::default(), |_| {});
    repaint_count.store(0, Ordering::Relaxed);
    callback.prepare(&device, &queue, &screen, &mut encoder, &mut resources);
    assert_eq!(
        repaint_count.load(Ordering::Relaxed),
        0,
        "Idle views must not repaint"
    );
}

#[test]
fn four_gpu_viewports_keep_independent_images_settings_and_release_closed_slots() {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        flags: orv_render::instance_flags(),
        backends: orv_render::configured_backends().expect("Native backend selection"),
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapter = pollster::block_on(instance.request_adapter(&Default::default()))
        .expect("Real GPU required");
    let (device, queue) =
        pollster::block_on(adapter.request_device(&orv_render::device_descriptor(&adapter)))
            .unwrap();
    let mut resources = egui_wgpu::CallbackResources::default();
    let mut encoder = device.create_command_encoder(&Default::default());
    let screen = egui_wgpu::ScreenDescriptor {
        size_in_pixels: [8, 2],
        pixels_per_point: 1.,
    };
    let mut callbacks = (0..4)
        .map(|i| {
            let mut pixels = vec![(i + 1) as f32 * 64.; 4];
            if i == 1 {
                pixels[0] = f32::NAN;
            }
            SliceCallback {
                gpu_image: None,
                slot: 8 + i,
                repaint: egui::Context::default(),
                image: Arc::new(Frame {
                    memory: None,
                    presentation: None,
                    calibration: None,
                    projection: None,
                    sop: None,
                    slab: None,
                    scalar: None,
                    validity: None,
                    color: None,
                    nifti: None,
                    rgba: None,
                    width: 2,
                    height: 2,
                    pixels: pixels.into(),
                    geometry: None,
                    instance: 0,
                    frame_index: 0,
                    monochrome1: false,
                    default_window: WindowLevel::SOFT_TISSUE,
                }),
                view: ViewState {
                    window: WindowLevel {
                        function: Default::default(),
                        width: 256.,
                        center: 128.,
                    },
                    invert: i == 2,
                    ..Default::default()
                },
                aspect: 1.,
                format: wgpu::TextureFormat::Rgba8Unorm,
            }
        })
        .collect::<Vec<_>>();
    for i in [0, 2, 3] {
        let frames = (0..2)
            .map(|z| {
                let mut frame = (*callbacks[i].image).clone();
                frame.geometry = Some(orv_core::study::Geometry {
                    position: [0., 0., f64::from(z)],
                    orientation: [1., 0., 0., 0., 1., 0.],
                    spacing: [1.; 2],
                    frame_of_reference: Some("generated native callback source".into()),
                    nifti_transform: None,
                });
                Arc::new(frame)
            })
            .collect();
        let volume = Arc::new(orv_core::volume::Volume::new(frames).unwrap());
        let grid = volume
            .reformat_grid(orv_core::Plane::Axial, volume.center(), 64)
            .unwrap();
        let source =
            orv_render::volume_source::GpuVolumeSource::new(&device, &queue, volume, || false)
                .unwrap();
        let image = orv_render::reslice::GpuReslicer::new(source)
            .unwrap()
            .render_frames([Some(&grid), None, None], || false)
            .unwrap()[0]
            .take()
            .unwrap();
        callbacks[i].image = image.frame().clone();
        callbacks[i].gpu_image = Some(image);
    }
    for callback in &callbacks {
        assert!(
            callback
                .prepare(&device, &queue, &screen, &mut encoder, &mut resources)
                .is_empty()
        );
    }
    let size = wgpu::Extent3d {
        width: 8,
        height: 2,
        depth_or_array_layers: 1,
    };
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("multi viewport golden"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("golden pixels"),
        size: 512,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    {
        let mut pass = encoder
            .begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("four callbacks"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            })
            .forget_lifetime();
        for (i, callback) in callbacks.iter().enumerate() {
            let x = i as f32 * 2.;
            pass.set_viewport(x, 0., 2., 2., 0., 1.);
            pass.set_scissor_rect(i as u32 * 2, 0, 2, 2);
            let rect = egui::Rect::from_min_size(egui::pos2(x, 0.), egui::vec2(2., 2.));
            callback.paint(
                egui::PaintCallbackInfo {
                    viewport: rect,
                    clip_rect: rect,
                    pixels_per_point: 1.,
                    screen_size_px: [8, 2],
                },
                &mut pass,
                &resources,
            );
        }
    }
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &target,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(256),
                rows_per_image: Some(2),
            },
        },
        size,
    );
    queue.submit([encoder.finish()]);
    let (tx, rx) = std::sync::mpsc::channel();
    buffer.map_async(wgpu::MapMode::Read, .., move |r| tx.send(r).unwrap());
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(std::time::Duration::from_secs(10)),
        })
        .unwrap();
    rx.recv_timeout(std::time::Duration::from_secs(10))
        .unwrap()
        .unwrap();
    let bytes = buffer.get_mapped_range(..).unwrap();
    for (y, row) in [
        [64u8, 64, 10, 128, 63, 63, 255, 255],
        [64, 64, 128, 128, 63, 63, 255, 255],
    ]
    .into_iter()
    .enumerate()
    {
        for (x, value) in row.into_iter().enumerate() {
            for channel in &bytes[y * 256 + x * 4..y * 256 + x * 4 + 3] {
                assert!(
                    (i16::from(*channel) - i16::from(value)).abs() <= 1,
                    "pixel {x},{y}: {channel} != {value}"
                );
            }
        }
    }
    drop(bytes);
    buffer.unmap();
    callbacks[0].gpu_image = None;
    let mut transition = device.create_command_encoder(&Default::default());
    callbacks[0].prepare(&device, &queue, &screen, &mut transition, &mut resources);
    let slot = &resources.get::<ViewportRenderers>().unwrap().slots[&8];
    assert!(slot.error.is_none());
    assert!(slot.gpu_source.is_none());
    assert!(Arc::ptr_eq(
        slot.current.as_ref().unwrap(),
        &callbacks[0].image
    ));
    let closed = [
        Arc::downgrade(&callbacks[2].image),
        Arc::downgrade(&callbacks[3].image),
    ];
    callbacks.truncate(2);
    assert!(closed.iter().all(|image| image.upgrade().is_some()));
    remove_slot(&mut resources, 10);
    remove_slot(&mut resources, 11);
    assert!(closed.iter().all(|image| image.upgrade().is_none()));
    let retained = &resources.get::<ViewportRenderers>().unwrap().slots;
    assert_eq!(retained.len(), 2);
    assert!(retained.contains_key(&8) && retained.contains_key(&9));
}
