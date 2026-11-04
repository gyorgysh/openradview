// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use orv_core::volume_material::{
    MaterialPoint, VolumeMaterial, gradient_normal, physical_gradient,
};
use orv_render::{
    material_volume::{Lighting, MaterialVolumeRenderer},
    volume::VolumeTarget,
    volume_source::GpuVolumeSource,
};

fn material() -> VolumeMaterial {
    VolumeMaterial::new(&[
        MaterialPoint {
            value: 0.,
            luminance: 0.1,
            extinction_per_mm: 0.1,
        },
        MaterialPoint {
            value: 150.,
            luminance: 0.8,
            extinction_per_mm: 0.1,
        },
        MaterialPoint {
            value: 400.,
            luminance: 0.2,
            extinction_per_mm: 0.3,
        },
    ])
    .unwrap()
}
fn constant_material(luminance: f32, extinction_per_mm: f32) -> VolumeMaterial {
    VolumeMaterial::new(&[-1000., 1000.].map(|value| MaterialPoint {
        value,
        luminance,
        extinction_per_mm,
    }))
    .unwrap()
}
fn read_linear(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    mut encoder: wgpu::CommandEncoder,
) -> Vec<[f64; 4]> {
    let size = texture.size();
    let stride = (size.width * 8).div_ceil(256) * 256;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("generated linear radiance readback"),
        size: u64::from(stride) * u64::from(size.height),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
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
    let values = {
        let bytes = buffer.get_mapped_range(..).unwrap();
        bytes
            .chunks_exact(stride as usize)
            .flat_map(|row| {
                row[..size.width as usize * 8]
                    .as_chunks::<8>()
                    .0
                    .iter()
                    .map(|pixel| {
                        std::array::from_fn(|i| {
                            let bits = u16::from_le_bytes([pixel[i * 2], pixel[i * 2 + 1]]);
                            assert_eq!(bits & 0x8000, 0, "Radiance is nonnegative");
                            let exponent = (bits >> 10) & 31;
                            assert_ne!(exponent, 31, "Radiance must be finite");
                            let fraction = f64::from(bits & 1023);
                            if exponent == 0 {
                                fraction * 2f64.powi(-24)
                            } else {
                                (1. + fraction / 1024.) * 2f64.powi(i32::from(exponent) - 15)
                            }
                        })
                    })
            })
            .collect::<Vec<_>>()
    };
    buffer.unmap();
    values
}
fn reference(
    volume: &Volume,
    view: &ViewState,
    state: &VolumeView,
    material: &VolumeMaterial,
    lighting: Lighting,
    size: [u32; 2],
    pixel: [u32; 2],
) -> f64 {
    let background = 0.0030352698;
    let Some((origin, direction, range)) = rays(volume, view, state, size, pixel[0], pixel[1])
    else {
        return background;
    };
    let speed = volume
        .world_vector_to_voxel(direction)
        .into_iter()
        .map(f64::abs)
        .fold(0., f64::max);
    let length = range[1] - range[0];
    let count = ((length / (state.quality.step_factor() / speed)).ceil() as u32)
        .clamp(1, state.quality.max_steps());
    let step = length / f64::from(count);
    let (mut radiance, mut transmittance) = (0., 1.);
    for i in 0..count {
        let distance = range[0] + (f64::from(i) + 0.5) * step;
        let point = std::array::from_fn(|a| origin[a] + direction[a] * distance);
        let Some(value) = volume.sample(point) else {
            continue;
        };
        let optics = material.sample(value).unwrap();
        let alpha = optics.opacity(step).unwrap();
        let illumination = if lighting.diffuse == 0. {
            f64::from(lighting.ambient)
        } else {
            physical_gradient(volume, point)
                .and_then(gradient_normal)
                .map_or(1., |normal| {
                    f64::from(lighting.ambient)
                        + f64::from(lighting.diffuse)
                            * normal
                                .into_iter()
                                .zip(direction)
                                .map(|(a, b)| a * b)
                                .sum::<f64>()
                                .abs()
                })
        };
        radiance += transmittance * alpha * optics.luminance * illumination;
        transmittance *= 1. - alpha;
        if transmittance < 0.0001 {
            break;
        }
    }
    radiance + transmittance * background
}
fn renderer(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    volume: Arc<Volume>,
    format: wgpu::TextureFormat,
) -> MaterialVolumeRenderer {
    let source = GpuVolumeSource::new(device, queue, volume, || false).unwrap();
    MaterialVolumeRenderer::new(source, format).unwrap()
}
fn target(size: [u32; 2]) -> VolumeTarget {
    VolumeTarget {
        slot: 0,
        size,
        region: [[0, 0], size],
    }
}

