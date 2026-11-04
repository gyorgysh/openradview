// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use crate::{
    label_source::GpuLabels,
    mask_projection::{GpuMaskProjector, MaskProjection},
};
use orv_core::{
    labels::Labels,
    mask::MaskView,
    study::{Geometry, NiftiMethod, NiftiTransform, demo_study},
    volume::Volume,
    volume_material::MaterialPoint,
    volume_view::{PhysicalClip, Projection, Quality},
};
use std::{
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};
const BACKGROUND: f64 = 0.0030352698;
fn device() -> (wgpu::Device, wgpu::Queue) {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: crate::configured_backends().unwrap(),
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapter = pollster::block_on(instance.request_adapter(&Default::default()))
        .expect("Actual GPU required");
    pollster::block_on(adapter.request_device(&crate::device_descriptor(&adapter))).unwrap()
}
fn volume(shear: bool, reversed: bool, offset: f64, missing: bool) -> Arc<Volume> {
    let template = demo_study().series.remove(0).frames.remove(0);
    Arc::new(
        Volume::new(
            (0..6)
                .map(|z| {
                    let mut f = (*template).clone();
                    f.width = 7;
                    f.height = 5;
                    f.pixels = vec![10.; 35].into();
                    f.validity = missing.then(|| vec![0; 35].into());
                    f.geometry = Some(Geometry {
                        position: [
                            offset + if shear { f64::from(z) * 0.25 } else { 0. },
                            -offset + if shear { f64::from(z) * 0.5 } else { 0. },
                            offset + f64::from(z) * 2.,
                        ],
                        orientation: if reversed {
                            [-1., 0., 0., 0., 1., 0.]
                        } else {
                            [1., 0., 0., 0., 1., 0.]
                        },
                        spacing: [2., 0.75],
                        frame_of_reference: Some("generated opaque material source".into()),
                        nifti_transform: None,
                    });
                    Arc::new(f)
                })
                .collect(),
        )
        .unwrap(),
    )
}
fn labels(source: Arc<Volume>, kind: u8) -> Arc<Labels> {
    let frames = source
        .frames()
        .iter()
        .map(|frame| {
            let mut f = (**frame).clone();
            f.validity = None;
            f.geometry.as_mut().unwrap().nifti_transform = Some(NiftiTransform {
                method: NiftiMethod::Sform,
                code: 1,
            });
            f.pixels = (0..35)
                .map(|i| {
                    if kind == 1 || (kind == 2 && i / 7 == 2) {
                        7.
                    } else {
                        0.
                    }
                })
                .collect::<Vec<_>>()
                .into();
            Arc::new(f)
        })
        .collect();
    Arc::new(Labels::import(source, &Volume::new(frames).unwrap(), || false).unwrap())
}
fn material(luminance: f32, sigma: f32) -> Arc<VolumeMaterial> {
    Arc::new(
        VolumeMaterial::new(&[-1000., 1000.].map(|value| MaterialPoint {
            value,
            luminance,
            extinction_per_mm: sigma,
        }))
        .unwrap(),
    )
}
fn mask(
    source: Arc<GpuVolumeSource>,
    labels: Arc<Labels>,
    state: &VolumeView,
    view: &ViewState,
    size: [u32; 2],
    number: u8,
) -> Arc<MaskImage> {
    let (labels, _) = GpuLabels::upload(source.clone(), labels, None, || false).unwrap();
    let mut projector = GpuMaskProjector::new(source.clone()).unwrap();
    let request = MaskProjection {
        labels,
        number,
        camera: state.camera.clone(),
        display: view.clone(),
        clip: state.clip.clone(),
        size,
    };
    let mut encoder = source.device().create_command_encoder(&Default::default());
    let image = projector
        .encode_rows(&mut encoder, &request, 0, 0..size[1])
        .unwrap()
        .unwrap();
    source.queue().submit([encoder.finish()]);
    image
}
fn read_bytes(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    bpp: u32,
) -> Vec<u8> {
    let size = texture.size();
    let stride = (size.width * bpp).div_ceil(256) * 256;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("generated opaque image readback"),
        size: u64::from(stride) * u64::from(size.height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
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
        tx.send(result).unwrap();
    });
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(Duration::from_secs(30)),
        })
        .unwrap();
    rx.recv_timeout(Duration::from_secs(10)).unwrap().unwrap();
    let data = buffer.get_mapped_range(..).unwrap();
    let result = data
        .chunks_exact(stride as usize)
        .flat_map(|row| row[..(size.width * bpp) as usize].iter().copied())
        .collect();
    drop(data);
    buffer.unmap();
    result
}
fn linear(device: &wgpu::Device, queue: &wgpu::Queue, texture: &wgpu::Texture) -> Vec<[f64; 4]> {
    read_bytes(device, queue, texture, 8)
        .as_chunks::<8>()
        .0
        .iter()
        .map(|v| {
            std::array::from_fn(|i| {
                let bits = u16::from_le_bytes([v[i * 2], v[i * 2 + 1]]);
                assert_eq!(bits & 0x8000, 0);
                let exponent = (bits >> 10) & 31;
                assert_ne!(exponent, 31);
                let fraction = f64::from(bits & 1023);
                if exponent == 0 {
                    fraction * 2f64.powi(-24)
                } else {
                    (1. + fraction / 1024.) * 2f64.powi(i32::from(exponent) - 15)
                }
            })
        })
        .collect()
}
fn read(device: &wgpu::Device, queue: &wgpu::Queue, image: &OpaqueMaterialImage) -> Vec<[f64; 4]> {
    linear(device, queue, image.texture())
}

