// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use orv_core::{
    labels::Labels,
    mask::MaskOwner,
    segments::{FractionalEncoding, FractionalKind, Segments},
    study::{Geometry, demo_study},
    surface::{FaceSide, SurfaceLimits},
    volume::Volume,
};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};

fn device(small: bool) -> (wgpu::Device, wgpu::Queue) {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: crate::configured_backends().unwrap(),
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapter = pollster::block_on(instance.request_adapter(&Default::default()))
        .expect("Actual GPU required for geometry uploads");
    let mut descriptor = crate::device_descriptor(&adapter);
    if small {
        descriptor.required_limits.max_buffer_size = 16384;
        descriptor.required_limits.max_storage_buffer_binding_size = 16384;
        descriptor.required_limits.max_uniform_buffer_binding_size = 16384;
    }
    pollster::block_on(adapter.request_device(&descriptor)).unwrap()
}
fn source(dim: [u32; 3]) -> Arc<Volume> {
    let template = demo_study().series.remove(0).frames.remove(0);
    Arc::new(
        Volume::new(
            (0..dim[2])
                .map(|z| {
                    let mut frame = (*template).clone();
                    frame.width = dim[0];
                    frame.height = dim[1];
                    frame.pixels = vec![10.; (dim[0] * dim[1]) as usize].into();
                    frame.geometry = Some(Geometry {
                        position: [
                            1000. + f64::from(z) * 0.25,
                            -2000.,
                            3000. + f64::from(z) * 2.,
                        ],
                        orientation: [-1., 0., 0., 0., 1., 0.],
                        spacing: [1.5, 0.75],
                        frame_of_reference: Some("generated GPU cell geometry".into()),
                        nifti_transform: None,
                    });
                    Arc::new(frame)
                })
                .collect(),
        )
        .unwrap(),
    )
}
fn surface(map: MaskOwner, number: u16, old: Option<&Arc<CellSurface>>) -> Arc<CellSurface> {
    CellSurface::build(map, number, old, SurfaceLimits::default(), || false)
        .unwrap()
        .0
}
fn fence(device: &wgpu::Device) {
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(Duration::from_secs(30)),
        })
        .unwrap();
}
fn read(device: &wgpu::Device, queue: &wgpu::Queue, chunk: &GpuCellChunk) -> Vec<[u32; 4]> {
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("generated quad word readback"),
        size: chunk.data_bytes,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_buffer_to_buffer(chunk.buffer(), 0, &buffer, 0, chunk.data_bytes);
    queue.submit([encoder.finish()]);
    mapped(device, &buffer)
        .as_chunks::<16>()
        .0
        .iter()
        .map(|bytes| {
            std::array::from_fn(|i| u32::from_le_bytes(bytes[i * 4..i * 4 + 4].try_into().unwrap()))
        })
        .collect()
}
fn mapped(device: &wgpu::Device, buffer: &wgpu::Buffer) -> Vec<u8> {
    let (tx, rx) = mpsc::sync_channel(1);
    buffer.map_async(wgpu::MapMode::Read, .., move |r| {
        let _ = tx.send(r);
    });
    fence(device);
    rx.recv_timeout(Duration::from_secs(10)).unwrap().unwrap();
    let data = buffer.get_mapped_range(..).unwrap();
    let bytes = data.to_vec();
    drop(data);
    buffer.unmap();
    bytes
}
fn verify(gpu: &GpuCellSurface) {
    let mut count = 0;
    for chunk in gpu.chunks() {
        assert!(Arc::ptr_eq(chunk.source(), gpu.source()));
        let actual = read(gpu.source.device(), gpu.source.queue(), chunk);
        let expected = gpu.surface.chunk(chunk.cpu.coordinate()).unwrap().faces();
        assert_eq!(actual.len(), expected.len());
        for (value, face) in actual.into_iter().zip(expected) {
            assert_eq!(value[..3], face.cell());
            assert_eq!(value[3] & 0xff, face.side() as u32);
            assert_eq!(value[3] & 0x100 != 0, face.boundary() == FaceBoundary::Grid);
            assert_eq!(value[3] & !0x107, 0);
            assert!(value[3] & 7 <= 5);
            count += 1;
        }
    }
    assert_eq!(count, gpu.surface.face_count());
    assert_eq!(gpu.referenced_data_bytes(), count as u64 * 16);
}
/// This checks integer vertex input consumption, not 3D projection or visibility.
fn vertex_probe(source: &GpuVolumeSource, chunk: &GpuCellChunk) {
    let device = source.device();
    let queue = source.queue();
    let width = 64u32;
    let count = chunk.cpu.faces().len() as u32;
    let height = count.div_ceil(width);
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("generated descriptor vertex probe"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba32Uint,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("integer instance probe"),
        source: wgpu::ShaderSource::Wgsl(format!(r#"
struct Vertex {{ @builtin(position) position:vec4f, @location(0) @interpolate(flat) words:vec4u }};
@vertex fn vs(@location(0) words:vec4u, @builtin(instance_index) instance:u32, @builtin(vertex_index) vertex:u32)->Vertex {{
    let corners=array<vec2f,6>(vec2f(0.,0.),vec2f(1.,0.),vec2f(1.,1.),vec2f(0.,0.),vec2f(1.,1.),vec2f(0.,1.));
    let p=vec2f(f32(instance%64u),f32(instance/64u))+corners[vertex];
    var out:Vertex; out.position=vec4f(p.x/64.*2.-1.,1.-p.y/{height}.*2.,0.,1.); out.words=words; return out;
}}
@fragment fn fs(input:Vertex)->@location(0) vec4u {{ return input.words; }}
"#).into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[],
        immediate_size: 0,
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("quad integer input validation"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs"),
            compilation_options: Default::default(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: 16,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &[wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Uint32x4,
                    offset: 0,
                    shader_location: 0,
                }],
            })],
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: wgpu::TextureFormat::Rgba32Uint,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: Default::default(),
        depth_stencil: None,
        multisample: Default::default(),
        multiview_mask: None,
        cache: None,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    let target_view = target.create_view(&Default::default());
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &target_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&pipeline);
        pass.set_vertex_buffer(0, chunk.buffer.slice(..));
        pass.draw(0..6, 0..count);
    }
    let bytes = u64::from(width) * u64::from(height) * 16;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: bytes,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        target.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 16),
                rows_per_image: Some(height),
            },
        },
        target.size(),
    );
    queue.submit([encoder.finish()]);
    let data = mapped(device, &buffer);
    for (pixel, face) in data
        .as_chunks::<16>()
        .0
        .iter()
        .take(count as usize)
        .zip(chunk.cpu.faces())
    {
        let value: [u32; 4] = std::array::from_fn(|i| {
            u32::from_le_bytes(pixel[i * 4..i * 4 + 4].try_into().unwrap())
        });
        assert_eq!(value[..3], face.cell());
        assert_eq!(value[3] & 255, face.side() as u32);
        assert_eq!(value[3] & 256 != 0, face.boundary() == FaceBoundary::Grid);
    }
}

