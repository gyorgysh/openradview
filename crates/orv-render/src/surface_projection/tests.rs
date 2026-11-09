// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use orv_core::{
    labels::Labels,
    mask::MaskOwner,
    segments::{FractionalEncoding, FractionalKind, Segments},
    study::{Geometry, NiftiMethod, NiftiTransform, demo_study},
    surface::{CellFace, CellSurface, FaceBoundary, SurfaceLimits},
    volume::Volume,
};
use std::{cell::Cell, sync::mpsc, time::Duration};

fn device(limits: impl FnOnce(&mut wgpu::Limits)) -> (wgpu::Device, wgpu::Queue) {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        flags: crate::instance_flags(),
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
#[derive(Debug, PartialEq)]
struct Pixels {
    cells: Vec<[u32; 4]>,
    depth: Vec<f32>,
    normals: Vec<[f64; 4]>,
}
fn read(image: &SurfaceImage) -> Pixels {
    let gpu = image.request.surface.source();
    let cells = bytes(gpu.device(), gpu.queue(), image.cells(), 16)
        .as_chunks::<16>()
        .0
        .iter()
        .map(|v| {
            std::array::from_fn(|i| u32::from_le_bytes(v[i * 4..i * 4 + 4].try_into().unwrap()))
        })
        .collect();
    let depth = bytes(gpu.device(), gpu.queue(), image.depth(), 4)
        .as_chunks::<4>()
        .0
        .iter()
        .map(|v| f32::from_le_bytes(*v))
        .collect();
    let normals = bytes(gpu.device(), gpu.queue(), image.normals(), 8)
        .as_chunks::<8>()
        .0
        .iter()
        .map(|v| {
            std::array::from_fn(|i| {
                half(u16::from_le_bytes(v[i * 2..i * 2 + 2].try_into().unwrap()))
            })
        })
        .collect();
    Pixels {
        cells,
        depth,
        normals,
    }
}
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a.into_iter().zip(b).map(|(a, b)| a * b).sum()
}
struct Hit {
    face: CellFace,
    depth: f64,
    edge_distance: f64,
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
            nearest = Some(Hit {
                face: *face,
                depth,
                edge_distance,
            });
        }
    }
    nearest
}
fn word(face: CellFace, number: u16) -> [u32; 4] {
    let [x, y, z] = face.cell();
    [
        x,
        y,
        z,
        u32::from(number)
            | ((face.side() as u32) << 16)
            | (u32::from(face.boundary() == FaceBoundary::Grid) << 24),
    ]
}
fn compare(image: &SurfaceImage) -> usize {
    let pixels = read(image);
    let req = image.request();
    let mut hits = 0;
    for (i, actual) in pixels.cells.iter().enumerate() {
        let expected = reference(req, [i as u32 % req.size[0], i as u32 / req.size[0]]);
        assert!(pixels.depth[i].is_finite());
        if let Some(expected) = expected {
            assert_ne!(actual[3], 0, "Missing hit at pixel {i}");
            assert!(
                (f64::from(pixels.depth[i]) - expected.depth).abs() < 0.0001,
                "Depth at {i}: {} versus {}, edge {}, camera {:?}",
                pixels.depth[i],
                expected.depth,
                expected.edge_distance,
                req.camera
            );
            if expected.edge_distance > 0.0001 {
                assert_eq!(
                    *actual,
                    word(expected.face, req.surface.surface().key().number),
                    "Owner at {i}"
                );
            }
            let side = (actual[3] >> 16) & 7;
            let face = req
                .surface
                .surface()
                .chunks()
                .flat_map(|c| c.faces())
                .find(|f| word(**f, req.surface.surface().key().number) == *actual)
                .unwrap();
            assert_eq!(side, face.side() as u32);
            let normal = req.surface.surface().physical_face(face).unwrap().normal;
            for (a, expected) in normal.into_iter().enumerate() {
                assert!((pixels.normals[i][a] - expected).abs() < 0.0006);
            }
            assert_eq!(pixels.normals[i][3], 1.);
            hits += 1;
        } else {
            assert_eq!(*actual, [0; 4], "Unexpected hit at pixel {i}");
            assert_eq!(pixels.depth[i], 0.);
            assert_eq!(pixels.normals[i], [0.; 4]);
        }
    }
    hits
}

