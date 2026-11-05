// SPDX-License-Identifier: GPL-3.0-or-later
mod acceleration;
mod physical_stroke;
mod pick;
use super::*;
use crate::{
    surface_projection::{GpuSurfaceProjector, SurfaceProjection},
    surface_source::GpuCellSurface,
};
use orv_core::{
    labels::Labels,
    mask::MaskOwner,
    segments::{FractionalEncoding, FractionalKind, Segments},
    study::{Geometry, NiftiMethod, NiftiTransform, demo_study},
    surface::{CellFace, CellSurface, SurfaceLimits},
    volume::Volume,
};
use orv_core::{
    volume_material::MaterialPoint,
    volume_view::{Camera, PhysicalClip, Projection, Quality},
};
use std::{cell::Cell, time::Duration};

fn device(limits: impl FnOnce(&mut wgpu::Limits)) -> (wgpu::Device, wgpu::Queue) {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: crate::configured_backends().unwrap(),
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapter = pollster::block_on(instance.request_adapter(&Default::default()))
        .expect("Actual GPU required for cell surface projection");
    let mut descriptor = crate::device_descriptor(&adapter);
    limits(&mut descriptor.required_limits);
    pollster::block_on(adapter.request_device(&descriptor)).unwrap()
}
fn source(dim: [u32; 3], shear: bool, reverse: bool, offset: f64) -> Arc<Volume> {
    let template = demo_study().series.remove(0).frames.remove(0);
    Arc::new(
        Volume::new(
            (0..dim[2])
                .map(|z| {
                    let mut f = (*template).clone();
                    f.width = dim[0];
                    f.height = dim[1];
                    f.pixels = vec![10.; (dim[0] * dim[1]) as usize].into();
                    f.geometry = Some(Geometry {
                        position: [
                            offset + if shear { f64::from(z) * 0.25 } else { 0. },
                            -offset + if shear { f64::from(z) * 0.5 } else { 0. },
                            offset + f64::from(z) * 2.,
                        ],
                        orientation: if reverse {
                            [-1., 0., 0., 0., 1., 0.]
                        } else {
                            [1., 0., 0., 0., 1., 0.]
                        },
                        spacing: [1.5, 0.75],
                        frame_of_reference: Some("generated cell projection".into()),
                        nifti_transform: None,
                    });
                    Arc::new(f)
                })
                .collect(),
        )
        .unwrap(),
    )
}
fn labels(source: Arc<Volume>, pattern: impl Fn(usize, usize, usize) -> u8) -> Arc<Labels> {
    let width = source.dimensions()[0];
    let frames = source
        .frames()
        .iter()
        .enumerate()
        .map(|(z, f)| {
            let mut f = (**f).clone();
            f.validity = None;
            f.geometry.as_mut().unwrap().nifti_transform = Some(NiftiTransform {
                method: NiftiMethod::Sform,
                code: 1,
            });
            f.pixels = (0..f.pixels.len())
                .map(|i| f32::from(pattern(i % width, i / width, z)))
                .collect::<Vec<_>>()
                .into();
            Arc::new(f)
        })
        .collect();
    Arc::new(Labels::import(source, &Volume::new(frames).unwrap(), || false).unwrap())
}
fn upload(gpu: &Arc<GpuVolumeSource>, owner: MaskOwner, number: u16) -> Arc<GpuCellSurface> {
    let surface = CellSurface::build(owner, number, None, SurfaceLimits::default(), || false)
        .unwrap()
        .0;
    GpuCellSurface::upload(gpu.clone(), surface, None, || false)
        .unwrap()
        .0
}
fn request(surface: Arc<GpuCellSurface>) -> SurfaceProjection {
    SurfaceProjection {
        camera: Camera::fit(surface.source().volume()),
        surface,
        display: ViewState {
            pan: [0.021, -0.017],
            ..Default::default()
        },
        clip: None,
        size: [37, 29],
    }
}
fn fence(device: &wgpu::Device) {
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(Duration::from_secs(30)),
        })
        .unwrap();
}
fn bytes(device: &wgpu::Device, queue: &wgpu::Queue, texture: &wgpu::Texture, bpp: u32) -> Vec<u8> {
    let size = texture.size();
    let stride = (size.width * bpp).div_ceil(256) * 256;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("generated surface image readback"),
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
    buffer.map_async(wgpu::MapMode::Read, .., move |r| {
        let _ = tx.send(r);
    });
    fence(device);
    rx.recv_timeout(Duration::from_secs(10)).unwrap().unwrap();
    let mapped = buffer.get_mapped_range(..).unwrap();
    let result = mapped
        .chunks_exact(stride as usize)
        .flat_map(|row| row[..(size.width * bpp) as usize].iter().copied())
        .collect();
    drop(mapped);
    buffer.unmap();
    result
}
fn half(bits: u16) -> f64 {
    let sign = if bits & 32768 == 0 { 1. } else { -1. };
    let exponent = (bits >> 10) & 31;
    let fraction = f64::from(bits & 1023);
    assert_ne!(exponent, 31, "Non-finite surface normal");
    sign * if exponent == 0 {
        fraction * 2f64.powi(-24)
    } else {
        (1. + fraction / 1024.) * 2f64.powi(i32::from(exponent) - 15)
    }
}
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a.into_iter().zip(b).map(|(a, b)| a * b).sum()
}
struct Hit {
    face: CellFace,
    depth: f64,
}
// Deliberately intersects f64 source-cell planes, independently of GPU matrices,
// depth-buffer values and triangle coverage. Edge contacts are not edit authority.
fn reference(req: &SurfaceProjection, pixel: [u32; 2]) -> Option<Hit> {
    let source = req.surface.source().volume();
    let frame = req
        .camera
        .frame(
            &req.display,
            f64::from(req.size[0]) / f64::from(req.size[1]),
        )
        .unwrap();
    let (world, direction) = frame.ray(std::array::from_fn(|a| {
        (f64::from(pixel[a]) + 0.5) / f64::from(req.size[a])
    }));
    let origin = source.world_to_voxel(world);
    let vector = source.world_vector_to_voxel(direction);
    let mut nearest: Option<Hit> = None;
    for face in req.surface.surface().chunks().flat_map(|c| c.faces()) {
        let side = face.side() as usize;
        let axis = side / 2;
        if vector[axis].abs() < 1e-14 {
            continue;
        }
        let center = face.cell().map(f64::from);
        let plane = center[axis] + if side.is_multiple_of(2) { -0.5 } else { 0.5 };
        let t = (plane - origin[axis]) / vector[axis];
        let p: [f64; 3] = std::array::from_fn(|a| origin[a] + t * vector[a]);
        let edge_distance = (0..3)
            .filter(|a| *a != axis)
            .map(|a| 0.5 - (p[a] - center[a]).abs())
            .fold(f64::INFINITY, f64::min);
        if edge_distance < -1e-10 {
            continue;
        }
        let position = source.voxel_to_world(p);
        if req.clip.as_ref().is_some_and(|clip| {
            (0..3).any(|a| position[a] < clip.minimum_mm[a] || position[a] > clip.maximum_mm[a])
        }) {
            continue;
        }
        let depth = dot(
            std::array::from_fn(|a| position[a] - source.center()[a]),
            direction,
        );
        if nearest.as_ref().is_none_or(|hit| depth < hit.depth - 1e-10) {
            nearest = Some(Hit { face: *face, depth });
        }
    }
    nearest
}