#[test]
fn actual_gpu_chunks_preserve_integer_words_old_buffers_and_incremental_edit_undo() {
    let (device, queue) = device(false);
    let cpu = source([67, 35, 34]);
    let source = GpuVolumeSource::new(&device, &queue, cpu.clone(), || false).unwrap();
    let before_gpu = MemoryBudget::gpu().used();
    let mut labels = Labels::empty(cpu).unwrap();
    labels.threshold(None, [0., 20.], 7, || false).unwrap();
    let labels = Arc::new(labels);
    let original = surface(MaskOwner::Manual(labels.clone()), 7, None);
    let (first, stats) =
        GpuCellSurface::upload(source.clone(), original.clone(), None, || false).unwrap();
    assert_eq!(stats.kind, UploadKind::Full);
    assert_eq!(stats.uploaded_chunks, original.chunks().count());
    assert_eq!(stats.uploaded_bytes, original.face_count() as u64 * 16);
    verify(&first);
    let snapshot = read(&device, &queue, first.chunk([0, 0, 0]).unwrap());
    let mut edit = labels.try_clone(|| false).unwrap();
    edit.paint(32, &[[31., 16.]], 1, 0, || false).unwrap();
    let edited = surface(MaskOwner::Manual(Arc::new(edit)), 7, Some(&original));
    let (changed, stats) =
        GpuCellSurface::upload(source.clone(), edited.clone(), Some(first.clone()), || {
            false
        })
        .unwrap();
    assert_eq!(stats.kind, UploadKind::Delta);
    assert!(stats.uploaded_chunks > 0 && stats.reused_chunks > 0);
    assert!(Arc::ptr_eq(
        first.chunk([2, 1, 1]).unwrap(),
        changed.chunk([2, 1, 1]).unwrap()
    ));
    assert_eq!(
        snapshot,
        read(&device, &queue, first.chunk([0, 0, 0]).unwrap())
    );
    verify(&changed);
    let mixed = changed
        .chunks()
        .find(|c| {
            c.cpu
                .faces()
                .iter()
                .any(|f| f.boundary() == FaceBoundary::Mask)
                && c.cpu
                    .faces()
                    .iter()
                    .any(|f| f.boundary() == FaceBoundary::Grid)
        })
        .unwrap();
    vertex_probe(&source, mixed);
    let undo_cpu = surface(MaskOwner::Manual(labels.clone()), 7, Some(&edited));
    let (undo, _) = GpuCellSurface::upload(
        source.clone(),
        undo_cpu.clone(),
        Some(changed.clone()),
        || false,
    )
    .unwrap();
    verify(&undo);
    let (same, stats) =
        GpuCellSurface::upload(source.clone(), undo_cpu, Some(undo.clone()), || false).unwrap();
    assert!(Arc::ptr_eq(&same, &undo));
    assert_eq!(stats.kind, UploadKind::Reuse);
    assert_eq!(stats.uploaded_bytes, 0);
    let mut erase = labels.try_clone(|| false).unwrap();
    erase.threshold(None, [20., 30.], 7, || false).unwrap();
    let empty = surface(MaskOwner::Manual(Arc::new(erase)), 7, Some(&original));
    let (removed, stats) =
        GpuCellSurface::upload(source.clone(), empty, Some(undo.clone()), || false).unwrap();
    assert_eq!(stats.kind, UploadKind::Delta);
    assert_eq!(stats.removed_chunks, undo.chunks().count());
    assert_eq!(stats.uploaded_chunks, 0);
    assert_eq!(removed.chunks().count(), 0);
    assert_eq!(stats.staging_admission_bytes, 0);
    drop(same);
    drop(undo);
    drop(changed);
    drop(first);
    drop(removed);
    drop(snapshot);
    fence(&device);
    assert_eq!(MemoryBudget::gpu().used(), before_gpu);
}