#[test]
fn physical_faces_match_independent_rays_for_orbit_pan_zoom_roll_and_reflection() {
    let (device, queue) = device(|_| {});
    for (shear, reverse, offset) in [(false, false, 0.), (true, true, 1000000.)] {
        let cpu = source([7, 5, 6], shear, reverse, offset);
        let map = labels(
            cpu.clone(),
            |x, y, z| if (x + y * 2 + z * 3) % 7 < 4 { 7 } else { 0 },
        );
        let gpu = GpuVolumeSource::new(&device, &queue, cpu, || false).unwrap();
        let renderer = GpuSurfaceProjector::new(gpu.clone()).unwrap();
        let mut req = request(upload(&gpu, MaskOwner::Manual(map), 7));
        assert!(compare(&renderer.render(req.clone(), || false).unwrap()) > 20);
        for (yaw, pitch, roll) in [(23., -31., 17.), (-117., 24., -47.), (180., 0., 0.)] {
            req.camera.yaw_degrees = yaw;
            req.camera.pitch_degrees = pitch;
            req.camera.roll_degrees = roll;
            req.display.pan = [0.071, -0.043];
            req.display.zoom = 1.31;
            let image = renderer.render(req.clone(), || false).unwrap();
            assert_eq!(image.request().camera, req.camera);
            assert_eq!(image.request().display, req.display);
            assert_eq!(
                image.depth_reference_mm(),
                req.surface.source().volume().center()
            );
            assert_eq!(image.statistics().faces, req.surface.surface().face_count());
            assert!(compare(&image) > 20);
        }
    }
}

#[test]
fn distant_camera_planes_preserve_source_centred_ray_lines_and_depth() {
    let (device, queue) = device(|_| {});
    let cpu = source([7, 5, 6], true, true, 1000000.);
    let map = labels(
        cpu.clone(),
        |x, y, z| if (x + 2 * y + 3 * z) % 7 < 4 { 7 } else { 0 },
    );
    let gpu = GpuVolumeSource::new(&device, &queue, cpu, || false).unwrap();
    let renderer = GpuSurfaceProjector::new(gpu.clone()).unwrap();
    let mut req = request(upload(&gpu, MaskOwner::Manual(map), 7));
    req.camera.yaw_degrees = 23.;
    req.camera.pitch_degrees = -31.;
    req.camera.roll_degrees = 17.;
    let old = renderer.render(req.clone(), || false).unwrap();
    assert!(compare(&old) > 20);
    let saved = read(&old);
    let direction = old.direction_unit();
    for distance in [-1000000., 1000000.] {
        let mut changed = req.clone();
        for (a, d) in direction.into_iter().enumerate() {
            changed.camera.target_mm[a] += distance * d;
        }
        let image = renderer.render(changed.clone(), || false).unwrap();
        assert_eq!(image.request().camera, changed.camera);
        assert!(compare(&image) > 20);
        let actual = read(&image);
        assert_eq!(actual.cells, saved.cells);
        assert_eq!(actual.normals, saved.normals);
        assert!(
            actual
                .depth
                .iter()
                .zip(&saved.depth)
                .all(|(a, b)| (a - b).abs() < 0.00002)
        );
    }
}

#[test]
fn clipping_discards_interfaces_without_inventing_cut_caps() {
    let (device, queue) = device(|_| {});
    let cpu = source([7, 5, 6], false, false, 0.);
    let gpu = GpuVolumeSource::new(&device, &queue, cpu.clone(), || false).unwrap();
    let map = labels(cpu, |_, _, _| 7);
    let renderer = GpuSurfaceProjector::new(gpu.clone()).unwrap();
    let mut req = request(upload(&gpu, MaskOwner::Manual(map), 7));
    let first = renderer.render(req.clone(), || false).unwrap();
    assert!(compare(&first) > 20);
    req.clip = Some(PhysicalClip {
        minimum_mm: [-10., 1.1, -10.],
        maximum_mm: [20.; 3],
    });
    let clipped = renderer.render(req.clone(), || false).unwrap();
    assert!(compare(&clipped) > 20);
    let pixels = read(&clipped);
    for (cell, normal) in pixels.cells.into_iter().zip(pixels.normals) {
        if cell[3] != 0 {
            assert_eq!(
                (cell[3] >> 16) & 7,
                3,
                "Only the real Y+ back interface remains"
            );
            assert_eq!(normal, [0., 1., 0., 1.]);
        }
    }
    assert!(
        read(&first)
            .cells
            .iter()
            .any(|cell| (cell[3] >> 16) & 7 == 2)
    );
    req.clip = Some(PhysicalClip {
        minimum_mm: [100.; 3],
        maximum_mm: [101.; 3],
    });
    assert_eq!(compare(&renderer.render(req, || false).unwrap()), 0);
}

