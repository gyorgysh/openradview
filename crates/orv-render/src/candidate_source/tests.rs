// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use orv_core::{
    labels::Labels,
    mask::MaskOwner,
    segments::{FractionalEncoding, FractionalKind, Segments},
    study::{Geometry, NiftiMethod, NiftiTransform, demo_study},
    surface::field::{
        CandidateAssemblyLimits, CandidateGrid, CandidateGridLimits, CellGraphLimits,
        CellSeamLimits, ConformingSurfaceLimits, FacePartitionLimits, MaskField,
        PhysicalConformingLimits, PlanarRefinementLimits, PlanarTriangulationLimits,
        RefinementLimits, SurfaceEnclosureLimits,
    },
    volume::Volume,
};
use std::{
    cell::Cell,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};

fn device() -> (wgpu::Device, wgpu::Queue) {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: crate::configured_backends().unwrap(),
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapter = pollster::block_on(instance.request_adapter(&Default::default()))
        .expect("Actual GPU required for candidate geometry tests");
    pollster::block_on(adapter.request_device(&crate::device_descriptor(&adapter))).unwrap()
}
fn volume() -> Arc<Volume> {
    volume_dimensions([3, 3, 2])
}
fn volume_dimensions(dimensions: [u32; 3]) -> Arc<Volume> {
    let template = demo_study().series.remove(0).frames.remove(0);
    Arc::new(
        Volume::new(
            (0..dimensions[2])
                .map(|z| {
                    let mut frame = (*template).clone();
                    frame.width = dimensions[0];
                    frame.height = dimensions[1];
                    frame.pixels = vec![10.; (dimensions[0] * dimensions[1]) as usize].into();
                    frame.geometry = Some(Geometry {
                        position: [1000. + z as f64 * 0.25, -2000., 3000. + z as f64 * 2.],
                        orientation: [-1., 0., 0., 0., 1., 0.],
                        spacing: [1.5, 0.75],
                        frame_of_reference: Some("generated candidate geometry".into()),
                        nifti_transform: None,
                    });
                    Arc::new(frame)
                })
                .collect(),
        )
        .unwrap(),
    )
}
fn geometry(source: Arc<Volume>, tolerance: f64, depth: u8) -> Arc<CandidateAssembly> {
    let mut payload = Vec::new();
    for z in 0..2 {
        for y in 0..3 {
            for x in 0..3 {
                let value = if (x % 2 == 0 && y % 2 == 0 && z == 0) || (x == 1 && y == 1 && z == 1)
                {
                    26
                } else {
                    -6
                };
                payload.push(((value + 200) / 2) as u8);
            }
        }
    }
    let mask = Arc::new(
        Segments::from_fractional(
            source,
            &[1],
            &[(1, 0), (1, 1)],
            &payload,
            FractionalEncoding {
                kind: FractionalKind::Probability,
                maximum: 200,
            },
            Some(true),
            || false,
        )
        .unwrap(),
    );
    let field = MaskField::new(MaskOwner::Supplied(mask), 1).unwrap();
    let limits = grid_limits(tolerance, depth);
    let grid = field
        .candidate_grid([0; 3], [2, 2, 1], 2, limits, || false)
        .unwrap();
    assembly(&grid, limits.seams)
}
fn grid_limits(tolerance: f64, depth: u8) -> CandidateGridLimits {
    let seams = CellSeamLimits {
        partition: FacePartitionLimits {
            max_edges_per_cell: 1024,
            max_points: 2048,
            ..FacePartitionLimits::default()
        },
        ..CellSeamLimits::default()
    };
    CandidateGridLimits {
        max_cells: 256,
        max_directory_bytes: 1024 * 1024,
        graph: CellGraphLimits::default(),
        planar_refinement: PlanarRefinementLimits {
            curves: RefinementLimits {
                precision_bits: 6,
                max_vertices: 4096,
                ..RefinementLimits::default()
            },
            max_vertices: 4096,
            ..PlanarRefinementLimits::default()
        },
        triangulation: PlanarTriangulationLimits::default(),
        enclosure: SurfaceEnclosureLimits::default(),
        surface: PhysicalConformingLimits {
            max_error_mm: tolerance,
            max_depth: depth,
            max_vertices: 2048,
            max_triangles: 2048,
            max_operations: 2_000_000,
            max_bytes: ConformingSurfaceLimits::default().max_bytes,
        },
        seams,
    }
}
fn assembly(grid: &CandidateGrid, seams: CellSeamLimits) -> Arc<CandidateAssembly> {
    Arc::new(
        CandidateAssembly::build(
            &[grid],
            CandidateAssemblyLimits {
                max_cells: 256,
                max_directory_bytes: 1024 * 1024,
                seams,
            },
            || false,
        )
        .unwrap(),
    )
}
fn single_voxel_map(source: Arc<Volume>) -> Arc<Labels> {
    let [width, height, _] = source.dimensions();
    let frames = source
        .frames()
        .iter()
        .enumerate()
        .map(|(z, frame)| {
            let mut frame = (**frame).clone();
            frame.pixels = (0..width * height)
                .map(|i| if i == 0 && z == 0 { 1. } else { 0. })
                .collect::<Vec<_>>()
                .into();
            frame.geometry.as_mut().unwrap().nifti_transform = Some(NiftiTransform {
                method: NiftiMethod::Sform,
                code: 1,
            });
            Arc::new(frame)
        })
        .collect();
    Arc::new(Labels::import(source, &Volume::new(frames).unwrap(), || false).unwrap())
}
fn fence(device: &wgpu::Device) {
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(Duration::from_secs(30)),
        })
        .unwrap();
}
fn mapped(device: &wgpu::Device, buffer: &wgpu::Buffer) -> Vec<u8> {
    let (tx, rx) = mpsc::sync_channel(1);
    buffer.map_async(wgpu::MapMode::Read, .., move |result| {
        let _ = tx.send(result);
    });
    fence(device);
    rx.recv_timeout(Duration::from_secs(10)).unwrap().unwrap();
    let data = buffer.get_mapped_range(..).unwrap();
    let bytes = data.to_vec();
    drop(data);
    buffer.unmap();
    bytes
}
fn read(source: &GpuVolumeSource, chunk: &GpuCandidateChunk) -> Vec<[u32; 6]> {
    let device = source.device();
    let output = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("candidate integer readback"),
        size: chunk.data_bytes(),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_buffer_to_buffer(chunk.buffer(), 0, &output, 0, chunk.data_bytes());
    source.queue().submit([encoder.finish()]);
    mapped(device, &output)
        .as_chunks::<24>()
        .0
        .iter()
        .map(|bytes| {
            std::array::from_fn(|i| u32::from_le_bytes(bytes[i * 4..i * 4 + 4].try_into().unwrap()))
        })
        .collect()
}

