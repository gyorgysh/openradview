// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use crate::label_source::GpuLabels;
use orv_core::{
    labels::Labels,
    study::{Geometry, NiftiMethod, NiftiTransform, demo_study},
    volume::Volume,
};
use std::{sync::mpsc, time::Duration};
fn device() -> (wgpu::Device, wgpu::Queue) {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: crate::configured_backends().unwrap(),
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapter = pollster::block_on(instance.request_adapter(&Default::default()))
        .expect("Actual GPU required");
    pollster::block_on(adapter.request_device(&crate::device_descriptor(&adapter))).unwrap()
}

#[test]
fn included_ct_empty_mask_rays_are_resolved_at_workbench_sizes() {
    let study = orv_core::load::load_zip(
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/demos/nlst-ct.zip"
        )),
        &std::sync::atomic::AtomicBool::new(false),
        |_| {},
    )
    .unwrap();
    let source = Arc::new(Volume::new(study.series[0].frames.clone()).unwrap());
    let labels = Arc::new(Labels::empty(source.clone()).unwrap());
    let (device, queue) = device();
    let gpu = GpuVolumeSource::new(&device, &queue, source.clone(), || false).unwrap();
    let (labels, _) = GpuLabels::upload(gpu.clone(), labels, None, || false).unwrap();
    let mut projector = GpuMaskProjector::new(gpu.clone()).unwrap();
    let mut camera = Camera::fit(&source);
    camera.yaw_degrees = 25.;
    camera.pitch_degrees = 15.;
    for height in 64..=256 {
        let size = [512, height];
        camera
            .fit_to_volume(&source, f64::from(size[0]) / f64::from(size[1]))
            .unwrap();
        let request = MaskProjection {
            input: MaskInput::Manual(labels.clone()),
            number: 1,
            camera: camera.clone(),
            display: ViewState::default(),
            clip: None,
            size,
        };
        let mut encoder = device.create_command_encoder(&Default::default());
        let image = projector
            .encode_rows(&mut encoder, &request, 0, 0..size[1])
            .unwrap()
            .unwrap();
        queue.submit([encoder.finish()]);
        let (cells, _) = read(&device, &queue, &image);
        assert!(
            cells.iter().all(|cell| cell[3] == 0),
            "empty mask contains an invalid or unexpected hit at size {size:?}"
        );
    }
    let mut painted = Labels::empty(source.clone()).unwrap();
    for z in [3, 4] {
        painted.paint(z, &[[181., 1.]], 3, 1, || false).unwrap();
    }
    let (painted, _) = GpuLabels::upload(gpu, Arc::new(painted), None, || false).unwrap();
    let size = [512, 108];
    camera
        .fit_to_volume(&source, f64::from(size[0]) / f64::from(size[1]))
        .unwrap();
    let request = MaskProjection {
        input: MaskInput::Manual(painted),
        number: 1,
        camera,
        display: ViewState::default(),
        clip: None,
        size,
    };
    let mut encoder = device.create_command_encoder(&Default::default());
    let image = projector
        .encode_rows(&mut encoder, &request, 0, 0..size[1])
        .unwrap()
        .unwrap();
    queue.submit([encoder.finish()]);
    assert!(compare(&device, &queue, &image) > 0);
}
fn source(sheared: bool, reversed: bool, masked: bool, offset: f64) -> Arc<Volume> {
    source_with_row_spacing(sheared, reversed, masked, offset, 1.5)
}
fn source_with_row_spacing(
    sheared: bool,
    reversed: bool,
    masked: bool,
    offset: f64,
    row_spacing: f64,
) -> Arc<Volume> {
    let template = demo_study().series.remove(0).frames.remove(0);
    Arc::new(
        Volume::new(
            (0..6)
                .map(|z| {
                    let mut f = (*template).clone();
                    f.width = 7;
                    f.height = 5;
                    f.pixels = vec![20.; 35].into();
                    f.geometry = Some(Geometry {
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
                        spacing: [row_spacing, 0.75],
                        frame_of_reference: Some("generated mask ray source".into()),
                        nifti_transform: None,
                    });
                    f.validity = masked.then(|| {
                        (0..35)
                            .map(|i| {
                                u8::from(!(i % 7 + 2 * (i / 7) + z as usize).is_multiple_of(7))
                            })
                            .collect::<Vec<_>>()
                            .into()
                    });
                    Arc::new(f)
                })
                .collect(),
        )
        .unwrap(),
    )
}
fn labels(source: Arc<Volume>, pattern: impl Fn(usize, usize, usize) -> u8) -> Arc<Labels> {
    let frames = source
        .frames()
        .iter()
        .enumerate()
        .map(|(z, frame)| {
            let mut f = (**frame).clone();
            f.validity = None;
            f.geometry.as_mut().unwrap().nifti_transform = Some(NiftiTransform {
                method: NiftiMethod::Sform,
                code: 1,
            });
            f.pixels = (0..35)
                .map(|i| f32::from(pattern(i % 7, i / 7, z)))
                .collect::<Vec<_>>()
                .into();
            Arc::new(f)
        })
        .collect();
    Arc::new(Labels::import(source, &Volume::new(frames).unwrap(), || false).unwrap())
}
fn request(labels: Arc<GpuLabels>) -> MaskProjection {
    MaskProjection {
        camera: Camera::fit(labels.labels().source()),
        input: MaskInput::Manual(labels),
        number: 7,
        display: ViewState::default(),
        clip: None,
        size: [17, 13],
    }
}
fn bytes(device: &wgpu::Device, queue: &wgpu::Queue, texture: &wgpu::Texture, bpp: u32) -> Vec<u8> {
    let size = texture.size();
    let stride = (size.width * bpp).div_ceil(256) * 256;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("generated mask projection readback"),
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
    let (tx, rx) = mpsc::channel();
    buffer.map_async(wgpu::MapMode::Read, .., move |v| {
        tx.send(v).unwrap();
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
fn read(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    image: &MaskImage,
) -> (Vec<[u32; 4]>, Vec<f32>) {
    let cells = bytes(device, queue, image.cells(), 16)
        .as_chunks::<16>()
        .0
        .iter()
        .map(|v| {
            std::array::from_fn(|i| u32::from_le_bytes(v[i * 4..i * 4 + 4].try_into().unwrap()))
        })
        .collect();
    let depth = bytes(device, queue, image.depth(), 4)
        .as_chunks::<4>()
        .0
        .iter()
        .map(|v| f32::from_le_bytes(*v))
        .collect();
    (cells, depth)
}
fn compare(device: &wgpu::Device, queue: &wgpu::Queue, image: &MaskImage) -> usize {
    let (cells, depths) = read(device, queue, image);
    let request = image.request();
    let size = request.size;
    let frame = request
        .camera
        .frame(&request.display, f64::from(size[0]) / f64::from(size[1]))
        .unwrap();
    let mask = request.input.view(request.number).unwrap();
    assert_eq!(image.key(), mask.key());
    assert_eq!(image.revision(), mask.revision());
    let mut count = 0;
    for y in 0..size[1] {
        for x in 0..size[0] {
            let i = (y * size[0] + x) as usize;
            let (origin, direction) = frame.ray([
                (f64::from(x) + 0.5) / f64::from(size[0]),
                (f64::from(y) + 0.5) / f64::from(size[1]),
            ]);
            let hit = mask
                .ray(origin, direction, [-1e30, 1e30], request.clip.as_ref())
                .unwrap()
                .first_eligible_hit(|| false)
                .unwrap();
            assert_ne!(
                cells[i][3], RAY_ERROR,
                "arithmetic refusal at {x}/{y}: {:?} depth {}",
                cells[i], depths[i]
            );
            match hit {
                Some(hit) => {
                    count += 1;
                    assert_eq!(
                        cells[i],
                        [
                            hit.voxel[0] as u32,
                            hit.voxel[1] as u32,
                            hit.voxel[2] as u32,
                            u32::from(request.number)
                        ],
                        "cell at {x}/{y}"
                    );
                    assert!(
                        (f64::from(depths[i]) - hit.range_mm[0]).abs() < 1e-4,
                        "depth at {x}/{y}: {} vs {}",
                        depths[i],
                        hit.range_mm[0]
                    );
                }
                None => {
                    assert_eq!(cells[i], [0; 4], "empty at {x}/{y}");
                    assert_eq!(depths[i], 0.);
                }
            }
        }
    }
    count
}
#[test]
fn mask_projection_matches_cpu_for_thin_regions_missing_samples_and_affine_cameras() {
    let (device, queue) = device();
    for (sheared, reversed, masked, offset) in [
        (false, false, false, 0.),
        (true, false, true, 0.),
        (true, true, true, 300000.),
    ] {
        let source = source(sheared, reversed, masked, offset);
        let labels = labels(source.clone(), |x, y, z| {
            if z == 3 || x == 3 {
                7
            } else if (x + 2 * y + z) % 5 == 0 {
                2
            } else {
                0
            }
        });
        let gpu = GpuVolumeSource::new(&device, &queue, source, || false).unwrap();
        let (labels, _) = GpuLabels::upload(gpu.clone(), labels, None, || false).unwrap();
        let mut projector = GpuMaskProjector::new(gpu).unwrap();
        let mut request = request(labels);
        request.camera.yaw_degrees = 31.;
        request.camera.pitch_degrees = 23.;
        request.camera.roll_degrees = 11.;
        request.display.pan = [0.03, -0.02];
        request.display.zoom = 1.1;
        let mut encoder = device.create_command_encoder(&Default::default());
        let image = projector
            .encode_rows(&mut encoder, &request, 0, 0..request.size[1])
            .unwrap()
            .unwrap();
        queue.submit([encoder.finish()]);
        assert!(compare(&device, &queue, &image) > 0);
        let center = request.input.source().volume().center();
        request.clip = Some(PhysicalClip {
            minimum_mm: std::array::from_fn(|a| center[a] - 1.25),
            maximum_mm: std::array::from_fn(|a| center[a] + 1.75),
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        let mut clipped = None;
        for start in (0..request.size[1]).step_by(3) {
            let result = projector
                .encode_rows(
                    &mut encoder,
                    &request,
                    1,
                    start..(start + 3).min(request.size[1]),
                )
                .unwrap();
            if result.is_some() {
                clipped = result;
            }
        }
        queue.submit([encoder.finish()]);
        assert!(compare(&device, &queue, &clipped.unwrap()) > 0);
        compare(&device, &queue, &image);
    }
}
#[test]
fn published_images_are_immutable_and_partial_frames_reject_changed_requests_or_rows() {
    let (device, queue) = device();
    let source = source(false, false, false, 0.);
    let labels = labels(source.clone(), |x, _, _| if x == 3 { 7 } else { 2 });
    let gpu = GpuVolumeSource::new(&device, &queue, source, || false).unwrap();
    let (labels, _) = GpuLabels::upload(gpu.clone(), labels, None, || false).unwrap();
    let mut projector = GpuMaskProjector::new(gpu).unwrap();
    let request = request(labels);
    let mut encoder = device.create_command_encoder(&Default::default());
    let old = projector
        .encode_rows(&mut encoder, &request, 0, 0..13)
        .unwrap()
        .unwrap();
    queue.submit([encoder.finish()]);
    let old_pixels = read(&device, &queue, &old);
    compare(&device, &queue, &old);
    let mut encoder = device.create_command_encoder(&Default::default());
    assert!(
        projector
            .encode_rows(&mut encoder, &request, 0, 0..4)
            .unwrap()
            .is_none()
    );
    let mut changed = request.clone();
    changed.number = 2;
    assert!(
        projector
            .encode_rows(&mut encoder, &changed, 0, 4..8)
            .is_err()
    );
    assert!(
        projector
            .encode_rows(&mut encoder, &request, 0, 5..8)
            .is_err()
    );
    assert!(
        projector
            .encode_rows(&mut encoder, &request, 0, 0..4)
            .is_err()
    );
    assert!(
        projector
            .encode_rows(&mut encoder, &request, 0, 4..8)
            .unwrap()
            .is_none()
    );
    let new = projector
        .encode_rows(&mut encoder, &request, 0, 8..13)
        .unwrap()
        .unwrap();
    assert!(!Arc::ptr_eq(&old, &new));
    let other = projector
        .encode_rows(&mut encoder, &changed, 1, 0..13)
        .unwrap()
        .unwrap();
    queue.submit([encoder.finish()]);
    compare(&device, &queue, &new);
    assert!(compare(&device, &queue, &other) > 0);
    assert_eq!(read(&device, &queue, &old), old_pixels);
    let mut encoder = device.create_command_encoder(&Default::default());
    assert!(
        projector
            .encode_rows(&mut encoder, &request, 0, 0..1)
            .unwrap()
            .is_none()
    );
    drop(encoder);
    projector.discard_pending(0).unwrap();
    let mut encoder = device.create_command_encoder(&Default::default());
    let result = projector
        .encode_rows(&mut encoder, &changed, 0, 0..13)
        .unwrap()
        .unwrap();
    queue.submit([encoder.finish()]);
    compare(&device, &queue, &result);
}
#[test]
fn exact_parallel_faces_empty_masks_and_invalid_projection_inputs_are_explicit() {
    let (device, queue) = device();
    let source = source(false, false, false, 0.);
    let labels = labels(source.clone(), |_, _, _| 7);
    let gpu = GpuVolumeSource::new(&device, &queue, source, || false).unwrap();
    let (labels, _) = GpuLabels::upload(gpu.clone(), labels, None, || false).unwrap();
    let mut projector = GpuMaskProjector::new(gpu).unwrap();
    let mut request = request(labels);
    request.size = [1, 1];
    // Physical x=.375 is exactly source x=.5: the larger cell owns this parallel face.
    request.camera.target_mm = [0.375, 3., 4.];
    request.camera.vertical_span_mm = 1.;
    let mut encoder = device.create_command_encoder(&Default::default());
    let image = projector
        .encode_rows(&mut encoder, &request, 0, 0..1)
        .unwrap()
        .unwrap();
    queue.submit([encoder.finish()]);
    compare(&device, &queue, &image);
    assert_eq!(read(&device, &queue, &image).0[0], [1, 0, 2, 7]);
    request.camera.target_mm[0] = 6.5 * 0.75; // Upper half-open x face is outside.
    let mut encoder = device.create_command_encoder(&Default::default());
    let image = projector
        .encode_rows(&mut encoder, &request, 0, 0..1)
        .unwrap()
        .unwrap();
    queue.submit([encoder.finish()]);
    assert_eq!(compare(&device, &queue, &image), 0);
    request.number = 9;
    request.camera.target_mm[0] = 1.;
    let mut encoder = device.create_command_encoder(&Default::default());
    let image = projector
        .encode_rows(&mut encoder, &request, 0, 0..1)
        .unwrap()
        .unwrap();
    queue.submit([encoder.finish()]);
    assert_eq!(compare(&device, &queue, &image), 0);
    let mut encoder = device.create_command_encoder(&Default::default());
    request.number = 0;
    assert!(
        projector
            .encode_rows(&mut encoder, &request, 0, 0..1)
            .is_err()
    );
    request.number = 7;
    assert!(
        projector
            .encode_rows(&mut encoder, &request, 2, 0..1)
            .is_err()
    );
    assert!(
        projector
            .encode_rows(&mut encoder, &request, 0, 1..1)
            .is_err()
    );
    request.size = [MAX_SIDE + 1, 1];
    assert!(
        projector
            .encode_rows(&mut encoder, &request, 0, 0..1)
            .is_err()
    );
    request.size = [1, 1];
    request.camera.target_mm[0] = f64::NAN;
    assert!(
        projector
            .encode_rows(&mut encoder, &request, 0, 0..1)
            .is_err()
    );
}

#[test]
fn constrained_real_device_keeps_legacy_rendering_and_refuses_mask_projection() {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: crate::configured_backends().unwrap(),
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapter = pollster::block_on(instance.request_adapter(&Default::default()))
        .expect("Actual GPU required");
    let mut descriptor = crate::device_descriptor(&adapter);
    descriptor
        .required_limits
        .max_storage_textures_per_shader_stage = 1;
    let (device, queue) = pollster::block_on(adapter.request_device(&descriptor)).unwrap();
    let source =
        GpuVolumeSource::new(&device, &queue, source(false, false, false, 0.), || false).unwrap();
    assert!(GpuMaskProjector::new(source.clone()).is_err());
    let state = orv_core::volume_view::VolumeView::fit(source.volume());
    let mut legacy =
        crate::volume::VolumeRenderer::from_source(source, wgpu::TextureFormat::Rgba8Unorm, || {
            false
        })
        .unwrap();
    let mut encoder = device.create_command_encoder(&Default::default());
    legacy
        .render(
            &device,
            &mut encoder,
            &ViewState::default(),
            &state,
            [16, 16],
        )
        .unwrap();
    queue.submit([encoder.finish()]);
    let pixels = bytes(&device, &queue, legacy.output_texture().unwrap(), 4);
    assert!(
        pixels
            .as_chunks::<4>()
            .0
            .iter()
            .any(|pixel| pixel[..3].iter().any(|v| *v > 0)),
        "The constrained device must still render source pixels"
    );
}

#[test]
fn excessive_batches_and_wrong_gpu_sources_cannot_publish_partial_images() {
    let (device, queue) = device();
    let volume = source(false, false, false, 0.);
    let map = labels(volume.clone(), |_, _, _| 7);
    let gpu = GpuVolumeSource::new(&device, &queue, volume.clone(), || false).unwrap();
    let (labels, _) = GpuLabels::upload(gpu.clone(), map.clone(), None, || false).unwrap();
    let mut projector = GpuMaskProjector::new(gpu).unwrap();
    let mut request = request(labels);
    request.size = [1, MAX_ROW_DISPATCHES as u32 + 1];
    let mut encoder = device.create_command_encoder(&Default::default());
    for row in 0..MAX_ROW_DISPATCHES as u32 {
        assert!(
            projector
                .encode_rows(&mut encoder, &request, 0, row..row + 1)
                .unwrap()
                .is_none()
        );
    }
    assert!(
        projector
            .encode_rows(
                &mut encoder,
                &request,
                0,
                MAX_ROW_DISPATCHES as u32..request.size[1]
            )
            .is_err()
    );
    drop(encoder);
    projector.discard_pending(0).unwrap();
    let other_gpu = GpuVolumeSource::new(&device, &queue, volume, || false).unwrap();
    let (other, _) = GpuLabels::upload(other_gpu, map, None, || false).unwrap();
    let mut wrong = request.clone();
    wrong.input = MaskInput::Manual(other);
    let mut encoder = device.create_command_encoder(&Default::default());
    assert!(
        projector
            .encode_rows(&mut encoder, &wrong, 0, 0..request.size[1])
            .is_err()
    );
    let complete = projector
        .encode_rows(&mut encoder, &request, 0, 0..request.size[1])
        .unwrap()
        .unwrap();
    queue.submit([encoder.finish()]);
    assert!(compare(&device, &queue, &complete) > 0);
}

#[test]
fn numeric_interval_collapse_is_not_reported_as_an_empty_ray() {
    let (device, queue) = device();
    let volume = source(false, false, false, 0.);
    let map = labels(volume.clone(), |_, _, _| 0);
    let gpu = GpuVolumeSource::new(&device, &queue, volume, || false).unwrap();
    let (labels, _) = GpuLabels::upload(gpu.clone(), map, None, || false).unwrap();
    let mut projector = GpuMaskProjector::new(gpu).unwrap();
    let mut request = request(labels);
    request.size = [1, 1];
    request.camera.target_mm = [10_000_000., 3., 4.];
    request.camera.yaw_degrees = 90.;
    let mut encoder = device.create_command_encoder(&Default::default());
    let image = projector
        .encode_rows(&mut encoder, &request, 0, 0..1)
        .unwrap()
        .unwrap();
    queue.submit([encoder.finish()]);
    let (cells, depth) = read(&device, &queue, &image);
    assert_eq!(cells, vec![[0, 0, 0, RAY_ERROR]]);
    assert_eq!(depth, vec![0.]);
}

#[test]
fn oblique_clipped_entry_matches_the_physical_cell_reference() {
    let (device, queue) = device();
    for (reversed, offset) in [(false, 0.), (true, 300000.)] {
        let source = source_with_row_spacing(true, reversed, false, offset, 2.);
        let center = source.center();
        let labels = labels(source.clone(), |_, y, _| if y == 2 { 7 } else { 0 });
        let gpu = GpuVolumeSource::new(&device, &queue, source, || false).unwrap();
        let (labels, _) = GpuLabels::upload(gpu.clone(), labels, None, || false).unwrap();
        let mut projector = GpuMaskProjector::new(gpu).unwrap();
        let mut request = request(labels);
        request.size = [13, 11];
        request.camera.yaw_degrees = 31.;
        request.camera.pitch_degrees = 23.;
        request.camera.roll_degrees = 11.;
        request.display.pan = [0.03, -0.02];
        request.clip = Some(PhysicalClip {
            minimum_mm: std::array::from_fn(|a| center[a] - 2.),
            maximum_mm: std::array::from_fn(|a| center[a] + 2.),
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        let image = projector
            .encode_rows(&mut encoder, &request, 0, 0..11)
            .unwrap()
            .unwrap();
        queue.submit([encoder.finish()]);
        compare(&device, &queue, &image);
    }
}