fn range(
    origin: [f64; 3],
    direction: [f64; 3],
    minimum: [f64; 3],
    maximum: [f64; 3],
) -> Option<[f64; 2]> {
    let mut near = f64::NEG_INFINITY;
    let mut far = f64::INFINITY;
    for a in 0..3 {
        if direction[a] == 0. {
            if origin[a] < minimum[a] || origin[a] > maximum[a] {
                return None;
            }
        } else {
            let p = (minimum[a] - origin[a]) / direction[a];
            let q = (maximum[a] - origin[a]) / direction[a];
            near = near.max(p.min(q));
            far = far.min(p.max(q));
        }
    }
    (near <= far).then_some([near, far])
}
// Homogeneous Beer-Lambert optics; no discrete sampling loop is used here.
fn analytic(scene: &OpaqueMaterialScene, pixel: [u32; 2]) -> f64 {
    let request = scene.mask.request();
    let source = request.labels.labels().source();
    let size = request.size;
    let frame = scene
        .volume
        .camera
        .frame(&scene.navigation, f64::from(size[0]) / f64::from(size[1]))
        .unwrap();
    let (origin, direction) = frame.ray([
        (f64::from(pixel[0]) + 0.5) / f64::from(size[0]),
        (f64::from(pixel[1]) + 0.5) / f64::from(size[1]),
    ]);
    let hit = MaskView::manual(request.labels.labels(), request.number)
        .unwrap()
        .ray(origin, direction, [-1e30, 1e30], scene.volume.clip.as_ref())
        .unwrap()
        .first_eligible_hit(|| false)
        .unwrap();
    let mut scalar = range(
        source.world_to_voxel(origin),
        source.world_vector_to_voxel(direction),
        [0.; 3],
        source.dimensions().map(|n| (n - 1) as f64),
    );
    if let Some(clip) = &scene.volume.clip {
        scalar = scalar
            .zip(range(origin, direction, clip.minimum_mm, clip.maximum_mm))
            .map(|(a, b)| [a[0].max(b[0]), a[1].min(b[1])]);
    }
    let distance = scalar.map_or(0., |v| {
        (v[1].min(hit.as_ref().map_or(f64::INFINITY, |h| h.range_mm[0])) - v[0]).max(0.)
    });
    let optics = scene.material.sample(10.).unwrap();
    let transmission = if source.has_missing_scalar() {
        1.
    } else {
        (-optics.extinction_per_mm * distance).exp()
    };
    optics.luminance * (1. - transmission)
        + transmission
            * if hit.is_some() {
                f64::from(scene.mask_luminance)
            } else {
                BACKGROUND
            }
}
fn compare(device: &wgpu::Device, queue: &wgpu::Queue, image: &OpaqueMaterialImage) {
    let size = image.scene.mask.request().size;
    for (i, pixel) in read(device, queue, image).iter().enumerate() {
        let expected = analytic(
            &image.scene,
            [(i % size[0] as usize) as u32, (i / size[0] as usize) as u32],
        );
        for value in &pixel[..3] {
            assert!(
                (value - expected).abs() < 0.001,
                "pixel{i}: {value} vs {expected}"
            );
        }
        assert_eq!(pixel[3], 1.);
    }
}
#[test]
fn opaque_masks_end_source_integration_and_receive_foreground_attenuation() {
    let (device, queue) = device();
    let source = volume(false, false, 0., false);
    let gpu = GpuVolumeSource::new(&device, &queue, source.clone(), || false).unwrap();
    let mut renderer =
        OpaqueMaterialRenderer::new(gpu.clone(), wgpu::TextureFormat::Rgba8Unorm).unwrap();
    let mut state = VolumeView::fit(&source);
    state.projection = Projection::Composite;
    let view = ViewState::default();
    for kind in [0, 1, 2] {
        let image = mask(
            gpu.clone(),
            labels(source.clone(), kind),
            &state,
            &view,
            [1, 1],
            7,
        );
        for (lum, sigma) in [(0.2, 0.), (0.2, 0.4), (0.2, 2.)] {
            for quality in [Quality::Interactive, Quality::Refined] {
                let mut state = state.clone();
                state.quality = quality;
                let scene = OpaqueMaterialScene {
                    mask: image.clone(),
                    navigation: view.clone(),
                    volume: state,
                    material: material(lum, sigma),
                    lighting: Lighting::UNLIT,
                    mask_luminance: 0.9,
                };
                let result = renderer
                    .render_rows(&scene, 0, 0..1, || false)
                    .unwrap()
                    .unwrap();
                compare(&device, &queue, &result);
            }
        }
    }
    let image = mask(gpu, labels(source, 2), &state, &view, [1, 1], 7);
    let scene = OpaqueMaterialScene {
        mask: image,
        navigation: view,
        volume: state,
        material: material(0.2, 0.4),
        lighting: Lighting::UNLIT,
        mask_luminance: 0.,
    };
    compare(
        &device,
        &queue,
        &renderer
            .render_rows(&scene, 0, 0..1, || false)
            .unwrap()
            .unwrap(),
    );
}
#[test]
fn affine_clipped_composition_matches_homogeneous_optics_and_preserves_old_images() {
    let (device, queue) = device();
    for (shear, reversed, offset) in [(true, false, 0.), (true, true, 300000.)] {
        let source = volume(shear, reversed, offset, false);
        let gpu = GpuVolumeSource::new(&device, &queue, source.clone(), || false).unwrap();
        let mut renderer =
            OpaqueMaterialRenderer::new(gpu.clone(), wgpu::TextureFormat::Rgba8Unorm).unwrap();
        let mut state = VolumeView::fit(&source);
        state.projection = Projection::Composite;
        state.camera.yaw_degrees = 31.;
        state.camera.pitch_degrees = 23.;
        state.camera.roll_degrees = 11.;
        let view = ViewState {
            pan: [0.03, -0.02],
            ..Default::default()
        };
        let image = mask(
            gpu.clone(),
            labels(source.clone(), 2),
            &state,
            &view,
            [13, 11],
            7,
        );
        let scene = OpaqueMaterialScene {
            mask: image,
            navigation: view.clone(),
            volume: state.clone(),
            material: material(0.2, 0.4),
            lighting: Lighting::UNLIT,
            mask_luminance: 0.85,
        };
        let old = renderer
            .render_rows(&scene, 0, 0..11, || false)
            .unwrap()
            .unwrap();
        compare(&device, &queue, &old);
        let prior = read(&device, &queue, &old);
        let center = source.center();
        state.clip = Some(PhysicalClip {
            minimum_mm: std::array::from_fn(|a| center[a] - 2.),
            maximum_mm: std::array::from_fn(|a| center[a] + 2.),
        });
        let image = mask(gpu, labels(source, 2), &state, &view, [13, 11], 7);
        let mut scene = scene;
        scene.mask = image;
        scene.volume = state;
        for start in (0..11).step_by(3) {
            let result = renderer
                .render_rows(&scene, 1, start..(start + 3).min(11), || false)
                .unwrap();
            if let Some(image) = result {
                compare(&device, &queue, &image);
            }
        }
        assert_eq!(read(&device, &queue, &old), prior);
    }
}
#[test]
fn scalar_half_cell_gaps_and_missing_samples_are_not_filled_with_source_opacity() {
    let (device, queue) = device();
    for missing in [false, true] {
        let source = volume(false, false, 0., missing);
        let gpu = GpuVolumeSource::new(&device, &queue, source.clone(), || false).unwrap();
        let mut renderer =
            OpaqueMaterialRenderer::new(gpu.clone(), wgpu::TextureFormat::Rgba8Unorm).unwrap();
        let mut state = VolumeView::fit(&source);
        state.projection = Projection::Composite;
        state.camera.target_mm[0] = -0.2 * 0.75;
        let view = ViewState::default();
        let image = mask(gpu, labels(source, 1), &state, &view, [1, 1], 7);
        let scene = OpaqueMaterialScene {
            mask: image,
            navigation: view,
            volume: state,
            material: material(0.2, 0.9),
            lighting: Lighting::UNLIT,
            mask_luminance: 0.8,
        };
        let result = renderer
            .render_rows(&scene, 0, 0..1, || false)
            .unwrap()
            .unwrap();
        compare(&device, &queue, &result);
        let expected = if missing { BACKGROUND } else { 0.8 };
        assert!((read(&device, &queue, &result)[0][0] - expected).abs() < 0.001);
    }
}
#[test]
fn invalid_or_cancelled_scenes_never_publish_or_modify_a_completed_image() {
    let (device, queue) = device();
    let source = volume(false, false, 0., false);
    let gpu = GpuVolumeSource::new(&device, &queue, source.clone(), || false).unwrap();
    let map = labels(source, 2);
    let mut renderer =
        OpaqueMaterialRenderer::new(gpu.clone(), wgpu::TextureFormat::Rgba8Unorm).unwrap();
    let mut state = VolumeView::fit(gpu.volume());
    state.projection = Projection::Composite;
    let view = ViewState::default();
    let image = mask(gpu.clone(), map.clone(), &state, &view, [13, 11], 7);
    let scene = OpaqueMaterialScene {
        mask: image,
        navigation: view.clone(),
        volume: state.clone(),
        material: material(0.2, 0.4),
        lighting: Lighting::UNLIT,
        mask_luminance: 0.8,
    };
    let old = renderer
        .render_rows(&scene, 0, 0..11, || false)
        .unwrap()
        .unwrap();
    let prior = read(&device, &queue, &old);
    assert!(
        renderer
            .render_rows(&scene, 0, 0..3, || false)
            .unwrap()
            .is_none()
    );
    let mut wrong = scene.clone();
    wrong.mask_luminance = 0.4;
    assert!(renderer.render_rows(&wrong, 0, 3..11, || false).is_err());
    renderer.cancel(0).unwrap();
    assert!(matches!(
        renderer.render_rows(&scene, 0, 0..11, || true),
        Err("Cancelled")
    ));
    let calls = AtomicUsize::new(0);
    assert!(matches!(
        renderer.render_rows(&scene, 0, 0..11, || calls.fetch_add(1, Ordering::Relaxed)
            > 0),
        Err("Cancelled")
    ));
    wrong = scene.clone();
    wrong.volume.camera.yaw_degrees += 1.;
    assert!(renderer.render_rows(&wrong, 0, 0..11, || false).is_err());
    wrong = scene.clone();
    wrong.mask_luminance = f32::NAN;
    assert!(renderer.render_rows(&wrong, 0, 0..11, || false).is_err());
    state.camera.target_mm = [10_000_000., 4., 4.];
    state.camera.yaw_degrees = 90.;
    let error = mask(gpu, map, &state, &view, [1, 1], 99);
    let invalid = OpaqueMaterialScene {
        mask: error,
        navigation: view,
        volume: state,
        ..scene.clone()
    };
    assert!(
        matches!(renderer.render_rows(&invalid,0,0..1,||false),Err(message)if message.contains("Mask ray arithmetic"))
    );
    assert_eq!(read(&device, &queue, &old), prior);
    compare(
        &device,
        &queue,
        &renderer
            .render_rows(&scene, 0, 0..11, || false)
            .unwrap()
            .unwrap(),
    );
}

