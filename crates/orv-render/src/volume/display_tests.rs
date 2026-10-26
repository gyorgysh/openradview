// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use std::{sync::mpsc, time::Duration};

#[test]
fn gl_compatible_display_matches_hardware_srgb_interpolation_and_framebuffer_encoding() {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: crate::configured_backends().unwrap(),
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapter = pollster::block_on(instance.request_adapter(&Default::default()))
        .expect("Real GPU required");
    let (device, queue) =
        pollster::block_on(adapter.request_device(&crate::device_descriptor(&adapter))).unwrap();
    let mut frames = orv_core::study::demo_study().series.remove(0).frames;
    frames.truncate(2);
    let volume = Arc::new(Volume::new(frames).unwrap());
    for format in [
        wgpu::TextureFormat::Rgba8Unorm,
        wgpu::TextureFormat::Rgba8UnormSrgb,
    ] {
        for manual in [false, true] {
            // The alias-view path is unavailable on native GL. Exercise both
            // production display modes on hosts whose driver supports both.
            if !manual && adapter.get_info().backend == wgpu::Backend::Gl {
                continue;
            }
            let mut renderer = VolumeRenderer::new_with_display_mode(
                &device,
                &queue,
                volume.clone(),
                format,
                manual,
                || false,
            )
            .unwrap();
            renderer.ensure_output(&device, [2, 2], 0);
            let source = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("RGB interpolation fixture"),
                size: wgpu::Extent3d {
                    width: 2,
                    height: 2,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: if manual {
                    wgpu::TextureFormat::Rgba8Unorm
                } else {
                    wgpu::TextureFormat::Rgba8UnormSrgb
                },
                usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            queue.write_texture(
                source.as_image_copy(),
                &[
                    0, 128, 255, 255, 255, 128, 0, 255, 0, 128, 255, 255, 255, 128, 0, 255,
                ],
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(8),
                    rows_per_image: Some(2),
                },
                source.size(),
            );
            renderer.outputs[0].as_mut().unwrap().display =
                renderer.display_bindings(&device, &source.create_view(&Default::default()));
            let target = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("display interpolation readback"),
                size: wgpu::Extent3d {
                    width: 3,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            });
            let view = target.create_view(&Default::default());
            let mut encoder = device.create_command_encoder(&Default::default());
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("volume display regression"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    ..Default::default()
                });
                renderer.paint(&mut pass);
            }
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("display bytes"),
                size: 256,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            encoder.copy_texture_to_buffer(
                target.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer: &buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(256),
                        rows_per_image: Some(1),
                    },
                },
                target.size(),
            );
            queue.submit([encoder.finish()]);
            let (tx, rx) = mpsc::sync_channel(1);
            buffer.map_async(wgpu::MapMode::Read, .., move |result| {
                tx.send(result).unwrap();
            });
            device
                .poll(wgpu::PollType::Wait {
                    submission_index: None,
                    timeout: Some(Duration::from_secs(10)),
                })
                .unwrap();
            rx.recv_timeout(Duration::from_secs(10)).unwrap().unwrap();
            {
                let bytes = buffer.get_mapped_range(..).unwrap();
                for (pixel, expected) in bytes[..12].as_chunks::<4>().0.iter().zip([
                    [0i16, 128, 255],
                    [188, 128, 188],
                    [255, 128, 0],
                ]) {
                    for (channel, expected) in pixel[..3].iter().zip(expected) {
                        assert!(
                            (i16::from(*channel) - expected).abs() <= 1,
                            "Incorrect linear-light interpolation: {pixel:?}, expected {expected}, manual={manual}, format={format:?}"
                        );
                    }
                    assert_eq!(pixel[3], 255);
                }
            }
            buffer.unmap();
        }
    }
}