const BACKGROUND: f64 = 0.0030352698;
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
fn project(
    gpu: &Arc<GpuVolumeSource>,
    owner: MaskOwner,
    number: u16,
    state: &VolumeView,
    view: &ViewState,
    size: [u32; 2],
) -> Arc<SurfaceImage> {
    let mut req = request(upload(gpu, owner, number));
    req.camera = state.camera.clone();
    req.display = view.clone();
    req.clip = state.clip.clone();
    req.size = size;
    GpuSurfaceProjector::new(gpu.clone())
        .unwrap()
        .render(req, || false)
        .unwrap()
}
fn scene(
    gpu: &Arc<GpuVolumeSource>,
    owner: MaskOwner,
    number: u16,
    size: [u32; 2],
) -> OpaqueSurfaceScene {
    let mut volume = VolumeView::fit(gpu.volume());
    volume.projection = Projection::Composite;
    let navigation = ViewState {
        pan: [0.021, -0.017],
        ..Default::default()
    };
    OpaqueSurfaceScene {
        surface: project(gpu, owner, number, &volume, &navigation, size),
        navigation,
        volume,
        material: material(0.2, 0.4),
        lighting: Lighting::UNLIT,
        surface_lighting: Lighting::UNLIT,
        surface_luminance: 0.85,
    }
}
fn linear(image: &OpaqueSurfaceImage) -> Vec<[f64; 4]> {
    let gpu = image.scene.surface.request().surface.source();
    bytes(gpu.device(), gpu.queue(), image.texture(), 8)
        .as_chunks::<8>()
        .0
        .iter()
        .map(|v| std::array::from_fn(|a| half(u16::from_le_bytes([v[a * 2], v[a * 2 + 1]]))))
        .collect()
}
fn range(origin: [f64; 3], direction: [f64; 3], lo: [f64; 3], hi: [f64; 3]) -> Option<[f64; 2]> {
    let mut t = [f64::NEG_INFINITY, f64::INFINITY];
    for a in 0..3 {
        if direction[a] == 0. {
            if origin[a] < lo[a] || origin[a] > hi[a] {
                return None;
            }
        } else {
            let p = (lo[a] - origin[a]) / direction[a];
            let q = (hi[a] - origin[a]) / direction[a];
            t[0] = t[0].max(p.min(q));
            t[1] = t[1].min(p.max(q));
        }
    }
    (t[0] < t[1]).then_some(t)
}
// Split at source interpolation-cell boundaries, independently of GPU integration
// steps. The generated sources have constant optics; validity is constant within
// each open interpolation cell, so Beer-Lambert attenuation is analytic.
fn analytic(scene: &OpaqueSurfaceScene, pixel: [u32; 2]) -> f64 {
    let req = scene.surface.request();
    let source = req.surface.source().volume();
    let frame = scene
        .volume
        .camera
        .frame(
            &scene.navigation,
            f64::from(req.size[0]) / f64::from(req.size[1]),
        )
        .unwrap();
    let (world, direction) = frame.ray(std::array::from_fn(|a| {
        (f64::from(pixel[a]) + 0.5) / f64::from(req.size[a])
    }));
    let origin = source.world_to_voxel(world);
    let vector = source.world_vector_to_voxel(direction);
    let hit = reference(req, pixel);
    let mut interval = range(
        origin,
        vector,
        [0.; 3],
        source.dimensions().map(|v| (v - 1) as f64),
    );
    if let Some(clip) = &scene.volume.clip {
        interval = interval
            .zip(range(world, direction, clip.minimum_mm, clip.maximum_mm))
            .map(|(a, b)| [a[0].max(b[0]), a[1].min(b[1])]);
    }
    let mut length = 0.;
    if let Some(mut interval) = interval {
        if let Some(hit) = &hit {
            let target_depth = dot(
                std::array::from_fn(|a| world[a] - source.center()[a]),
                direction,
            );
            interval[1] = interval[1].min(hit.depth - target_depth);
        }
        if interval[0] < interval[1] {
            let mut breaks = vec![interval[0], interval[1]];
            for a in 0..3 {
                if vector[a] != 0. {
                    for plane in 0..source.dimensions()[a] {
                        let t = (plane as f64 - origin[a]) / vector[a];
                        if t > interval[0] && t < interval[1] {
                            breaks.push(t);
                        }
                    }
                }
            }
            breaks.sort_by(f64::total_cmp);
            breaks.dedup();
            for pair in breaks.windows(2) {
                let midpoint = (pair[0] + pair[1]) * 0.5;
                if source
                    .sample(std::array::from_fn(|a| world[a] + direction[a] * midpoint))
                    .is_some()
                {
                    length += pair[1] - pair[0];
                }
            }
        }
    }
    let optics = scene.material.sample(10.).unwrap();
    let transmission = (-optics.extinction_per_mm * length).exp();
    let behind = hit.map_or(BACKGROUND, |hit| {
        let normal = req
            .surface
            .surface()
            .physical_face(&hit.face)
            .unwrap()
            .normal;
        f64::from(scene.surface_luminance)
            * (f64::from(scene.surface_lighting.ambient)
                + f64::from(scene.surface_lighting.diffuse) * dot(normal, direction).abs())
    });
    optics.luminance * (1. - transmission) + transmission * behind
}
fn compare(image: &OpaqueSurfaceImage) {
    let size = image.scene.surface.request().size;
    for (i, pixel) in linear(image).iter().enumerate() {
        let expected = analytic(image.scene(), [i as u32 % size[0], i as u32 / size[0]]);
        assert!(
            pixel[..3].iter().all(|v| (v - expected).abs() < 0.001),
            "Pixel {i}: {pixel:?} vs {expected}"
        );
        assert_eq!(pixel[3], 1.);
    }
}