#[test]
fn homogeneous_optical_slab_matches_beer_lambert_at_both_step_sizes() {
    let (device, queue) = device();
    let volume = volume(Some(100.), false);
    let view = ViewState::default();
    let mut state = VolumeView::fit(&volume);
    state.projection = Projection::Composite;
    state.camera.yaw_degrees = 25.;
    state.camera.pitch_degrees = 15.;
    let material = constant_material(0.8, 0.2);
    let mut renderer = renderer(
        &device,
        &queue,
        volume.clone(),
        wgpu::TextureFormat::Rgba8UnormSrgb,
    );
    let size = [23, 19];
    for quality in [Quality::Interactive, Quality::Refined] {
        state.quality = quality;
        for lighting in [Lighting::UNLIT, Lighting::HEADLIGHT] {
            let mut encoder = device.create_command_encoder(&Default::default());
            renderer
                .render_target(
                    &mut encoder,
                    &view,
                    &state,
                    &material,
                    lighting,
                    target(size),
                )
                .unwrap();
            let values = read_linear(
                &device,
                &queue,
                renderer.output_texture(0).unwrap(),
                encoder,
            );
            for y in 0..size[1] {
                for x in 0..size[0] {
                    let expected = rays(&volume, &view, &state, size, x, y).map_or(
                        0.0030352698,
                        |(_, _, r)| {
                            let t = (-0.2f64 * (r[1] - r[0])).exp();
                            0.8 * (1. - t) + 0.0030352698 * t
                        },
                    );
                    let pixel = values[(y * size[0] + x) as usize];
                    assert!(
                        (pixel[0] - expected).abs() < 0.0006,
                        "{quality:?}: {pixel:?} != {expected}"
                    );
                    assert_eq!(pixel[0], pixel[1]);
                    assert_eq!(pixel[1], pixel[2]);
                    assert_eq!(pixel[3], 1.);
                }
            }
        }
    }
}

#[test]
fn piecewise_material_and_physical_gradient_match_reference_on_sheared_grid() {
    let (device, queue) = device();
    let volume = volume(None, false);
    let view = ViewState::default();
    let mut state = VolumeView::fit(&volume);
    state.projection = Projection::Composite;
    state.camera.yaw_degrees = 33.;
    state.camera.pitch_degrees = 21.;
    let material = material();
    let mut renderer = renderer(
        &device,
        &queue,
        volume.clone(),
        wgpu::TextureFormat::Rgba8Unorm,
    );
    let size = [32, 24];
    let mut images = Vec::new();
    for lighting in [Lighting::UNLIT, Lighting::HEADLIGHT] {
        let mut encoder = device.create_command_encoder(&Default::default());
        // Different rows share one encoder but retain their own region uniforms.
        for y in (0..size[1]).step_by(3) {
            renderer
                .render_target(
                    &mut encoder,
                    &view,
                    &state,
                    &material,
                    lighting,
                    VolumeTarget {
                        slot: 0,
                        size,
                        region: [[0, y], [size[0], 3]],
                    },
                )
                .unwrap();
        }
        let values = read_linear(
            &device,
            &queue,
            renderer.output_texture(0).unwrap(),
            encoder,
        );
        for y in 0..size[1] {
            for x in 0..size[0] {
                let expected = reference(&volume, &view, &state, &material, lighting, size, [x, y]);
                let pixel = values[(y * size[0] + x) as usize];
                assert!(
                    (pixel[0] - expected).abs() < 0.0009,
                    "{lighting:?} ({x},{y}): {pixel:?} != {expected}"
                );
            }
        }
        images.push(values);
    }
    assert!(
        images[0]
            .iter()
            .zip(&images[1])
            .any(|(a, b)| a[0] - b[0] > 0.05)
    );
}