#[test]
fn post_allocation_cancellation_policy_shrink_and_equal_geometry_preserve_owners_and_admission() {
    let (device, queue) = device(false);
    let cpu = source([67, 35, 34]);
    let source = GpuVolumeSource::new(&device, &queue, cpu.clone(), || false).unwrap();
    let mut labels = Labels::empty(cpu).unwrap();
    labels.threshold(Some(16), [0., 20.], 7, || false).unwrap();
    let labels = Arc::new(labels);
    let original = surface(MaskOwner::Manual(labels.clone()), 7, None);
    let mut changed = labels.try_clone(|| false).unwrap();
    changed.paint(16, &[[31., 16.]], 1, 0, || false).unwrap();
    let changed = surface(MaskOwner::Manual(Arc::new(changed)), 7, Some(&original));
    let mut other = labels.try_clone(|| false).unwrap();
    other.paint(32, &[[31., 16.]], 1, 9, || false).unwrap();
    let other = surface(MaskOwner::Manual(Arc::new(other)), 7, Some(&original));
    let before_cpu = MemoryBudget::volumes().used();
    let before_gpu = MemoryBudget::gpu().used();
    let (base, _) =
        GpuCellSurface::upload(source.clone(), original.clone(), None, || false).unwrap();
    fence(&device);
    let cpu_used = MemoryBudget::volumes().used();
    let gpu_used = MemoryBudget::gpu().used();
    let directory = (original.chunk_grid().into_iter().product::<usize>()
        * std::mem::size_of::<Option<Arc<GpuCellChunk>>>()
        + std::mem::size_of::<GpuCellSurface>()
        + 2 * std::mem::size_of::<usize>()) as u64;
    let entered = AtomicBool::new(false);
    let cancel = || {
        let hit = MemoryBudget::volumes().used() > cpu_used + directory;
        entered.fetch_or(hit, Ordering::AcqRel);
        hit
    };
    assert!(
        GpuCellSurface::upload(source.clone(), changed.clone(), Some(base.clone()), cancel)
            .is_err()
    );
    assert!(entered.load(Ordering::Acquire));
    fence(&device);
    assert_eq!(MemoryBudget::volumes().used(), cpu_used);
    assert_eq!(MemoryBudget::gpu().used(), gpu_used);
    verify(&base);
    let gpu_budget = MemoryBudget::gpu();
    let old_limit = gpu_budget.limit();
    gpu_budget.set_limit(0);
    let reuse =
        GpuCellSurface::upload(source.clone(), original.clone(), Some(base.clone()), || {
            false
        })
        .unwrap()
        .0;
    assert!(Arc::ptr_eq(&reuse, &base));
    assert!(
        GpuCellSurface::upload(source.clone(), changed.clone(), Some(base.clone()), || {
            false
        })
        .is_err()
    );
    let (equal, stats) =
        GpuCellSurface::upload(source.clone(), other.clone(), Some(base.clone()), || false)
            .unwrap();
    assert_eq!(stats.kind, UploadKind::Delta);
    assert_eq!(stats.uploaded_chunks, 0);
    assert_eq!(stats.reused_chunks, base.chunks().count());
    assert_ne!(equal.surface.revision(), base.surface.revision());
    for c in equal.chunks() {
        assert!(Arc::ptr_eq(c, base.chunk(c.cpu.coordinate()).unwrap()));
    }
    gpu_budget.set_limit(old_limit);
    drop(equal);
    drop(reuse);
    drop(base);
    fence(&device);
    assert_eq!(MemoryBudget::volumes().used(), before_cpu);
    assert_eq!(MemoryBudget::gpu().used(), before_gpu);
}