#[test]
fn constrained_real_device_refuses_checked_masks_and_still_renders_materials() {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: crate::configured_backends().unwrap(),
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapter = pollster::block_on(instance.request_adapter(&Default::default()))
        .expect("Actual GPU required");
    let mut descriptor = crate::device_descriptor(&adapter);
    descriptor
        .required_limits
        .max_storage_buffers_per_shader_stage = 0;
    let (device, queue) = pollster::block_on(adapter.request_device(&descriptor)).unwrap();
    let source = volume(false, false, 0., false);
    let gpu = GpuVolumeSource::new(&device, &queue, source.clone(), || false).unwrap();
    assert!(OpaqueMaterialRenderer::new(gpu.clone(), wgpu::TextureFormat::Rgba8Unorm).is_err());
    assert!(GpuMaskProjector::new(gpu.clone()).is_ok());
    let mut renderer =
        crate::material_volume::MaterialVolumeRenderer::new(gpu, wgpu::TextureFormat::Rgba8Unorm)
            .unwrap();
    let mut state = VolumeView::fit(&source);
    state.projection = Projection::Composite;
    let mut encoder = device.create_command_encoder(&Default::default());
    renderer
        .render_target(
            &mut encoder,
            &ViewState::default(),
            &state,
            &material(0.2, 0.4),
            Lighting::UNLIT,
            crate::volume::VolumeTarget {
                slot: 0,
                size: [1, 1],
                region: [[0, 0], [1, 1]],
            },
        )
        .unwrap();
    queue.submit([encoder.finish()]);
    let pixel = linear(&device, &queue, renderer.output_texture(0).unwrap())[0];
    assert!((pixel[0] - 0.2 * (1. - (-3.2f64).exp()) - BACKGROUND * (-3.2f64).exp()).abs() < 0.001);
    assert_eq!(pixel[3], 1.);
}
#[test]
fn checked_images_encode_linear_radiance_once_at_each_framebuffer_format() {
    let (device, queue) = device();
    let source = volume(false, false, 0., false);
    let gpu = GpuVolumeSource::new(&device, &queue, source.clone(), || false).unwrap();
    let mut state = VolumeView::fit(&source);
    state.projection = Projection::Composite;
    let view = ViewState::default();
    let image = mask(gpu.clone(), labels(source, 1), &state, &view, [1, 1], 7);
    let scene = OpaqueMaterialScene {
        mask: image,
        navigation: view,
        volume: state,
        material: material(0.2, 0.4),
        lighting: Lighting::UNLIT,
        mask_luminance: 0.25,
    };
    for format in [
        wgpu::TextureFormat::Rgba8Unorm,
        wgpu::TextureFormat::Rgba8UnormSrgb,
    ] {
        let mut renderer = OpaqueMaterialRenderer::new(gpu.clone(), format).unwrap();
        let image = renderer
            .render_rows(&scene, 0, 0..1, || false)
            .unwrap()
            .unwrap();
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("generated opaque display target"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let target_view = target.create_view(&Default::default());
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("generated checked display"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            image.paint(&mut pass);
        }
        queue.submit([encoder.finish()]);
        let pixel = read_bytes(&device, &queue, &target, 4);
        assert!(
            pixel[..3].iter().all(|v| (i32::from(*v) - 137).abs() <= 1),
            "{format:?}: {pixel:?}"
        );
        assert_eq!(pixel[3], 255);
    }
}

#[test]
fn source_identity_admission_and_row_limits_preserve_the_last_complete_image() {
    let (device, queue) = device();
    let source = volume(false, false, 0., false);
    let gpu = GpuVolumeSource::new(&device, &queue, source.clone(), || false).unwrap();
    let map = labels(source.clone(), 2);
    let mut state = VolumeView::fit(&source);
    state.projection = Projection::Composite;
    let view = ViewState::default();
    let scene = OpaqueMaterialScene {
        mask: mask(gpu.clone(), map.clone(), &state, &view, [1, 257], 7),
        navigation: view.clone(),
        volume: state.clone(),
        material: material(0.2, 0.4),
        lighting: Lighting::UNLIT,
        mask_luminance: 0.8,
    };
    let mut renderer = OpaqueMaterialRenderer::new(gpu, wgpu::TextureFormat::Rgba8Unorm).unwrap();
    let old = renderer
        .render_rows(&scene, 0, 0..257, || false)
        .unwrap()
        .unwrap();
    let prior = read(&device, &queue, &old);
    let other_gpu = GpuVolumeSource::new(&device, &queue, source, || false).unwrap();
    let mut wrong = scene.clone();
    wrong.mask = mask(other_gpu, map, &state, &view, [1, 257], 7);
    assert!(renderer.render_rows(&wrong, 0, 0..257, || false).is_err());
    for change in 0..4 {
        let mut wrong = scene.clone();
        match change {
            0 => wrong.navigation.pan[0] += 0.1,
            1 => {
                wrong.volume.clip = Some(PhysicalClip {
                    minimum_mm: [0.; 3],
                    maximum_mm: [1.; 3],
                })
            }
            2 => wrong.volume.projection = Projection::Maximum,
            _ => wrong.mask_luminance = -0.1,
        }
        assert!(renderer.render_rows(&wrong, 0, 0..257, || false).is_err());
    }
    assert!(renderer.render_rows(&scene, 2, 0..257, || false).is_err());
    assert!(renderer.render_rows(&scene, 0, 1..257, || false).is_err());
    assert!(renderer.render_rows(&scene, 0, 0..258, || false).is_err());
    let budget = MemoryBudget::gpu();
    let occupied = budget.reserve(budget.limit() - budget.used()).unwrap();
    assert!(renderer.render_rows(&scene, 0, 0..257, || false).is_err());
    drop(occupied);
    for row in 0..256 {
        assert!(
            renderer
                .render_rows(&scene, 0, row..row + 1, || false)
                .unwrap()
                .is_none()
        );
    }
    assert!(renderer.render_rows(&scene, 0, 256..257, || false).is_err());
    renderer.cancel(0).unwrap();
    let new = renderer
        .render_rows(&scene, 0, 0..257, || false)
        .unwrap()
        .unwrap();
    compare(&device, &queue, &new);
    assert_eq!(read(&device, &queue, &old), prior);
}