#[test]
fn known_interfaces_receive_foreground_attenuation_and_end_source_integration() {
    let (device, queue) = device(|_| {});
    let cpu = source([7, 5, 6], false, false, 0.);
    let gpu = GpuVolumeSource::new(&device, &queue, cpu.clone(), || false).unwrap();
    let mut renderer =
        OpaqueSurfaceRenderer::new(gpu.clone(), wgpu::TextureFormat::Rgba8Unorm).unwrap();
    for kind in [0, 1, 2] {
        let map = labels(cpu.clone(), |_, y, _| {
            if kind == 1 || (kind == 2 && y == 2) {
                7
            } else {
                0
            }
        });
        let mut scene = scene(&gpu, MaskOwner::Manual(map), 7, [17, 13]);
        for sigma in [0., 0.4, 2.] {
            for lighting in [Lighting::UNLIT, Lighting::HEADLIGHT] {
                scene.material = material(0.2, sigma);
                scene.surface_lighting = lighting;
                for quality in [Quality::Interactive, Quality::Refined] {
                    scene.volume.quality = quality;
                    let image = renderer
                        .render_rows(&scene, 0, 0..13, || false)
                        .unwrap()
                        .unwrap();
                    compare(&image);
                }
            }
        }
    }
}

#[test]
fn affine_camera_clips_and_distant_target_planes_preserve_physical_optics() {
    let (device, queue) = device(|_| {});
    for offset in [0., 1000000.] {
        let cpu = source([7, 5, 6], true, true, offset);
        let gpu = GpuVolumeSource::new(&device, &queue, cpu.clone(), || false).unwrap();
        let owner = MaskOwner::Manual(labels(cpu.clone(), |x, y, z| {
            if (x + 2 * y + 3 * z) % 7 < 4 { 7 } else { 0 }
        }));
        let mut scene = scene(&gpu, owner.clone(), 7, [23, 19]);
        scene.volume.camera.yaw_degrees = 31.;
        scene.volume.camera.pitch_degrees = -23.;
        scene.volume.camera.roll_degrees = 11.;
        scene.navigation.zoom = 1.13;
        scene.surface_lighting = Lighting::HEADLIGHT;
        let mut renderer =
            OpaqueSurfaceRenderer::new(gpu.clone(), wgpu::TextureFormat::Rgba8Unorm).unwrap();
        for clip in [
            None,
            Some(PhysicalClip {
                minimum_mm: cpu.center().map(|v| v - 2.),
                maximum_mm: cpu.center().map(|v| v + 2.),
            }),
        ] {
            scene.volume.clip = clip;
            scene.surface = project(
                &gpu,
                owner.clone(),
                7,
                &scene.volume,
                &scene.navigation,
                [23, 19],
            );
            let original = renderer
                .render_rows(&scene, 0, 0..19, || false)
                .unwrap()
                .unwrap();
            compare(&original);
            let saved = linear(&original);
            let frame = scene
                .volume
                .camera
                .frame(&scene.navigation, 23. / 19.)
                .unwrap();
            let target = scene.volume.camera.target_mm;
            for distance in [-1000000., 1000000.] {
                scene.volume.camera.target_mm =
                    std::array::from_fn(|a| target[a] + distance * frame.direction_unit[a]);
                scene.surface = project(
                    &gpu,
                    owner.clone(),
                    7,
                    &scene.volume,
                    &scene.navigation,
                    [23, 19],
                );
                let mut completed = None;
                for row in (0..19).step_by(3) {
                    completed = renderer
                        .render_rows(&scene, 1, row..(row + 3).min(19), || false)
                        .unwrap();
                }
                let image = completed.unwrap();
                compare(&image);
                assert_eq!(image.scene().volume, scene.volume);
                assert!(
                    linear(&image)
                        .iter()
                        .flatten()
                        .zip(saved.iter().flatten())
                        .all(|(a, b)| (a - b).abs() < 0.001)
                );
                assert_eq!(linear(&original), saved);
            }
            scene.volume.camera.target_mm = target;
        }
    }
}