#[test]
fn source_device_identity_and_buffer_limits_refuse_incompatible_uploads_without_poisoning() {
    let (device, queue) = device(true);
    assert_eq!(device.limits().max_buffer_size, 16384);
    let cpu = source([67, 35, 34]);
    let gpu = GpuVolumeSource::new(&device, &queue, cpu.clone(), || false).unwrap();
    let blank = surface(
        MaskOwner::Manual(Arc::new(Labels::empty(cpu.clone()).unwrap())),
        7,
        None,
    );
    let (empty, _) = GpuCellSurface::upload(gpu.clone(), blank.clone(), None, || false).unwrap();
    let mut full = Labels::empty(cpu.clone()).unwrap();
    full.threshold(None, [0., 20.], 7, || false).unwrap();
    let full = surface(MaskOwner::Manual(Arc::new(full)), 7, None);
    assert!(GpuCellSurface::upload(gpu.clone(), full, Some(empty.clone()), || false).is_err());
    assert_eq!(empty.chunks().count(), 0);
    let duplicate = GpuVolumeSource::new(&device, &queue, cpu, || false).unwrap();
    assert!(
        GpuCellSurface::upload(duplicate, blank.clone(), Some(empty.clone()), || false).is_err()
    );
    let copied = Arc::new(Volume::new(gpu.volume().frames().to_vec()).unwrap());
    let wrong = surface(
        MaskOwner::Manual(Arc::new(Labels::empty(copied).unwrap())),
        7,
        None,
    );
    assert!(GpuCellSurface::upload(gpu.clone(), wrong, None, || false).is_err());
    let (other_device, other_queue) = self::device(false);
    let foreign =
        GpuVolumeSource::new(&other_device, &other_queue, gpu.volume().clone(), || false).unwrap();
    assert!(GpuCellSurface::upload(foreign, blank.clone(), Some(empty.clone()), || false).is_err());
    fence(&other_device);
    assert!(GpuCellSurface::upload(gpu, blank, Some(empty), || true).is_err());
    let mut descriptor = wgpu::BufferDescriptor {
        label: Some("generated invalid mapped size"),
        size: 3,
        usage: wgpu::BufferUsages::VERTEX,
        mapped_at_creation: true,
    };
    assert!(allocation::buffer(&device, &descriptor).is_err());
    descriptor.size = 16;
    let good = allocation::buffer(&device, &descriptor).unwrap();
    good.unmap();
    fence(&device);
}

