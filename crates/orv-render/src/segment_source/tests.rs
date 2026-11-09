// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use crate::{
    mask_input::MaskInput,
    mask_projection::{GpuMaskProjector, MaskImage, MaskProjection, RAY_ERROR},
};
use orv_core::{
    ViewState,
    segments::{FractionalEncoding, FractionalKind},
    study::{Geometry, demo_study},
    volume::Volume,
    volume_view::{Camera, PhysicalClip},
};
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    time::Duration,
};

fn device() -> (wgpu::Device, wgpu::Queue) {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        flags: crate::instance_flags(),
        backends: crate::configured_backends().unwrap(),
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapter = pollster::block_on(instance.request_adapter(&Default::default()))
        .expect("Actual GPU required");
    pollster::block_on(adapter.request_device(&crate::device_descriptor(&adapter))).unwrap()
}
fn source(dim: [u32; 3], sheared: bool, reversed: bool, offset: f64) -> Arc<Volume> {
    let template = demo_study().series.remove(0).frames.remove(0);
    Arc::new(
        Volume::new(
            (0..dim[2])
                .map(|z| {
                    let mut frame = (*template).clone();
                    frame.width = dim[0];
                    frame.height = dim[1];
                    let count = dim[0] as usize * dim[1] as usize;
                    frame.pixels = vec![10.; count].into();
                    let mut valid = vec![1; count];
                    if z == 0 {
                        valid[1] = 0;
                    }
                    frame.validity = Some(valid.into());
                    frame.geometry = Some(Geometry {
                        position: [
                            offset + if sheared { f64::from(z) * 0.25 } else { 0. },
                            -offset + if sheared { f64::from(z) * 0.5 } else { 0. },
                            offset + f64::from(z) * 2.,
                        ],
                        orientation: if reversed {
                            [-1., 0., 0., 0., 1., 0.]
                        } else {
                            [1., 0., 0., 0., 1., 0.]
                        },
                        spacing: [1.5, 0.75],
                        frame_of_reference: Some("generated supplied GPU grid".into()),
                        nifti_transform: None,
                    });
                    Arc::new(frame)
                })
                .collect(),
        )
        .unwrap(),
    )
}
fn masks(source: Arc<Volume>, kind: u8) -> Arc<Segments> {
    let [w, h, d] = source.dimensions();
    let middle = d / 2;
    let frames = [
        (65535, d - 1),
        (1, 0),
        (65535, 0),
        (1, middle),
        (65535, middle),
        (900, 0),
    ];
    let maximum: u8 = match kind {
        0 => 1,
        1 => 200,
        2 => 255,
        _ => 3,
    };
    let values = frames
        .iter()
        .flat_map(|&(number, z)| {
            (0..w * h).map(move |i| {
                if number == 900 || (number == 65535 && z == middle) {
                    0
                } else if i == 1 {
                    maximum
                } else {
                    [0, maximum / 2, maximum.div_ceil(2), maximum, 1][(i % w + 2 * (i / w) + z) % 5]
                }
            })
        })
        .collect::<Vec<u8>>();
    let numbers = [65535, 1, 900, 42];
    Arc::new(if kind == 0 {
        let mut bits = vec![0u8; values.len().div_ceil(8).next_multiple_of(2)];
        for (i, &value) in values.iter().enumerate() {
            if value != 0 {
                bits[i / 8] |= 1 << (i % 8);
            }
        }
        Segments::from_binary(source, &numbers, &frames, &bits, Some(true), || false).unwrap()
    } else {
        let mut values = values;
        if values.len() % 2 != 0 {
            values.push(255);
        }
        Segments::from_fractional(
            source,
            &numbers,
            &frames,
            &values,
            FractionalEncoding {
                kind: if kind == 2 {
                    FractionalKind::Occupancy
                } else {
                    FractionalKind::Probability
                },
                maximum,
            },
            Some(true),
            || false,
        )
        .unwrap()
    })
}
fn read(device: &wgpu::Device, queue: &wgpu::Queue, texture: &wgpu::Texture, bpp: u32) -> Vec<u8> {
    let size = texture.size();
    let stride = (size.width * bpp).div_ceil(256) * 256;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("generated supplied readback"),
        size: u64::from(stride) * u64::from(size.height) * u64::from(size.depth_or_array_layers),
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
        tx.send(r).unwrap();
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
fn verify(device: &wgpu::Device, queue: &wgpu::Queue, uploaded: &GpuSegment) {
    let [w, h, d] = uploaded.source().volume().dimensions();
    let mask = uploaded.segments().mask(uploaded.number()).unwrap();
    let addresses = read(device, queue, uploaded.address_texture(), 4)
        .as_chunks::<4>()
        .0
        .iter()
        .map(|v| u32::from_le_bytes(*v))
        .collect::<Vec<_>>();
    assert_eq!(addresses.len(), d);
    let bytes = read(device, queue, uploaded.packed_texture(), 4);
    let words = bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|v| u32::from_le_bytes(*v))
        .collect::<Vec<_>>();
    let binary = uploaded.encoding() == SegmentEncoding::Binary;
    let per_word = if binary { 32 } else { 4 };
    let width = w.div_ceil(per_word);
    let mut planes = 0;
    for (z, &address) in addresses.iter().enumerate() {
        let known = uploaded.segments().plane_is_supplied(uploaded.number(), z);
        if !known {
            assert_eq!(address, UNKNOWN_PLANE);
            continue;
        }
        assert_eq!(address, planes);
        planes += 1;
        for y in 0..h {
            for x in 0..width * per_word {
                let word = words[(address as usize * h + y) * width + x / per_word];
                let value = if binary {
                    (word >> (x % 32)) & 1
                } else {
                    (word >> ((x % 4) * 8)) & 255
                };
                let expected = if x < w {
                    mask.raw_value([x, y, z]).unwrap()
                } else {
                    0
                };
                assert_eq!(
                    value,
                    u32::from(expected),
                    "ID{} cell{x}/{y}/{z}",
                    uploaded.number()
                );
            }
        }
    }
    if planes == 0 {
        assert!(words.iter().all(|v| *v == 0));
    }
}
fn projected(projector: &mut GpuMaskProjector, request: &MaskProjection) -> Arc<MaskImage> {
    let source = request.input.source();
    let mut encoder = source.device().create_command_encoder(&Default::default());
    let image = projector
        .encode_rows(&mut encoder, request, 0, 0..request.size[1])
        .unwrap()
        .unwrap();
    source.queue().submit([encoder.finish()]);
    image
}
fn compare(image: &MaskImage) -> usize {
    let request = image.request();
    let source = request.input.source();
    let device = source.device();
    let queue = source.queue();
    let cells = read(device, queue, image.cells(), 16);
    let depths = read(device, queue, image.depth(), 4);
    let view = request.input.view(request.number).unwrap();
    let camera = request
        .camera
        .frame(
            &request.display,
            f64::from(request.size[0]) / f64::from(request.size[1]),
        )
        .unwrap();
    assert_eq!(image.key(), view.key());
    assert_eq!(image.revision(), view.revision());
    let mut count = 0;
    for (i, word) in cells.as_chunks::<16>().0.iter().enumerate() {
        let pixel: [u32; 4] =
            std::array::from_fn(|a| u32::from_le_bytes(word[a * 4..a * 4 + 4].try_into().unwrap()));
        let depth = f32::from_le_bytes(depths[i * 4..i * 4 + 4].try_into().unwrap());
        let (origin, direction) = camera.ray([
            ((i % request.size[0] as usize) as f64 + 0.5) / f64::from(request.size[0]),
            ((i / request.size[0] as usize) as f64 + 0.5) / f64::from(request.size[1]),
        ]);
        let hit = view
            .ray(origin, direction, [-1e30, 1e30], request.clip.as_ref())
            .unwrap()
            .first_eligible_hit(|| false)
            .unwrap();
        assert_ne!(pixel[3], RAY_ERROR, "arithmetic refusal at pixel{i}");
        if let Some(hit) = hit {
            count += 1;
            assert_eq!(
                pixel,
                [
                    hit.voxel[0] as u32,
                    hit.voxel[1] as u32,
                    hit.voxel[2] as u32,
                    u32::from(request.number)
                ],
                "pixel{i}"
            );
            assert!(
                (f64::from(depth) - hit.range_mm[0]).abs() < 1e-4,
                "pixel{i}: {depth} vs {}",
                hit.range_mm[0]
            );
        } else {
            assert_eq!(pixel, [0; 4]);
            assert_eq!(depth, 0.);
        }
    }
    count
}
#[test]
fn packed_words_retain_original_samples_large_ids_overlaps_and_sparse_coverage() {
    let (device, queue) = device();
    let source = source([67, 35, 34], true, false, 0.);
    let gpu = GpuVolumeSource::new(&device, &queue, source.clone(), || false).unwrap();
    for kind in 0..4 {
        let map = masks(source.clone(), kind);
        for number in [65535, 1, 900, 42] {
            let (uploaded, stats) =
                GpuSegment::upload(gpu.clone(), map.clone(), number, None, || false).unwrap();
            verify(&device, &queue, &uploaded);
            assert_eq!(
                stats.supplied_planes,
                match number {
                    65535 => 3,
                    1 => 2,
                    900 => 1,
                    _ => 0,
                }
            );
            if number == 65535 {
                assert_eq!(
                    stats.stored_sample_bytes,
                    if kind == 0 { 1260 } else { 7140 }
                );
                let membership = map.mask(number).unwrap();
                assert!(membership.value([1, 0, 0]).unwrap());
                assert!(!source.frames()[0].has_sample(1));
            }
            assert_eq!(stats.uploaded_address_bytes, 136);
            if number == 42 {
                assert_eq!(
                    uploaded.packed_texture().size(),
                    wgpu::Extent3d {
                        width: 1,
                        height: 1,
                        depth_or_array_layers: 1
                    }
                );
                assert_eq!(stats.stored_sample_bytes, 4);
                assert_eq!(stats.uploaded_sample_bytes, 0);
            }
            let (same, reused) = GpuSegment::upload(
                gpu.clone(),
                map.clone(),
                number,
                Some(uploaded.clone()),
                || false,
            )
            .unwrap();
            assert!(Arc::ptr_eq(&same, &uploaded));
            assert!(reused.reused);
            assert_eq!(reused.uploaded_sample_bytes, 0);
            assert_eq!(reused.uploaded_address_bytes, 0);
        }
    }
}
#[test]
fn supplied_rays_match_core_on_affine_clipped_and_rebased_sources() {
    let (device, queue) = device();
    let mut hits = 0;
    for (reversed, offset) in [(false, 0.), (true, 300000.)] {
        let source = source([7, 5, 6], true, reversed, offset);
        let gpu = GpuVolumeSource::new(&device, &queue, source.clone(), || false).unwrap();
        let mut projector = GpuMaskProjector::new(gpu.clone()).unwrap();
        for kind in 0..4 {
            let map = masks(source.clone(), kind);
            for number in [65535, 1, 900, 42] {
                let (uploaded, _) =
                    GpuSegment::upload(gpu.clone(), map.clone(), number, None, || false).unwrap();
                let mut camera = Camera::fit(&source);
                camera.yaw_degrees = 31.;
                camera.pitch_degrees = 23.;
                camera.roll_degrees = 11.;
                let mut request = MaskProjection {
                    input: MaskInput::Supplied(uploaded),
                    number,
                    camera,
                    display: ViewState {
                        pan: [0.03, -0.02],
                        ..Default::default()
                    },
                    clip: None,
                    size: [17, 13],
                };
                hits += compare(&projected(&mut projector, &request));
                let center = source.voxel_to_world([3., 2., 0.]);
                request.clip = Some(PhysicalClip {
                    minimum_mm: center.map(|v| v - 2.),
                    maximum_mm: center.map(|v| v + 2.),
                });
                compare(&projected(&mut projector, &request));
                request.clip = None;
                request.camera = Camera::fit(&source);
                request.camera.target_mm = source.voxel_to_world([1., 2., 0.]);
                request.display = ViewState::default();
                request.size = [1, 1];
                compare(&projected(&mut projector, &request));
            }
        }
    }
    assert!(hits > 0);
}
#[test]
fn failed_replacements_and_cancellation_leave_prior_uploads_and_images_unchanged() {
    let (device, queue) = device();
    let source = source([67, 35, 34], false, false, 0.);
    let gpu = GpuVolumeSource::new(&device, &queue, source.clone(), || false).unwrap();
    let map = masks(source.clone(), 2);
    let (old, _) = GpuSegment::upload(gpu.clone(), map.clone(), 65535, None, || false).unwrap();
    let prior = read(&device, &queue, old.packed_texture(), 4);
    for after in [0, 1, 3, 6] {
        let calls = AtomicUsize::new(0);
        assert!(matches!(
            GpuSegment::upload(gpu.clone(), map.clone(), 1, Some(old.clone()), || calls
                .fetch_add(1, Ordering::Relaxed)
                >= after),
            Err("Cancelled")
        ));
        assert_eq!(read(&device, &queue, old.packed_texture(), 4), prior);
    }
    assert!(GpuSegment::upload(gpu.clone(), map.clone(), 0, None, || false).is_err());
    assert!(matches!(
        GpuSegment::upload(gpu.clone(), map.clone(), 65535, Some(old.clone()), || true),
        Err("Cancelled")
    ));
    let other = GpuVolumeSource::new(&device, &queue, source.clone(), || false).unwrap();
    assert!(GpuSegment::upload(other, map.clone(), 65535, Some(old.clone()), || false).is_err());
    let different = masks(self::source([67, 35, 34], false, false, 0.), 1);
    assert!(GpuSegment::upload(gpu.clone(), different, 65535, None, || false).is_err());
    let budget = MemoryBudget::gpu();
    let limit = budget.limit();
    budget.set_limit(0);
    assert!(GpuSegment::upload(gpu.clone(), map.clone(), 1, None, || false).is_err());
    budget.set_limit(limit);
    let budget = MemoryBudget::volumes();
    let limit = budget.limit();
    budget.set_limit(0);
    assert!(GpuSegment::upload(gpu.clone(), map.clone(), 1, None, || false).is_err());
    budget.set_limit(limit);
    let mut projector = GpuMaskProjector::new(gpu.clone()).unwrap();
    let request = MaskProjection {
        input: MaskInput::Supplied(old.clone()),
        number: 65535,
        camera: Camera::fit(&source),
        display: ViewState::default(),
        clip: None,
        size: [17, 13],
    };
    let image = projected(&mut projector, &request);
    let image_before = read(&device, &queue, image.cells(), 16);
    let (new, stats) =
        GpuSegment::upload(gpu.clone(), map, 1, Some(old.clone()), || false).unwrap();
    assert!(!stats.reused);
    let mut wrong = request.clone();
    wrong.input = MaskInput::Supplied(new.clone());
    let mut encoder = device.create_command_encoder(&Default::default());
    assert!(
        projector
            .encode_rows(&mut encoder, &wrong, 0, 0..13)
            .is_err()
    );
    assert!(
        projector
            .encode_rows(&mut encoder, &request, 0, 0..3)
            .unwrap()
            .is_none()
    );
    wrong.number = 1;
    assert!(
        projector
            .encode_rows(&mut encoder, &wrong, 0, 3..13)
            .is_err()
    );
    queue.submit([encoder.finish()]);
    projector.discard_pending(0).unwrap();
    compare(&projected(&mut projector, &wrong));
    assert_eq!(read(&device, &queue, image.cells(), 16), image_before);
    assert_eq!(read(&device, &queue, old.packed_texture(), 4), prior);
    let replacement = masks(source, 1);
    let (changed, stats) =
        GpuSegment::upload(gpu, replacement, 65535, Some(old.clone()), || false).unwrap();
    assert!(!stats.reused);
    assert_ne!(changed.segments().object_id(), old.segments().object_id());
    verify(&device, &queue, &changed);
    assert_eq!(read(&device, &queue, old.packed_texture(), 4), prior);
}