#[test]
fn partial_missing_source_stencils_preserve_the_exact_valid_physical_length() {
    let (device, queue) = device(|_| {});
    let cpu = source([7, 5, 6], false, false, 0.);
    let cpu = Arc::new(
        Volume::new(
            cpu.frames()
                .iter()
                .enumerate()
                .map(|(z, f)| {
                    let mut f = (**f).clone();
                    f.validity = (z == 0).then(|| vec![0; 35].into());
                    Arc::new(f)
                })
                .collect(),
        )
        .unwrap(),
    );
    let gpu = GpuVolumeSource::new(&device, &queue, cpu.clone(), || false).unwrap();
    let owner = MaskOwner::Manual(labels(cpu, |_, _, z| if z == 1 { 7 } else { 0 }));
    let mut scene = scene(&gpu, owner.clone(), 7, [1; 2]);
    scene.navigation.pan = [0.; 2];
    scene.volume.camera.pitch_degrees = 90.;
    scene.surface = project(&gpu, owner, 7, &scene.volume, &scene.navigation, [1; 2]);
    let mut renderer = OpaqueSurfaceRenderer::new(gpu, wgpu::TextureFormat::Rgba8Unorm).unwrap();
    for quality in [Quality::Interactive, Quality::Refined] {
        scene.volume.quality = quality;
        let image = renderer
            .render_rows(&scene, 0, 0..1, || false)
            .unwrap()
            .unwrap();
        compare(&image);
        let expected = 0.2 + (0.85 - 0.2) * (-0.4f64).exp();
        assert!((linear(&image)[0][0] - expected).abs() < 0.001);
    }
}