#[test]
fn supplied_fractional_overlaps_large_ids_and_unknown_boundaries_reach_gpu_unchanged() {
    let (device, queue) = device(false);
    let cpu = source([8, 6, 4]);
    let gpu = GpuVolumeSource::new(&device, &queue, cpu.clone(), || false).unwrap();
    for kind in [FractionalKind::Probability, FractionalKind::Occupancy] {
        let payload: Vec<u8> = (0..4 * 48)
            .map(|i| {
                if (i / 48) % 2 == 1 {
                    0
                } else if i % 8 < 4 {
                    100
                } else {
                    99
                }
            })
            .collect();
        let map = Arc::new(
            Segments::from_fractional(
                cpu.clone(),
                &[65535, 9],
                &[(65535, 1), (65535, 2), (9, 1), (9, 2)],
                &payload,
                FractionalEncoding { maximum: 200, kind },
                Some(true),
                || false,
            )
            .unwrap(),
        );
        let first = surface(MaskOwner::Supplied(map.clone()), 65535, None);
        let (first, _) = GpuCellSurface::upload(gpu.clone(), first, None, || false).unwrap();
        verify(&first);
        assert_eq!(first.surface.key().number, 65535);
        for chunk in first.chunks() {
            for value in read(&device, &queue, chunk) {
                assert_ne!(value[3] & 255, FaceSide::ZMinus as u32);
            }
        }
        let second = surface(MaskOwner::Supplied(map.clone()), 9, None);
        let (second, stats) =
            GpuCellSurface::upload(gpu.clone(), second, Some(first.clone()), || false).unwrap();
        assert_eq!(second.surface.key().number, 9);
        assert_eq!(stats.kind, UploadKind::Full);
        assert_eq!(stats.reused_chunks, 0);
        verify(&second);
        assert_eq!(map.mask(65535).unwrap().raw_value([5, 0, 1]), Some(99));
        assert_eq!(map.mask(65535).unwrap().raw_value([1, 0, 1]), Some(100));
        assert!(map.mask(65535).unwrap().raw_value([0, 0, 0]).is_none());
    }
    fence(&device);
}

#[test]
fn complex_generated_geometry_uses_bounded_batches_and_returns_every_descriptor() {
    let (device, queue) = device(false);
    let template = source([80; 3]);
    let cpu = Arc::new(
        Volume::new(
            template
                .frames()
                .iter()
                .enumerate()
                .map(|(z, frame)| {
                    let mut frame = (**frame).clone();
                    frame.pixels = (0..80 * 80)
                        .map(|i| {
                            if (i % 80 + i / 80 + z) % 2 == 0 {
                                10.
                            } else {
                                0.
                            }
                        })
                        .collect::<Vec<_>>()
                        .into();
                    Arc::new(frame)
                })
                .collect(),
        )
        .unwrap(),
    );
    let source = GpuVolumeSource::new(&device, &queue, cpu.clone(), || false).unwrap();
    let mut labels = Labels::empty(cpu).unwrap();
    labels.threshold(None, [10., 10.], 7, || false).unwrap();
    let cpu = surface(MaskOwner::Manual(Arc::new(labels)), 7, None);
    assert_eq!(cpu.face_count(), 80 * 80 * 80 * 3);
    let before_gpu = MemoryBudget::gpu().used();
    let before_cpu = MemoryBudget::volumes().used();
    let (gpu, stats) = GpuCellSurface::upload(source, cpu.clone(), None, || false).unwrap();
    assert!(stats.gpu_admission_bytes > STAGING_LIMIT);
    assert!(stats.fenced_batches >= 2);
    let largest = gpu.chunks().map(|c| c.admitted_bytes).max().unwrap();
    assert!(stats.staging_admission_bytes <= STAGING_LIMIT + largest);
    assert!(stats.staging_admission_bytes < stats.gpu_admission_bytes);
    assert_eq!(stats.uploaded_bytes, cpu.face_count() as u64 * 16);
    verify(&gpu);
    // Keep the source independently alive while checking geometry retirement.
    let source = gpu.source.clone();
    drop(gpu);
    fence(&device);
    assert_eq!(MemoryBudget::gpu().used(), before_gpu);
    assert_eq!(MemoryBudget::volumes().used(), before_cpu);
    drop(source);
    fence(&device);
}