#[test]
fn constrained_real_device_refuses_supplied_addresses_and_keeps_manual_projection() {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        flags: crate::instance_flags(),
        backends: crate::configured_backends().unwrap(),
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapter = pollster::block_on(instance.request_adapter(&Default::default()))
        .expect("Actual GPU required");
    let mut descriptor = crate::device_descriptor(&adapter);
    descriptor
        .required_limits
        .max_sampled_textures_per_shader_stage = 2;
    let (device, queue) = pollster::block_on(adapter.request_device(&descriptor)).unwrap();
    let source = source([7, 5, 6], false, false, 0.);
    let gpu = GpuVolumeSource::new(&device, &queue, source.clone(), || false).unwrap();
    let (supplied, _) =
        GpuSegment::upload(gpu.clone(), masks(source.clone(), 2), 65535, None, || false).unwrap();
    let mut projector = GpuMaskProjector::new(gpu.clone()).unwrap();
    let mut request = MaskProjection {
        input: MaskInput::Supplied(supplied),
        number: 65535,
        camera: Camera::fit(&source),
        display: ViewState::default(),
        clip: None,
        size: [17, 13],
    };
    let mut encoder = device.create_command_encoder(&Default::default());
    assert!(
        matches!(projector.encode_rows(&mut encoder,&request,0,0..13),Err(message) if message.contains("addresses"))
    );
    let mut labels = orv_core::labels::Labels::empty(source).unwrap();
    labels.threshold(None, [-100., 100.], 7, || false).unwrap();
    let (labels, _) =
        crate::label_source::GpuLabels::upload(gpu, Arc::new(labels), None, || false).unwrap();
    request.input = MaskInput::Manual(labels);
    request.number = 256;
    assert!(
        projector
            .encode_rows(&mut encoder, &request, 0, 0..13)
            .is_err()
    );
    request.number = 7;
    drop(encoder);
    assert!(compare(&projected(&mut projector, &request)) > 0);
}
fn scalar_range(origin: [f64; 3], direction: [f64; 3], maximum: [f64; 3]) -> Option<[f64; 2]> {
    let mut near = f64::NEG_INFINITY;
    let mut far = f64::INFINITY;
    for a in 0..3 {
        if direction[a] == 0. {
            if origin[a] < 0. || origin[a] > maximum[a] {
                return None;
            }
        } else {
            let p = -origin[a] / direction[a];
            let q = (maximum[a] - origin[a]) / direction[a];
            near = near.max(p.min(q));
            far = far.min(p.max(q));
        }
    }
    (near <= far).then_some([near, far])
}
fn half(bits: u16) -> f64 {
    assert_eq!(bits & 0x8000, 0);
    let exponent = (bits >> 10) & 31;
    assert_ne!(exponent, 31);
    let fraction = f64::from(bits & 1023);
    if exponent == 0 {
        fraction * 2f64.powi(-24)
    } else {
        (1. + fraction / 1024.) * 2f64.powi(i32::from(exponent) - 15)
    }
}
#[test]
fn supplied_opaque_composition_matches_independent_homogeneous_optics() {
    use crate::{
        material_volume::Lighting,
        opaque_material::{OpaqueMaterialRenderer, OpaqueMaterialScene},
    };
    use orv_core::{
        volume_material::{MaterialPoint, VolumeMaterial},
        volume_view::{Projection, Quality, VolumeView},
    };
    let (device, queue) = device();
    let original = source([7, 5, 6], true, true, 300000.);
    let source = Arc::new(
        Volume::new(
            original
                .frames()
                .iter()
                .map(|f| {
                    let mut frame = (**f).clone();
                    frame.validity = None;
                    Arc::new(frame)
                })
                .collect(),
        )
        .unwrap(),
    );
    let gpu = GpuVolumeSource::new(&device, &queue, source.clone(), || false).unwrap();
    let mut projector = GpuMaskProjector::new(gpu.clone()).unwrap();
    let mut renderer =
        OpaqueMaterialRenderer::new(gpu.clone(), wgpu::TextureFormat::Rgba8Unorm).unwrap();
    let material = Arc::new(
        VolumeMaterial::new(&[-1000., 1000.].map(|value| MaterialPoint {
            value,
            luminance: 0.2,
            extinction_per_mm: 0.4,
        }))
        .unwrap(),
    );
    for kind in 0..4 {
        let map = masks(source.clone(), kind);
        for number in [65535, 1, 900, 42] {
            let (uploaded, _) =
                GpuSegment::upload(gpu.clone(), map.clone(), number, None, || false).unwrap();
            let mut state = VolumeView::fit(&source);
            state.projection = Projection::Composite;
            state.camera.yaw_degrees = 31.;
            state.camera.pitch_degrees = 23.;
            let request = MaskProjection {
                input: MaskInput::Supplied(uploaded),
                number,
                camera: state.camera.clone(),
                display: ViewState::default(),
                clip: None,
                size: [13, 11],
            };
            let mask = projected(&mut projector, &request);
            compare(&mask);
            let rays = request.camera.frame(&request.display, 13. / 11.).unwrap();
            let view = request.input.view(number).unwrap();
            for quality in [Quality::Interactive, Quality::Refined] {
                state.quality = quality;
                let scene = OpaqueMaterialScene {
                    mask: mask.clone(),
                    navigation: request.display.clone(),
                    volume: state.clone(),
                    material: material.clone(),
                    lighting: Lighting::UNLIT,
                    mask_luminance: 0.8,
                };
                let image = renderer
                    .render_rows(&scene, 0, 0..11, || false)
                    .unwrap()
                    .unwrap();
                let data = read(&device, &queue, image.texture(), 8);
                for (i, bytes) in data.as_chunks::<8>().0.iter().enumerate() {
                    let (origin, direction) =
                        rays.ray([((i % 13) as f64 + 0.5) / 13., ((i / 13) as f64 + 0.5) / 11.]);
                    let hit = view
                        .ray(origin, direction, [-1e30, 1e30], None)
                        .unwrap()
                        .first_eligible_hit(|| false)
                        .unwrap();
                    let range = scalar_range(
                        source.world_to_voxel(origin),
                        source.world_vector_to_voxel(direction),
                        source.dimensions().map(|n| (n - 1) as f64),
                    );
                    let distance = range.map_or(0., |v| {
                        (v[1].min(hit.as_ref().map_or(f64::INFINITY, |h| h.range_mm[0])) - v[0])
                            .max(0.)
                    });
                    let t = (-0.4 * distance).exp();
                    let expected =
                        0.2 * (1. - t) + t * if hit.is_some() { 0.8 } else { 0.0030352698 };
                    for c in 0..3 {
                        let actual = half(u16::from_le_bytes([bytes[c * 2], bytes[c * 2 + 1]]));
                        assert!(
                            (actual - expected).abs() < 0.001,
                            "kind{kind} ID{number} pixel{i}: {actual} vs {expected}"
                        );
                    }
                    assert_eq!(&bytes[6..], &[0, 0x3c]);
                }
            }
        }
    }
}