#[test]
fn sparse_supplied_interfaces_and_clips_expose_real_backfaces_with_foreground_source() {
    let (device, queue) = device(|_| {});
    let cpu = source([7, 5, 6], false, false, 0.);
    let gpu = GpuVolumeSource::new(&device, &queue, cpu.clone(), || false).unwrap();
    let mut payload = vec![0; 4 * 35];
    payload[..35].fill(100);
    payload[70..105].fill(200);
    let map = Arc::new(
        Segments::from_fractional(
            cpu.clone(),
            &[65535, 9],
            &[(65535, 1), (65535, 2), (9, 1), (9, 2)],
            &payload,
            FractionalEncoding {
                maximum: 200,
                kind: FractionalKind::Probability,
            },
            Some(true),
            || false,
        )
        .unwrap(),
    );
    let mut renderer =
        OpaqueSurfaceRenderer::new(gpu.clone(), wgpu::TextureFormat::Rgba8Unorm).unwrap();
    for id in [65535, 9] {
        let owner = MaskOwner::Supplied(map.clone());
        let mut scene = scene(&gpu, owner.clone(), id, [17, 13]);
        scene.volume.camera.pitch_degrees = 90.;
        scene.surface = project(&gpu, owner, id, &scene.volume, &scene.navigation, [17, 13]);
        let image = renderer
            .render_rows(&scene, 0, 0..13, || false)
            .unwrap()
            .unwrap();
        compare(&image);
        assert_eq!(image.scene().surface.key().number, id);
        let pixels = bytes(&device, &queue, scene.surface.cells(), 16);
        assert!(
            pixels
                .as_chunks::<16>()
                .0
                .iter()
                .any(|v| (u32::from_le_bytes(v[12..].try_into().unwrap()) >> 16) & 7 == 5)
        );
    }
    let owner = MaskOwner::Manual(labels(cpu, |_, _, _| 7));
    let mut scene = scene(&gpu, owner.clone(), 7, [1; 2]);
    scene.navigation.pan = [0.; 2];
    scene.volume.clip = Some(PhysicalClip {
        minimum_mm: [-10., 1.1, -10.],
        maximum_mm: [20.; 3],
    });
    scene.surface = project(&gpu, owner, 7, &scene.volume, &scene.navigation, [1; 2]);
    let image = renderer
        .render_rows(&scene, 0, 0..1, || false)
        .unwrap()
        .unwrap();
    compare(&image);
    let expected = 0.2 + (0.85 - 0.2) * (-0.4 * (6. - 1.1f64)).exp();
    assert!((linear(&image)[0][0] - expected).abs() < 0.001);
    assert!(
        linear(&image)[0][0] < 0.4,
        "A clip-start cap would incorrectly hide foreground source"
    );
}