/// Checks integer vertex-attribute consumption, not physical projection or visibility.
fn vertex_probe(source: &GpuVolumeSource, chunk: &GpuCandidateChunk, expected: &[[u32; 6]]) {
    let device = source.device();
    let width = 64u32;
    let count = u32::try_from(expected.len()).unwrap();
    let height = count.div_ceil(width);
    let texture = || {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some("candidate vertex input probe"),
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
        })
    };
    let cells = texture();
    let positions = texture();
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("candidate integer vertex probe"),
        source: wgpu::ShaderSource::Wgsl(format!(r#"
struct Vertex {{ @builtin(position) position:vec4f, @location(0) @interpolate(flat) cell:vec3u, @location(1) @interpolate(flat) local:vec3u }};
struct Output {{ @location(0) cell:vec4u, @location(1) local:vec4u }};
@vertex fn vs(@location(0) cell:vec3u, @location(1) local:vec3u, @builtin(instance_index) instance:u32, @builtin(vertex_index) vertex:u32)->Vertex {{
    let corners=array<vec2f,6>(vec2f(0.,0.),vec2f(1.,0.),vec2f(1.,1.),vec2f(0.,0.),vec2f(1.,1.),vec2f(0.,1.));
    let p=vec2f(f32(instance%64u),f32(instance/64u))+corners[vertex];
    var out:Vertex; out.position=vec4f(p.x/64.*2.-1.,1.-p.y/{height}.*2.,0.,1.); out.cell=cell; out.local=local; return out;
}}
@fragment fn fs(input:Vertex)->Output {{ var out:Output; out.cell=vec4u(input.cell,1u); out.local=vec4u(input.local,1u); return out; }}
"#).into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[],
        immediate_size: 0,
    });
    let target = || {
        Some(wgpu::ColorTargetState {
            format: wgpu::TextureFormat::Rgba32Uint,
            blend: None,
            write_mask: wgpu::ColorWrites::ALL,
        })
    };
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("candidate integer vertex input validation"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs"),
            compilation_options: Default::default(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: VERTEX_BYTES,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &[
                    wgpu::VertexAttribute {
                        format: wgpu::VertexFormat::Uint32x3,
                        offset: 0,
                        shader_location: 0,
                    },
                    wgpu::VertexAttribute {
                        format: wgpu::VertexFormat::Uint32x3,
                        offset: 12,
                        shader_location: 1,
                    },
                ],
            })],
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs"),
            compilation_options: Default::default(),
            targets: &[target(), target()],
        }),
        primitive: Default::default(),
        depth_stencil: None,
        multisample: Default::default(),
        multiview_mask: None,
        cache: None,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    let views = [
        cells.create_view(&Default::default()),
        positions.create_view(&Default::default()),
    ];
    {
        let attachment = |view| {
            Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })
        };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[attachment(&views[0]), attachment(&views[1])],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&pipeline);
        pass.set_vertex_buffer(0, chunk.buffer().slice(..));
        pass.draw(0..6, 0..count);
    }
    let bytes = u64::from(width) * u64::from(height) * 16;
    let output = || {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: bytes,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        })
    };
    let outputs = [output(), output()];
    for (texture, output) in [&cells, &positions].into_iter().zip(&outputs) {
        encoder.copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: output,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(width * 16),
                    rows_per_image: Some(height),
                },
            },
            texture.size(),
        );
    }
    source.queue().submit([encoder.finish()]);
    let cell_pixels = mapped(device, &outputs[0]);
    let local_pixels = mapped(device, &outputs[1]);
    for (i, words) in expected.iter().enumerate() {
        let at = i * 16;
        for axis in 0..3 {
            assert_eq!(
                u32::from_le_bytes(
                    cell_pixels[at + axis * 4..at + axis * 4 + 4]
                        .try_into()
                        .unwrap()
                ),
                words[axis]
            );
            assert_eq!(
                u32::from_le_bytes(
                    local_pixels[at + axis * 4..at + axis * 4 + 4]
                        .try_into()
                        .unwrap()
                ),
                words[axis + 3]
            );
        }
    }
}
fn expected(assembly: &CandidateAssembly, origin: [u32; 3]) -> Vec<[u32; 6]> {
    let cell = assembly
        .cells()
        .iter()
        .find(|cell| cell.origin == origin.map(|v| v as usize))
        .unwrap();
    let CandidateCellState::Candidate { mesh } = cell.state else {
        panic!("expected candidate")
    };
    let mesh = &assembly.meshes()[usize::from(mesh)];
    mesh.triangles()
        .iter()
        .filter(|triangle| matches!(triangle.state, ConformingFacetState::WithinTolerance { .. }))
        .flat_map(|triangle| {
            triangle.vertices.map(|id| {
                let position = mesh.vertices()[id as usize].position().unwrap();
                [
                    origin[0],
                    origin[1],
                    origin[2],
                    position[0],
                    position[1],
                    position[2],
                ]
            })
        })
        .collect()
}