#[test]
fn missing_stencils_do_not_invent_boundary_normals_and_transparent_rays_keep_background() {
    let (device, queue) = device();
    let mut frames = volume(None, false).frames().to_vec();
    for frame in &mut frames {
        Arc::make_mut(frame).validity = Some(
            (0..16)
                .map(|i| u8::from(i != 5 && i != 6))
                .collect::<Vec<_>>()
                .into(),
        );
    }
    let volume = Arc::new(Volume::new(frames).unwrap());
    let view = ViewState::default();
    let mut state = VolumeView::fit(&volume);
    state.projection = Projection::Composite;
    let mut renderer = renderer(
        &device,
        &queue,
        volume.clone(),
        wgpu::TextureFormat::Rgba8Unorm,
    );
    let size = [17, 13];
    for material in [material(), constant_material(1., 0.)] {
        let mut encoder = device.create_command_encoder(&Default::default());
        renderer
            .render_target(
                &mut encoder,
                &view,
                &state,
                &material,
                Lighting::HEADLIGHT,
                target(size),
            )
            .unwrap();
        let values = read_linear(
            &device,
            &queue,
            renderer.output_texture(0).unwrap(),
            encoder,
        );
        for y in 0..size[1] {
            for x in 0..size[0] {
                let expected = reference(
                    &volume,
                    &view,
                    &state,
                    &material,
                    Lighting::HEADLIGHT,
                    size,
                    [x, y],
                );
                assert!((values[(y * size[0] + x) as usize][0] - expected).abs() < 0.001);
            }
        }
    }
}

#[test]
fn linear_radiance_is_encoded_once_for_both_framebuffer_formats() {
    let (device, queue) = device();
    let volume = volume(Some(100.), false);
    let view = ViewState::default();
    let mut state = VolumeView::fit(&volume);
    state.projection = Projection::Composite;
    let material = constant_material(0.21404114, 10.);
    for format in [
        wgpu::TextureFormat::Rgba8Unorm,
        wgpu::TextureFormat::Rgba8UnormSrgb,
    ] {
        let mut renderer = renderer(&device, &queue, volume.clone(), format);
        let mut encoder = device.create_command_encoder(&Default::default());
        renderer
            .render_target(
                &mut encoder,
                &view,
                &state,
                &material,
                Lighting::UNLIT,
                target([3, 3]),
            )
            .unwrap();
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("linear display encoding target"),
            size: wgpu::Extent3d {
                width: 3,
                height: 3,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let output = texture.create_view(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &output,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            renderer.paint_target(0, &mut pass);
        }
        let pixels = read(&device, &queue, &texture, encoder, [3, 3]);
        // Float16 integration plus framebuffer encoding can straddle one byte.
        // A second gamma transform would differ by dozens of display codes.
        assert_gray(&pixels[16..20], 128.);
        assert_eq!(pixels[19], 255);
    }
}

#[test]
fn material_rendering_refuses_unrepresentable_knots_and_conflicting_display_modes() {
    let (device, queue) = device();
    let volume = volume(Some(100.), false);
    let mut renderer = renderer(
        &device,
        &queue,
        volume.clone(),
        wgpu::TextureFormat::Rgba8Unorm,
    );
    let mut view = ViewState::default();
    let mut state = VolumeView::fit(&volume);
    let material = material();
    let mut encoder = device.create_command_encoder(&Default::default());
    assert!(
        renderer
            .render_target(
                &mut encoder,
                &view,
                &state,
                &material,
                Lighting::UNLIT,
                target([4, 4])
            )
            .is_err()
    );
    state.projection = Projection::Composite;
    view.invert = true;
    assert!(
        renderer
            .render_target(
                &mut encoder,
                &view,
                &state,
                &material,
                Lighting::UNLIT,
                target([4, 4])
            )
            .is_err()
    );
    view.invert = false;
    let tiny = VolumeMaterial::new(&[0., f32::from_bits(1)].map(|value| MaterialPoint {
        value,
        luminance: 1.,
        extinction_per_mm: 0.1,
    }))
    .unwrap();
    assert!(
        renderer
            .render_target(
                &mut encoder,
                &view,
                &state,
                &tiny,
                Lighting::UNLIT,
                target([4, 4])
            )
            .is_err()
    );
    for lighting in [
        Lighting {
            ambient: f32::NAN,
            diffuse: 0.,
        },
        Lighting {
            ambient: -0.1,
            diffuse: 0.,
        },
        Lighting {
            ambient: 0.5,
            diffuse: 0.6,
        },
    ] {
        assert!(
            renderer
                .render_target(
                    &mut encoder,
                    &view,
                    &state,
                    &material,
                    lighting,
                    target([4, 4])
                )
                .is_err()
        );
    }
    assert!(
        renderer
            .render_target(
                &mut encoder,
                &view,
                &state,
                &material,
                Lighting::UNLIT,
                target([2049, 2])
            )
            .is_err()
    );
}