#[test]
fn oblique_missing_interpolation_cells_match_independent_validity_interval_optics() {
    let (device, queue) = device(|_| {});
    for all_missing in [false, true] {
        let cpu = source([7, 5, 6], true, true, 1000000.);
        let cpu = Arc::new(
            Volume::new(
                cpu.frames()
                    .iter()
                    .enumerate()
                    .map(|(z, f)| {
                        let mut f = (**f).clone();
                        f.validity = Some(
                            (0..35)
                                .map(|i| {
                                    u8::from(!all_missing && (i % 7 + 2 * (i / 7) + 3 * z) % 7 != 0)
                                })
                                .collect::<Vec<_>>()
                                .into(),
                        );
                        Arc::new(f)
                    })
                    .collect(),
            )
            .unwrap(),
        );
        let owner = MaskOwner::Manual(labels(cpu.clone(), |x, y, z| {
            if (x + 2 * y + z) % 5 < 3 { 7 } else { 0 }
        }));
        let gpu = GpuVolumeSource::new(&device, &queue, cpu.clone(), || false).unwrap();
        let mut renderer =
            OpaqueSurfaceRenderer::new(gpu.clone(), wgpu::TextureFormat::Rgba8Unorm).unwrap();
        let mut scene = scene(&gpu, owner.clone(), 7, [23, 19]);
        for (yaw, pitch, roll) in [(23., -31., 17.), (-117., 24., -47.), (180., 0., 0.)] {
            scene.volume.camera.yaw_degrees = yaw;
            scene.volume.camera.pitch_degrees = pitch;
            scene.volume.camera.roll_degrees = roll;
            for clip in [
                None,
                Some(PhysicalClip {
                    minimum_mm: cpu.center().map(|v| v - 2.7),
                    maximum_mm: cpu.center().map(|v| v + 2.7),
                }),
            ] {
                scene.volume.clip = clip;
                scene.surface = project(
                    &gpu,
                    owner.clone(),
                    7,
                    &scene.volume,
                    &scene.navigation,
                    [23, 19],
                );
                for quality in [Quality::Interactive, Quality::Refined] {
                    scene.volume.quality = quality;
                    let image = renderer
                        .render_rows(&scene, 0, 0..19, || false)
                        .unwrap()
                        .unwrap();
                    compare(&image);
                }
            }
        }
    }
}

#[test]
fn source_and_surface_headlights_use_independent_physical_normals() {
    let (device, queue) = device(|_| {});
    let cpu = source([7, 5, 6], false, false, 0.);
    let cpu = Arc::new(
        Volume::new(
            cpu.frames()
                .iter()
                .map(|f| {
                    let mut f = (**f).clone();
                    f.pixels = (0..35)
                        .map(|i| (10 + i % 7 * 5) as f32)
                        .collect::<Vec<_>>()
                        .into();
                    Arc::new(f)
                })
                .collect(),
        )
        .unwrap(),
    );
    let owner = MaskOwner::Manual(labels(cpu.clone(), |_, y, _| if y == 2 { 7 } else { 0 }));
    let gpu = GpuVolumeSource::new(&device, &queue, cpu, || false).unwrap();
    let mut scene = scene(&gpu, owner, 7, [1; 2]);
    scene.lighting = Lighting::HEADLIGHT;
    scene.surface_lighting = Lighting::HEADLIGHT;
    let mut renderer = OpaqueSurfaceRenderer::new(gpu, wgpu::TextureFormat::Rgba8Unorm).unwrap();
    let image = renderer
        .render_rows(&scene, 0, 0..1, || false)
        .unwrap()
        .unwrap();
    // Source gradient is X, perpendicular to this Y ray. The mask face is Y.
    let transmission = (-0.4 * 2.25f64).exp();
    let expected = 0.2 * 0.25 * (1. - transmission) + 0.85 * transmission;
    assert!((linear(&image)[0][0] - expected).abs() < 0.001);
}