#[test]
fn actual_gpu_upload_preserves_integer_triangle_vertices_and_reuses_equal_input() {
    let (device, queue) = device();
    let cpu = volume();
    let source = GpuVolumeSource::new(&device, &queue, cpu.clone(), || false).unwrap();
    let assembly = geometry(cpu, 1000., 2);
    let (uploaded, stats) = GpuCandidateSurface::upload(
        source.clone(),
        assembly.clone(),
        None,
        CandidateUploadLimits::default(),
        || false,
    )
    .unwrap();
    assert!(stats.represented_triangles > 0);
    assert_eq!(stats.kind, UploadKind::Full);
    assert_eq!(stats.uploaded_chunks, uploaded.chunks().len());
    assert_eq!(stats.uploaded_bytes, uploaded.referenced_data_bytes());
    let mut count = 0;
    for chunk in uploaded.chunks() {
        let actual = read(&source, chunk);
        assert_eq!(actual, expected(&assembly, chunk.origin()));
        count += chunk.triangle_count();
    }
    let first = &uploaded.chunks()[0];
    vertex_probe(&source, first, &expected(&assembly, first.origin()));
    assert_eq!(count, stats.represented_triangles);
    assert!(
        GpuCandidateSurface::upload(
            source.clone(),
            assembly.clone(),
            Some(uploaded.clone()),
            CandidateUploadLimits {
                max_triangles: stats.represented_triangles - 1,
                ..CandidateUploadLimits::default()
            },
            || false,
        )
        .is_err()
    );
    assert!(
        GpuCandidateSurface::upload(
            source.clone(),
            assembly.clone(),
            Some(uploaded.clone()),
            CandidateUploadLimits {
                max_bytes: uploaded.referenced_data_bytes() - 1,
                ..CandidateUploadLimits::default()
            },
            || false,
        )
        .is_err()
    );
    let (again, repeated) = GpuCandidateSurface::upload(
        source,
        assembly,
        Some(uploaded.clone()),
        CandidateUploadLimits::default(),
        || false,
    )
    .unwrap();
    assert!(Arc::ptr_eq(&again, &uploaded));
    assert_eq!(repeated.kind, UploadKind::Reuse);
    assert_eq!(repeated.uploaded_bytes, 0);
    assert_eq!(repeated.reused_chunks, uploaded.chunks().len());
}