#[test]
fn color_source_alpha_remains_independent_of_stored_supplied_membership() {
    let (device, queue) = device();
    let original = source([7, 5, 6], true, true, 300000.);
    let source = Arc::new(
        Volume::new(
            original
                .frames()
                .iter()
                .enumerate()
                .map(|(z, f)| {
                    let mut frame = (**f).clone();
                    frame.pixels = Arc::from([]);
                    frame.validity = None;
                    frame.rgba = Some(
                        (0..35)
                            .flat_map(|i| [90, 120, 150, if z == 0 && i == 1 { 0 } else { 255 }])
                            .collect::<Vec<u8>>()
                            .into(),
                    );
                    Arc::new(frame)
                })
                .collect(),
        )
        .unwrap(),
    );
    assert!(source.is_color());
    assert!(!source.has_missing_scalar());
    let gpu = GpuVolumeSource::new(&device, &queue, source.clone(), || false).unwrap();
    let mut projector = GpuMaskProjector::new(gpu.clone()).unwrap();
    for kind in 0..4 {
        let map = masks(source.clone(), kind);
        assert_eq!(map.value(65535, [1, 0, 0]), Some(true));
        assert!(!source.frames()[0].has_sample(1));
        let (uploaded, _) = GpuSegment::upload(gpu.clone(), map, 65535, None, || false).unwrap();
        verify(&device, &queue, &uploaded);
        let mut camera = Camera::fit(&source);
        camera.target_mm = source.voxel_to_world([1., 2., 0.]);
        let mut request = MaskProjection {
            input: MaskInput::Supplied(uploaded),
            number: 65535,
            camera,
            display: ViewState::default(),
            clip: None,
            size: [1, 1],
        };
        let image = projected(&mut projector, &request);
        assert_eq!(compare(&image), 1);
        let cells = read(&device, &queue, image.cells(), 16);
        let y = u32::from_le_bytes(cells[4..8].try_into().unwrap());
        assert!(y > 0, "A missing front sample must not become the hit");
        request.camera.yaw_degrees = 31.;
        request.camera.pitch_degrees = 23.;
        request.size = [17, 13];
        compare(&projected(&mut projector, &request));
    }
}