#[test]
fn rows_identity_cancellation_admission_and_corrupt_inputs_never_replace_completed_images() {
    let (device, queue) = device(|_| {});
    let cpu = source([7, 5, 6], false, false, 0.);
    let owner = MaskOwner::Manual(labels(cpu.clone(), |_, y, _| if y == 2 { 7 } else { 0 }));
    let gpu = GpuVolumeSource::new(&device, &queue, cpu.clone(), || false).unwrap();
    let scene = scene(&gpu, owner.clone(), 7, [13, 257]);
    let mut renderer =
        OpaqueSurfaceRenderer::new(gpu.clone(), wgpu::TextureFormat::Rgba8Unorm).unwrap();
    fence(&device);
    let before_gpu = MemoryBudget::gpu().used();
    let before_cpu = MemoryBudget::volumes().used();
    let old = renderer
        .render_rows(&scene, 0, 0..257, || false)
        .unwrap()
        .unwrap();
    let saved = linear(&old);
    for rows in [1..257, 0..258, 0..0] {
        assert!(renderer.render_rows(&scene, 0, rows, || false).is_err());
    }
    assert!(renderer.render_rows(&scene, 2, 0..257, || false).is_err());
    let other = GpuVolumeSource::new(&device, &queue, cpu, || false).unwrap();
    let wrong = OpaqueSurfaceScene {
        surface: project(
            &other,
            owner.clone(),
            7,
            &scene.volume,
            &scene.navigation,
            [13, 257],
        ),
        ..scene.clone()
    };
    assert!(renderer.render_rows(&wrong, 0, 0..257, || false).is_err());
    drop(wrong);
    drop(other);
    for choice in 0..9 {
        let mut wrong = scene.clone();
        match choice {
            0 => wrong.volume.camera.yaw_degrees += 1.,
            1 => wrong.navigation.pan[0] += 0.1,
            2 => {
                wrong.volume.clip = Some(PhysicalClip {
                    minimum_mm: [-1.; 3],
                    maximum_mm: [1.; 3],
                })
            }
            3 => wrong.surface_luminance = f32::NAN,
            4 => wrong.surface_lighting.ambient = 2.,
            5 => wrong.lighting.diffuse = -1.,
            6 => wrong.volume.projection = Projection::Maximum,
            7 => wrong.navigation.invert = true,
            _ => wrong.navigation.transform.flip_vertical = true,
        }
        assert!(renderer.render_rows(&wrong, 0, 0..257, || false).is_err());
    }
    assert!(
        renderer
            .render_rows(&scene, 0, 0..3, || false)
            .unwrap()
            .is_none()
    );
    for choice in 0..4 {
        let mut wrong = scene.clone();
        match choice {
            0 => wrong.surface_luminance = 0.1,
            1 => wrong.surface_lighting = Lighting::HEADLIGHT,
            2 => wrong.material = material(0.1, 0.2),
            _ => {
                wrong.surface = project(
                    &gpu,
                    owner.clone(),
                    7,
                    &scene.volume,
                    &scene.navigation,
                    [13, 257],
                )
            }
        }
        assert!(renderer.render_rows(&wrong, 0, 3..257, || false).is_err());
    }
    renderer.cancel(0).unwrap();
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
    assert!(renderer.render_rows(&scene, 0, 0..257, || true).is_err());
    let calls = Cell::new(0);
    assert!(
        renderer
            .render_rows(&scene, 0, 0..257, || {
                calls.set(calls.get() + 1);
                calls.get() > 1
            })
            .is_err()
    );
    fence(&device);
    let used_gpu = MemoryBudget::gpu().used();
    let used_cpu = MemoryBudget::volumes().used();
    for cpu in [false, true] {
        let budget = if cpu {
            MemoryBudget::volumes()
        } else {
            MemoryBudget::gpu()
        };
        let limit = budget.limit();
        budget.set_limit(0);
        let refused = renderer.render_rows(&scene, 0, 0..257, || false).is_err();
        budget.set_limit(limit);
        assert!(refused);
    }
    fence(&device);
    assert_eq!(MemoryBudget::gpu().used(), used_gpu);
    assert_eq!(MemoryBudget::volumes().used(), used_cpu);
    for word in [8u32, 7 | (6 << 16), 7 | (1 << 31)] {
        let bad = project(
            &gpu,
            owner.clone(),
            7,
            &scene.volume,
            &scene.navigation,
            [13, 257],
        );
        let bad_scene = OpaqueSurfaceScene {
            surface: bad,
            ..scene.clone()
        };
        clear(
            &device,
            &queue,
            bad_scene.surface.cells(),
            wgpu::Color {
                r: 0.,
                g: 0.,
                b: 0.,
                a: f64::from(word),
            },
        );
        assert!(
            renderer
                .render_rows(&bad_scene, 0, 0..257, || false)
                .is_err()
        );
        assert_eq!(linear(&old), saved);
    }
    let bad = project(&gpu, owner, 7, &scene.volume, &scene.navigation, [13, 257]);
    clear(
        &device,
        &queue,
        bad.depth(),
        wgpu::Color {
            r: 1000.,
            g: 0.,
            b: 0.,
            a: 0.,
        },
    );
    let bad_scene = OpaqueSurfaceScene {
        surface: bad,
        ..scene.clone()
    };
    assert!(
        renderer
            .render_rows(&bad_scene, 0, 0..257, || false)
            .is_err()
    );
    drop(bad_scene);
    let good = renderer
        .render_rows(&scene, 0, 0..257, || false)
        .unwrap()
        .unwrap();
    compare(&good);
    drop(good);
    assert_eq!(linear(&old), saved);
    drop(old);
    fence(&device);
    assert_eq!(MemoryBudget::gpu().used(), before_gpu);
    assert_eq!(MemoryBudget::volumes().used(), before_cpu);
}
fn clear(device: &wgpu::Device, queue: &wgpu::Queue, texture: &wgpu::Texture, color: wgpu::Color) {
    let view = texture.create_view(&Default::default());
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("deliberately invalid generated surface input"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(color),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
    }
    queue.submit([encoder.finish()]);
}