#[test]
fn missing_source_and_unknown_fractional_planes_stay_open_with_full_u16_ids() {
    let (device, queue) = device(|_| {});
    let cpu = source([7, 5, 6], true, true, 1000000.);
    let gpu = GpuVolumeSource::new(&device, &queue, cpu.clone(), || false).unwrap();
    let mut payload = vec![0; 4 * 35];
    payload[..35].fill(100);
    payload[70..105].fill(200);
    let map = Arc::new(
        Segments::from_fractional(
            cpu,
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
    let renderer = GpuSurfaceProjector::new(gpu.clone()).unwrap();
    for number in [65535, 9] {
        let mut req = request(upload(&gpu, MaskOwner::Supplied(map.clone()), number));
        req.camera.pitch_degrees = 90.;
        let image = renderer.render(req, || false).unwrap();
        assert_eq!(image.key().number, number);
        assert!(compare(&image) > 20);
        let pixels = read(&image);
        assert!(pixels.cells.iter().any(|cell| (cell[3] >> 16) & 7 == 5));
        for cell in pixels.cells.into_iter().filter(|cell| cell[3] != 0) {
            assert_eq!(cell[3] & 65535, u32::from(number));
            assert_ne!(
                (cell[3] >> 16) & 7,
                4,
                "No Z- cap against unknown Z0; sheared side walls remain valid"
            );
        }
    }
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
    let map = labels(cpu.clone(), |_, _, z| if z == 1 { 7 } else { 0 });
    let gpu = GpuVolumeSource::new(&device, &queue, cpu, || false).unwrap();
    let renderer = GpuSurfaceProjector::new(gpu.clone()).unwrap();
    let mut req = request(upload(&gpu, MaskOwner::Manual(map), 7));
    req.camera.pitch_degrees = 90.;
    let image = renderer.render(req, || false).unwrap();
    assert!(compare(&image) > 20);
    assert!(
        read(&image)
            .cells
            .iter()
            .filter(|v| v[3] != 0)
            .all(|v| (v[3] >> 16) & 7 == 5)
    );
}

#[test]
fn immutable_outputs_empty_clear_cancellation_and_admission_preserve_prior_images() {
    let (device, queue) = device(|_| {});
    let cpu = source([7, 5, 6], false, false, 0.);
    let map = labels(cpu.clone(), |_, _, z| if z == 2 { 7 } else { 0 });
    let gpu = GpuVolumeSource::new(&device, &queue, cpu.clone(), || false).unwrap();
    let renderer = GpuSurfaceProjector::new(gpu.clone()).unwrap();
    let req = request(upload(&gpu, MaskOwner::Manual(map.clone()), 7));
    fence(&device);
    let before_gpu = MemoryBudget::gpu().used();
    let before_cpu = MemoryBudget::volumes().used();
    let old = renderer.render(req.clone(), || false).unwrap();
    let saved = read(&old);
    assert!(saved.cells.iter().any(|v| v[3] != 0));
    assert_eq!(old.key(), req.surface.surface().key());
    assert_eq!(old.revision(), req.surface.surface().revision());
    let mut changed = map.try_clone(|| false).unwrap();
    changed.threshold(None, [0., 20.], 7, || false).unwrap();
    let changed = request(upload(&gpu, MaskOwner::Manual(Arc::new(changed)), 7));
    let fresh = renderer.render(changed, || false).unwrap();
    assert_ne!(old.revision(), fresh.revision());
    assert!(compare(&fresh) > 20);
    assert_eq!(read(&old), saved);
    drop(fresh);
    let empty = labels(cpu, |_, _, _| 0);
    let empty = request(upload(&gpu, MaskOwner::Manual(empty), 7));
    let empty = renderer.render(empty, || false).unwrap();
    assert_eq!(
        empty.statistics(),
        ProjectionStatistics {
            fenced_batches: 1,
            ..Default::default()
        }
    );
    assert_eq!(compare(&empty), 0);
    drop(empty);
    fence(&device);
    let old_gpu = MemoryBudget::gpu().used();
    let old_cpu = MemoryBudget::volumes().used();
    assert!(renderer.render(req.clone(), || true).is_err());
    let entered = Cell::new(false);
    assert!(
        renderer
            .render(req.clone(), || {
                let allocated = MemoryBudget::gpu().used() > old_gpu;
                entered.set(entered.get() || allocated);
                allocated
            })
            .is_err()
    );
    assert!(entered.get());
    fence(&device);
    assert_eq!(MemoryBudget::gpu().used(), old_gpu);
    assert_eq!(MemoryBudget::volumes().used(), old_cpu);
    let budget = MemoryBudget::gpu();
    let limit = budget.limit();
    budget.set_limit(0);
    let refused = renderer.render(req.clone(), || false).is_err();
    budget.set_limit(limit);
    assert!(refused);
    let budget = MemoryBudget::volumes();
    let limit = budget.limit();
    budget.set_limit(0);
    let refused = renderer.render(req.clone(), || false).is_err();
    budget.set_limit(limit);
    assert!(refused);
    fence(&device);
    assert_eq!(MemoryBudget::gpu().used(), old_gpu);
    assert_eq!(MemoryBudget::volumes().used(), old_cpu);
    assert_eq!(read(&old), saved);
    drop(old);
    fence(&device);
    assert_eq!(MemoryBudget::gpu().used(), before_gpu);
    assert_eq!(MemoryBudget::volumes().used(), before_cpu);
}

#[test]
fn requested_2048_resolution_is_retained_and_every_output_is_valid() {
    let (device, queue) = device(|_| {});
    let cpu = source([7, 5, 6], false, false, 0.);
    let gpu = GpuVolumeSource::new(&device, &queue, cpu.clone(), || false).unwrap();
    let renderer = GpuSurfaceProjector::new(gpu.clone()).unwrap();
    let mut req = request(upload(&gpu, MaskOwner::Manual(labels(cpu, |_, _, _| 7)), 7));
    req.size = [2048; 2];
    let image = renderer.render(req, || false).unwrap();
    for texture in [image.cells(), image.depth(), image.normals()] {
        assert_eq!([texture.width(), texture.height()], [2048; 2]);
    }
    let pixels = read(&image);
    let legal: std::collections::HashSet<_> = image
        .request
        .surface
        .surface()
        .chunks()
        .flat_map(|c| c.faces())
        .map(|face| word(*face, 7))
        .collect();
    let mut hits = 0;
    for i in 0..pixels.cells.len() {
        if pixels.cells[i][3] == 0 {
            assert_eq!(pixels.cells[i], [0; 4]);
            assert_eq!(pixels.depth[i], 0.);
            assert_eq!(pixels.normals[i], [0.; 4]);
        } else {
            assert!(legal.contains(&pixels.cells[i]));
            assert_eq!((pixels.cells[i][3] >> 16) & 7, 2);
            assert!((pixels.depth[i] + 3.75).abs() < 0.00001);
            assert_eq!(pixels.normals[i], [0., -1., 0., 1.]);
            hits += 1;
        }
    }
    assert!(hits > 1000000);
    for y in (17..2048).step_by(127) {
        for x in (31..2048).step_by(131) {
            let i = (y * 2048 + x) as usize;
            if let Some(hit) = reference(image.request(), [x, y]) {
                assert!((f64::from(pixels.depth[i]) - hit.depth).abs() < 0.00001);
                assert_eq!(pixels.cells[i], word(hit.face, 7));
            } else {
                assert_eq!(pixels.cells[i], [0; 4]);
            }
        }
    }
}

#[test]
fn bounded_batches_accumulate_depth_and_cancel_after_submission_without_publishing() {
    let (device, queue) = device(|_| {});
    let cpu = source([40; 3], false, false, 0.);
    let map = labels(cpu.clone(), |x, y, z| {
        if (x + y + z).is_multiple_of(2) { 7 } else { 0 }
    });
    let gpu = GpuVolumeSource::new(&device, &queue, cpu, || false).unwrap();
    let renderer = GpuSurfaceProjector::new(gpu.clone()).unwrap();
    let mut req = request(upload(&gpu, MaskOwner::Manual(map), 7));
    req.camera.yaw_degrees = 21.;
    req.camera.pitch_degrees = -13.;
    req.size = [32, 24];
    assert!(req.surface.surface().face_count() > MAX_BATCH_FACES as usize);
    let image = renderer.render(req.clone(), || false).unwrap();
    assert_eq!(image.statistics().faces, req.surface.surface().face_count());
    assert_eq!(image.statistics().fenced_batches, 2);
    let saved = read(&image);
    for [x, y] in [[11, 9], [13, 10], [15, 11], [17, 12], [19, 13], [21, 14]] {
        let hit = reference(&req, [x, y]).unwrap();
        let i = (y * 32 + x) as usize;
        assert_eq!(saved.cells[i], word(hit.face, 7));
        assert!((f64::from(saved.depth[i]) - hit.depth).abs() < 0.0001);
    }
    fence(&device);
    let before_gpu = MemoryBudget::gpu().used();
    let before_cpu = MemoryBudget::volumes().used();
    let mut faces = 0;
    let first_draws = req
        .surface
        .chunks()
        .take_while(|c| {
            if faces >= MAX_BATCH_FACES as usize {
                return false;
            }
            faces += c.cpu_chunk().faces().len();
            true
        })
        .count();
    let calls = Cell::new(0);
    let cancel_at_fence = 3 + first_draws;
    assert!(
        renderer
            .render(req, || {
                calls.set(calls.get() + 1);
                calls.get() >= cancel_at_fence
            })
            .is_err()
    );
    assert_eq!(calls.get(), cancel_at_fence);
    fence(&device);
    assert_eq!(MemoryBudget::gpu().used(), before_gpu);
    assert_eq!(MemoryBudget::volumes().used(), before_cpu);
    assert_eq!(read(&image), saved);
}

#[test]
fn source_request_and_device_limits_refuse_incompatible_projection() {
    let (device, queue) = device(|_| {});
    let cpu = source([7, 5, 6], false, false, 0.);
    let map = labels(cpu.clone(), |_, _, _| 7);
    let gpu = GpuVolumeSource::new(&device, &queue, cpu.clone(), || false).unwrap();
    let renderer = GpuSurfaceProjector::new(gpu.clone()).unwrap();
    let req = request(upload(&gpu, MaskOwner::Manual(map.clone()), 7));
    let duplicate = GpuVolumeSource::new(&device, &queue, cpu, || false).unwrap();
    assert!(
        renderer
            .render(
                request(upload(&duplicate, MaskOwner::Manual(map), 7)),
                || false
            )
            .is_err()
    );
    for size in [[0, 17], [17, 0], [2049, 17], [17, 2049], [2048, 1]] {
        let mut invalid = req.clone();
        invalid.size = size;
        assert!(renderer.render(invalid, || false).is_err());
    }
    let mut invalid = req.clone();
    invalid.camera.vertical_span_mm = f64::NAN;
    assert!(renderer.render(invalid, || false).is_err());
    let mut invalid = req.clone();
    invalid.display.transform.flip_horizontal = true;
    assert!(renderer.render(invalid, || false).is_err());
    let mut invalid = req.clone();
    invalid.clip = Some(PhysicalClip {
        minimum_mm: [1.; 3],
        maximum_mm: [0.; 3],
    });
    assert!(renderer.render(invalid, || false).is_err());
    let mut invalid = req;
    invalid.clip = Some(PhysicalClip {
        minimum_mm: [1000000.; 3],
        maximum_mm: [1000000.000001; 3],
    });
    assert!(renderer.render(invalid, || false).is_err());
    for restriction in 0..3 {
        let (small_device, small_queue) = self::device(|limits| match restriction {
            0 => limits.max_color_attachments = 2,
            1 => limits.max_color_attachment_bytes_per_sample = 24,
            _ => limits.max_uniform_buffer_binding_size = 256,
        });
        let cpu = source([7, 5, 6], false, false, 0.);
        let small = GpuVolumeSource::new(&small_device, &small_queue, cpu, || false).unwrap();
        assert!(GpuSurfaceProjector::new(small).is_err());
    }
    let cpu = source([7, 5, 6], false, false, 0.);
    let cpu = Arc::new(
        Volume::new(
            cpu.frames()
                .iter()
                .map(|f| {
                    let mut f = (**f).clone();
                    f.geometry.as_mut().unwrap().spacing[0] = 0.000001;
                    Arc::new(f)
                })
                .collect(),
        )
        .unwrap(),
    );
    let map = labels(cpu.clone(), |_, _, _| 7);
    let gpu = GpuVolumeSource::new(&device, &queue, cpu, || false).unwrap();
    let renderer = GpuSurfaceProjector::new(gpu.clone()).unwrap();
    let mut req = request(upload(&gpu, MaskOwner::Manual(map), 7));
    req.camera.pitch_degrees = 90.;
    assert_eq!(
        renderer.render(req, || false).err(),
        Some("Surface depth range exceeds supported GPU precision")
    );
    let (small_device, small_queue) = self::device(|limits| {
        limits.max_texture_dimension_2d = 32;
        limits.max_compute_workgroups_per_dimension = 0;
        limits.max_storage_buffers_per_shader_stage = 0;
        limits.max_storage_textures_per_shader_stage = 0;
    });
    let cpu = source([7, 5, 6], false, false, 0.);
    let map = labels(cpu.clone(), |_, _, _| 7);
    let gpu = GpuVolumeSource::new(&small_device, &small_queue, cpu, || false).unwrap();
    let renderer = GpuSurfaceProjector::new(gpu.clone()).unwrap();
    let mut req = request(upload(&gpu, MaskOwner::Manual(map), 7));
    assert!(renderer.render(req.clone(), || false).is_err());
    req.size = [31, 29];
    assert!(compare(&renderer.render(req, || false).unwrap()) > 20);
}