#[test]
fn late_upload_cancellation_releases_admission_without_publishing() {
    let (device, queue) = device();
    let cpu = volume();
    let source = GpuVolumeSource::new(&device, &queue, cpu.clone(), || false).unwrap();
    let assembly = geometry(cpu, 1000., 2);
    let cpu_before = MemoryBudget::volumes().used();
    let gpu_before = MemoryBudget::gpu().used();
    let directory = (assembly.meshes().len() * std::mem::size_of::<Arc<GpuCandidateChunk>>()
        + std::mem::size_of::<GpuCandidateSurface>()
        + 2 * std::mem::size_of::<usize>()) as u64;
    let entered = AtomicBool::new(false);
    let result = GpuCandidateSurface::upload(
        source.clone(),
        assembly.clone(),
        None,
        CandidateUploadLimits::default(),
        || {
            let hit = MemoryBudget::volumes().used() > cpu_before + directory;
            entered.fetch_or(hit, Ordering::AcqRel);
            hit
        },
    );
    assert!(matches!(result, Err("Cancelled")));
    assert!(entered.load(Ordering::Acquire));
    fence(&device);
    assert_eq!(MemoryBudget::volumes().used(), cpu_before);
    assert_eq!(MemoryBudget::gpu().used(), gpu_before);
}

