// SPDX-License-Identifier: GPL-3.0-or-later
//! Actual callback preparation/paint order: siblings must retain independent textures and uniforms.
use super::*;
use egui_wgpu::CallbackTrait;
use orv_core::WindowLevel;

#[test]
fn three_gpu_viewports_keep_independent_images_and_display_settings() {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: orv_render::native_backends(),
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapter = pollster::block_on(instance.request_adapter(&Default::default()))
        .expect("Real GPU required");
    let (device, queue) = pollster::block_on(adapter.request_device(&Default::default())).unwrap();
    let mut resources = egui_wgpu::CallbackResources::default();
    let mut encoder = device.create_command_encoder(&Default::default());
    let screen = egui_wgpu::ScreenDescriptor {
        size_in_pixels: [6, 2],
        pixels_per_point: 1.,
    };
    let callbacks = (0..3)
        .map(|i| {
            let mut pixels = vec![(i + 1) as f32 * 64.; 4];
            if i == 1 {
                pixels[0] = f32::NAN;
            }
            SliceCallback {
                slot: i,
                repaint: egui::Context::default(),
                image: Arc::new(Frame {
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
    for callback in &callbacks {
        assert!(
            callback
                .prepare(&device, &queue, &screen, &mut encoder, &mut resources)
                .is_empty()
        );
    }
    let size = wgpu::Extent3d {
        width: 6,
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
                label: Some("three callbacks"),
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
                    screen_size_px: [6, 2],
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
    for (y, row) in [[64u8, 64, 10, 128, 63, 63], [64, 64, 128, 128, 63, 63]]
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
}