#[test]
fn complete_2048_output_keeps_requested_detail_and_matches_sampled_optics() {
    let (device, queue) = device(|_| {});
    let cpu = source([7, 5, 6], false, false, 0.);
    let owner = MaskOwner::Manual(labels(cpu.clone(), |_, y, _| if y == 2 { 7 } else { 0 }));
    let gpu = GpuVolumeSource::new(&device, &queue, cpu, || false).unwrap();
    let scene = scene(&gpu, owner, 7, [2048; 2]);
    let mut renderer = OpaqueSurfaceRenderer::new(gpu, wgpu::TextureFormat::Rgba8Unorm).unwrap();
    let image = renderer
        .render_rows(&scene, 0, 0..2048, || false)
        .unwrap()
        .unwrap();
    assert_eq!(
        [image.texture().width(), image.texture().height()],
        [2048; 2]
    );
    let pixels = linear(&image);
    assert_eq!(pixels.len(), 2048 * 2048);
    assert!(pixels.iter().all(|v| v[0] >= 0.
        && v[0] <= 1.
        && v[0] == v[1]
        && v[0] == v[2]
        && v[3] == 1.));
    for y in (17..2048).step_by(127) {
        for x in (31..2048).step_by(131) {
            let expected = analytic(&scene, [x, y]);
            assert!((pixels[(y * 2048 + x) as usize][0] - expected).abs() < 0.001);
        }
    }
}

#[test]
fn checked_surface_images_encode_once_and_constrained_devices_refuse_composition() {
    let (device, queue) = device(|_| {});
    let cpu = source([7, 5, 6], false, false, 0.);
    let owner = MaskOwner::Manual(labels(cpu.clone(), |_, _, _| 7));
    let gpu = GpuVolumeSource::new(&device, &queue, cpu, || false).unwrap();
    let mut scene = scene(&gpu, owner, 7, [1; 2]);
    scene.surface_luminance = 0.25;
    for format in [
        wgpu::TextureFormat::Rgba8Unorm,
        wgpu::TextureFormat::Rgba8UnormSrgb,
    ] {
        let mut renderer = OpaqueSurfaceRenderer::new(gpu.clone(), format).unwrap();
        let image = renderer
            .render_rows(&scene, 0, 0..1, || false)
            .unwrap()
            .unwrap();
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("generated surface display target"),
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
        let view = target.create_view(&Default::default());
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("generated surface presentation"),
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
            image.paint(&mut pass);
        }
        queue.submit([encoder.finish()]);
        let actual = bytes(&device, &queue, &target, 4);
        assert!(actual[..3].iter().all(|v| (i32::from(*v) - 137).abs() <= 1));
        assert_eq!(actual[3], 255);
    }
    for choice in 0..3 {
        let (device, queue) = self::device(|limits| match choice {
            0 => limits.max_storage_buffers_per_shader_stage = 0,
            1 => limits.max_sampled_textures_per_shader_stage = 3,
            _ => limits.max_uniform_buffer_binding_size = 256,
        });
        let cpu = source([7, 5, 6], false, false, 0.);
        let gpu = GpuVolumeSource::new(&device, &queue, cpu, || false).unwrap();
        assert!(OpaqueSurfaceRenderer::new(gpu.clone(), wgpu::TextureFormat::Rgba8Unorm).is_err());
        if choice < 2 {
            assert!(GpuSurfaceProjector::new(gpu).is_ok());
        }
    }
}