#[test]
fn edited_revision_reuses_equal_gpu_chunks_and_removes_absent_geometry() {
    let (device, queue) = device();
    let cpu = volume_dimensions([8, 3, 2]);
    let source = GpuVolumeSource::new(&device, &queue, cpu.clone(), || false).unwrap();
    let map = single_voxel_map(cpu);
    let before = MaskField::new(MaskOwner::Manual(map.clone()), 1).unwrap();
    let limits = grid_limits(1000., 2);
    let grid = before
        .candidate_grid([0; 3], [1; 3], 2, limits, || false)
        .unwrap();
    let old_assembly = assembly(&grid, limits.seams);
    let (old_gpu, old_stats) = GpuCandidateSurface::upload(
        source.clone(),
        old_assembly,
        None,
        CandidateUploadLimits::default(),
        || false,
    )
    .unwrap();
    assert_eq!(old_gpu.chunks().len(), 1);
    assert!(old_stats.represented_triangles > 0);
    let mut edited = map.try_clone(|| false).unwrap();
    edited.paint(1, &[[6., 1.]], 1, 1, || false).unwrap();
    let edited = Arc::new(edited);
    let after = MaskField::new(MaskOwner::Manual(edited.clone()), 1).unwrap();
    let (edited_grid, update) = after
        .candidate_grid_reusing([0; 3], [1; 3], 2, limits, Some(&grid), || false)
        .unwrap();
    assert_eq!(update.reused_cells, 1);
    let new_assembly = assembly(&edited_grid, limits.seams);
    let (new_gpu, stats) = GpuCandidateSurface::upload(
        source.clone(),
        new_assembly.clone(),
        Some(old_gpu.clone()),
        CandidateUploadLimits::default(),
        || false,
    )
    .unwrap();
    assert_eq!(stats.kind, UploadKind::Delta);
    assert_eq!(stats.uploaded_chunks, 0);
    assert_eq!(stats.reused_chunks, 1);
    assert!(Arc::ptr_eq(&new_gpu.chunks()[0], &old_gpu.chunks()[0]));
    assert_ne!(
        new_gpu.assembly().field().revision(),
        old_gpu.assembly().field().revision()
    );
    assert_eq!(
        read(&source, &new_gpu.chunks()[0]),
        expected(&new_assembly, [0; 3])
    );
    let mut removed = edited.try_clone(|| false).unwrap();
    removed.paint(0, &[[0., 0.]], 1, 0, || false).unwrap();
    let removed = MaskField::new(MaskOwner::Manual(Arc::new(removed)), 1).unwrap();
    let (empty_grid, _) = removed
        .candidate_grid_reusing([0; 3], [1; 3], 2, limits, Some(&edited_grid), || false)
        .unwrap();
    let (empty, stats) = GpuCandidateSurface::upload(
        source,
        assembly(&empty_grid, limits.seams),
        Some(new_gpu),
        CandidateUploadLimits::default(),
        || false,
    )
    .unwrap();
    assert!(empty.chunks().is_empty());
    assert_eq!(stats.removed_chunks, 1);
    assert_eq!(stats.uploaded_bytes, 0);
}

#[test]
fn upload_respects_source_limits_uncertainty_and_cancellation() {
    let (device, queue) = device();
    let cpu = volume();
    let source = GpuVolumeSource::new(&device, &queue, cpu.clone(), || false).unwrap();
    let assembly = geometry(cpu, 0., 0);
    let (uploaded, stats) = GpuCandidateSurface::upload(
        source.clone(),
        assembly.clone(),
        None,
        CandidateUploadLimits::default(),
        || false,
    )
    .unwrap();
    assert!(stats.unresolved_triangles > 0);
    for chunk in uploaded.chunks() {
        assert_eq!(read(&source, chunk), expected(&assembly, chunk.origin()));
    }
    let wrong = GpuVolumeSource::new(&device, &queue, volume(), || false).unwrap();
    assert!(
        GpuCandidateSurface::upload(
            wrong,
            assembly.clone(),
            None,
            CandidateUploadLimits::default(),
            || false
        )
        .is_err()
    );
    assert!(
        GpuCandidateSurface::upload(
            source.clone(),
            assembly.clone(),
            None,
            CandidateUploadLimits {
                max_triangles: MAX_TRIANGLES + 1,
                ..CandidateUploadLimits::default()
            },
            || false
        )
        .is_err()
    );
    assert!(
        GpuCandidateSurface::upload(
            source.clone(),
            assembly.clone(),
            None,
            CandidateUploadLimits {
                max_bytes: MAX_BYTES + 1,
                ..CandidateUploadLimits::default()
            },
            || false
        )
        .is_err()
    );
    assert!(
        GpuCandidateSurface::upload(
            source.clone(),
            assembly.clone(),
            None,
            CandidateUploadLimits::default(),
            || true
        )
        .is_err()
    );
    let calls = Cell::new(0usize);
    assert!(
        GpuCandidateSurface::upload(
            source,
            assembly,
            None,
            CandidateUploadLimits::default(),
            || {
                calls.set(calls.get() + 1);
                calls.get() == 3
            }
        )
        .is_err()
    );
}
