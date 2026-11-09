// SPDX-License-Identifier: GPL-3.0-or-later
mod acceleration;
mod affine;
mod coordinated;
use super::*;
use crate::candidate_opaque::{
    CandidateOpaqueImage, CandidateOpaqueRenderer, CandidateOpaqueScene,
};
use crate::candidate_projection::{CandidateImage, CandidateProjection, GpuCandidateProjector};
use crate::material_volume::Lighting;
use orv_core::{
    ViewState,
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
    volume_material::{MaterialPoint, VolumeMaterial},
    volume_view::{Camera, PhysicalClip, Projection, VolumeView},
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
        flags: crate::instance_flags(),
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
    volume_geometry(dimensions, 1000., true, true)
}
fn volume_geometry(dimensions: [u32; 3], offset: f64, shear: bool, reverse: bool) -> Arc<Volume> {
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
                        position: [
                            offset + if shear { z as f64 * 0.25 } else { 0. },
                            -2. * offset + if shear { z as f64 * 0.5 } else { 0. },
                            3. * offset + z as f64 * 2.,
                        ],
                        orientation: if reverse {
                            [-1., 0., 0., 0., 1., 0.]
                        } else {
                            [1., 0., 0., 0., 1., 0.]
                        },
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
fn volume_with_missing_sample() -> Arc<Volume> {
    let base = volume_geometry([3, 3, 2], 1000., true, true);
    let frames = base
        .frames()
        .iter()
        .enumerate()
        .map(|(z, frame)| {
            let mut frame = (**frame).clone();
            frame.validity = Some(
                (0..9)
                    .map(|i| u8::from(!(z == 0 && i == 0)))
                    .collect::<Vec<_>>()
                    .into(),
            );
            Arc::new(frame)
        })
        .collect();
    Arc::new(Volume::new(frames).unwrap())
}
fn geometry(source: Arc<Volume>, tolerance: f64, depth: u8) -> Arc<CandidateAssembly> {
    geometry_scaled(source, tolerance, depth, 1)
}
fn geometry_scaled(
    source: Arc<Volume>,
    tolerance: f64,
    depth: u8,
    scale: i16,
) -> Arc<CandidateAssembly> {
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
                payload.push(((value * scale + 200) / 2) as u8);
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
fn texture_bytes(source: &GpuVolumeSource, texture: &wgpu::Texture, bpp: u32) -> Vec<u8> {
    let size = texture.size();
    let stride = (size.width * bpp).div_ceil(256) * 256;
    let buffer = source.device().create_buffer(&wgpu::BufferDescriptor {
        label: Some("generated candidate projection readback"),
        size: u64::from(stride) * u64::from(size.height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = source.device().create_command_encoder(&Default::default());
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
    source.queue().submit([encoder.finish()]);
    mapped(source.device(), &buffer)
        .chunks_exact(stride as usize)
        .flat_map(|row| row[..(size.width * bpp) as usize].iter().copied())
        .collect()
}
fn half(bits: u16) -> f64 {
    let sign = if bits & 32768 == 0 { 1. } else { -1. };
    let exponent = (bits >> 10) & 31;
    let fraction = f64::from(bits & 1023);
    assert_ne!(exponent, 31, "Non-finite candidate normal");
    sign * if exponent == 0 {
        fraction * 2f64.powi(-24)
    } else {
        (1. + fraction / 1024.) * 2f64.powi(i32::from(exponent) - 15)
    }
}
fn image_pixels(image: &CandidateImage) -> (Vec<[u32; 4]>, Vec<f32>, Vec<[f64; 4]>) {
    let source = image.request().surface.source();
    let cells = texture_bytes(source, image.cells(), 16)
        .as_chunks::<16>()
        .0
        .iter()
        .map(|bytes| {
            std::array::from_fn(|i| u32::from_le_bytes(bytes[i * 4..i * 4 + 4].try_into().unwrap()))
        })
        .collect();
    let depth = texture_bytes(source, image.depth(), 4)
        .as_chunks::<4>()
        .0
        .iter()
        .map(|bytes| f32::from_le_bytes(*bytes))
        .collect();
    let normals = texture_bytes(source, image.normals(), 8)
        .as_chunks::<8>()
        .0
        .iter()
        .map(|bytes| {
            std::array::from_fn(|i| {
                half(u16::from_le_bytes(
                    bytes[i * 2..i * 2 + 2].try_into().unwrap(),
                ))
            })
        })
        .collect();
    (cells, depth, normals)
}
fn read(source: &GpuVolumeSource, chunk: &GpuCandidateChunk) -> Vec<[u32; 16]> {
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
        .as_chunks::<64>()
        .0
        .iter()
        .map(|bytes| {
            std::array::from_fn(|i| u32::from_le_bytes(bytes[i * 4..i * 4 + 4].try_into().unwrap()))
        })
        .collect()
}

/// Checks integer vertex-attribute consumption, not physical projection or visibility.
fn vertex_probe(source: &GpuVolumeSource, chunk: &GpuCandidateChunk, expected: &[[u32; 16]]) {
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
                array_stride: TRIANGLE_BYTES,
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
fn expected(assembly: &CandidateAssembly, origin: [u32; 3]) -> Vec<[u32; 16]> {
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
        .map(|triangle| {
            let mut words = [0u32; 16];
            words[..3].copy_from_slice(&origin);
            for (i, id) in triangle.vertices.into_iter().enumerate() {
                words[3 + 3 * i..6 + 3 * i]
                    .copy_from_slice(&mesh.vertices()[id as usize].position().unwrap());
            }
            let samples: [i16; 8] = std::array::from_fn(|i| {
                let at = std::array::from_fn(|a| origin[a] as usize + ((i >> a) & 1));
                assembly.field().sample(at).unwrap().value.unwrap()
            });
            for i in 0..4 {
                let low = u32::from(u16::from_le_bytes(samples[2 * i].to_le_bytes()));
                let high = u32::from(u16::from_le_bytes(samples[2 * i + 1].to_le_bytes()));
                words[12 + i] = low | (high << 16);
            }
            words
        })
        .collect()
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a.into_iter().zip(b).map(|(a, b)| a * b).sum()
}
fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    std::array::from_fn(|i| a[i] - b[i])
}
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
struct ReferenceHit {
    cell: [u32; 3],
    depth: f64,
    margin: f64,
    voxel: [f64; 3],
}
fn reference_normal(request: &CandidateProjection, hit: &ReferenceHit) -> Option<[f64; 3]> {
    let field = request.surface.assembly().field();
    let lower = hit.cell.map(|v| v as usize);
    let point: [f64; 3] = std::array::from_fn(|axis| hit.voxel[axis] - lower[axis] as f64);
    let mut gradient = [0.; 3];
    for i in 0..8 {
        let position = std::array::from_fn(|axis| lower[axis] + ((i >> axis) & 1));
        let sample = field.sample(position)?;
        if !sample.source_valid {
            return None;
        }
        let value = f64::from(sample.value?);
        for (axis, out) in gradient.iter_mut().enumerate() {
            let mut factor = if i & (1 << axis) == 0 { -1. } else { 1. };
            for (other, q) in point.into_iter().enumerate() {
                if other != axis {
                    factor *= if i & (1 << other) == 0 { 1. - q } else { q };
                }
            }
            *out += value * factor;
        }
    }
    let source = request.surface.source().volume();
    let physical: [f64; 3] = std::array::from_fn(|world| {
        let mut unit = [0.; 3];
        unit[world] = 1.;
        dot(gradient, source.world_vector_to_voxel(unit))
    });
    let length = dot(physical, physical).sqrt();
    (length > 1e-6 && length.is_finite()).then(|| physical.map(|v| -v / length))
}
// Independent f64 ray/triangle intersection in source coordinates. Edge contacts
// have implementation-defined raster coverage and are excluded from strict checks.
fn reference(
    request: &CandidateProjection,
    pixel: [u32; 2],
    edge_slack: f64,
    clip_slack: f64,
) -> Option<ReferenceHit> {
    let source = request.surface.source().volume();
    let frame = request
        .camera
        .frame(
            &request.display,
            f64::from(request.size[0]) / f64::from(request.size[1]),
        )
        .unwrap();
    let (world, direction) = frame.ray(std::array::from_fn(|a| {
        (f64::from(pixel[a]) + 0.5) / f64::from(request.size[a])
    }));
    let origin = source.world_to_voxel(world);
    let vector = source.world_vector_to_voxel(direction);
    let denominator = f64::from(ConformingSurfaceMesh::POSITION_DENOMINATOR);
    let mut best: Option<(f64, ReferenceHit)> = None;
    for cell in request.surface.assembly().cells() {
        let CandidateCellState::Candidate { mesh } = cell.state else {
            continue;
        };
        let mesh = &request.surface.assembly().meshes()[usize::from(mesh)];
        for triangle in mesh.triangles() {
            if !matches!(triangle.state, ConformingFacetState::WithinTolerance { .. }) {
                continue;
            }
            let points = triangle.vertices.map(|id| {
                let local = mesh.vertices()[id as usize].position().unwrap();
                std::array::from_fn(|a| cell.origin[a] as f64 + f64::from(local[a]) / denominator)
            });
            let e1 = sub(points[1], points[0]);
            let e2 = sub(points[2], points[0]);
            let p = cross(vector, e2);
            let determinant = dot(e1, p);
            if determinant.abs() < 1e-12 {
                continue;
            }
            let delta = sub(origin, points[0]);
            let u = dot(delta, p) / determinant;
            let q = cross(delta, e1);
            let v = dot(vector, q) / determinant;
            let margin = u.min(v).min(1. - u - v);
            if margin < -edge_slack {
                continue;
            }
            let t = dot(e2, q) / determinant;
            let voxel = std::array::from_fn(|a| origin[a] + vector[a] * t);
            let world_point = source.voxel_to_world(voxel);
            if request.clip.as_ref().is_some_and(|clip| {
                (0..3).any(|a| {
                    world_point[a] < clip.minimum_mm[a] - clip_slack
                        || world_point[a] > clip.maximum_mm[a] + clip_slack
                })
            }) {
                continue;
            }
            let depth = dot(sub(world_point, source.center()), direction);
            if best.as_ref().is_none_or(|(prior, _)| t < *prior) {
                best = Some((
                    t,
                    ReferenceHit {
                        cell: cell.origin.map(|v| v as u32),
                        depth,
                        margin,
                        voxel,
                    },
                ));
            }
        }
    }
    best.map(|(_, hit)| hit)
}

fn assert_projection(image: &CandidateImage) {
    let request = image.request();
    let (cells, depths, normals) = image_pixels(image);
    let mut interior = 0usize;
    let mut checked_normals = 0usize;
    for (i, ((cell, depth), normal)) in cells.iter().zip(&depths).zip(&normals).enumerate() {
        let pixel = [i as u32 % request.size[0], i as u32 / request.size[0]];
        if cell[3] != 0 {
            assert!(
                reference(request, pixel, 0.002, 0.001).is_some(),
                "unexpected hit at {pixel:?}"
            );
        } else {
            assert_eq!(*normal, [0.; 4]);
        }
        if let Some(hit) = reference(request, pixel, 0., 0.)
            && hit.margin > 0.001
        {
            interior += 1;
            assert_eq!(cell[..3], hit.cell, "pixel {pixel:?}");
            assert_eq!(
                cell[3],
                u32::from(request.surface.assembly().field().key().number)
            );
            assert!(
                (f64::from(*depth) - hit.depth).abs() < 0.001,
                "pixel {pixel:?}: {depth} vs {}",
                hit.depth
            );
            if let Some(expected) = reference_normal(request, &hit) {
                checked_normals += 1;
                assert_eq!(normal[3], 1., "normal unavailable at {pixel:?}");
                for axis in 0..3 {
                    assert!(
                        (normal[axis] - expected[axis]).abs() < 0.004,
                        "normal at {pixel:?}: {normal:?} vs {expected:?}"
                    );
                }
            }
        }
    }
    assert!(
        interior > 10,
        "the fixture must exercise interior triangle coverage"
    );
    assert!(
        checked_normals > 10,
        "the fixture must exercise physical field normals"
    );
}
fn assert_vertex_bound(image: &CandidateImage) {
    let source = image.request().surface.source();
    let volume = source.volume();
    let basis: [[f64; 3]; 3] = std::array::from_fn(|axis| {
        let mut vector = [0.; 3];
        vector[axis] = 1.;
        volume.voxel_vector_to_world(vector)
    });
    let center = volume.dimensions().map(|d| (d - 1) as f64 * 0.5);
    let denominator = f64::from(ConformingSurfaceMesh::POSITION_DENOMINATOR);
    for chunk in image.request().surface.chunks() {
        for words in read(source, chunk) {
            for vertex in 0..3 {
                let local = 3 + 3 * vertex;
                let exact_q = std::array::from_fn(|a| {
                    f64::from(words[a]) - center[a] + f64::from(words[local + a]) / denominator
                });
                let float_q: [f32; 3] = std::array::from_fn(|a| {
                    let grid = ConformingSurfaceMesh::POSITION_DENOMINATOR;
                    let whole = words[local + a] / grid;
                    let fraction = words[local + a] % grid;
                    (words[a] + whole) as f32 - center[a] as f32
                        + fraction as f32 / denominator as f32
                });
                let exact = volume.voxel_vector_to_world(exact_q);
                let approximate: [f64; 3] = std::array::from_fn(|world| {
                    f64::from(
                        basis[0][world] as f32 * float_q[0]
                            + basis[1][world] as f32 * float_q[1]
                            + basis[2][world] as f32 * float_q[2],
                    )
                });
                let error: f64 = exact
                    .into_iter()
                    .zip(approximate)
                    .map(|(a, b)| (a - b).abs())
                    .sum();
                assert!(
                    error <= image.vertex_error_upper_mm(),
                    "{error} > {}",
                    image.vertex_error_upper_mm()
                );
            }
        }
    }
}

#[test]
fn actual_gpu_candidate_projection_matches_physical_ray_triangles() {
    let (device, queue) = device();
    for (offset, shear, reverse, yaw, pitch) in [
        (0., false, false, 0., 0.),
        (1_000_000., true, true, 27., -17.),
    ] {
        let cpu = volume_geometry([3, 3, 2], offset, shear, reverse);
        let source = GpuVolumeSource::new(&device, &queue, cpu.clone(), || false).unwrap();
        let assembly = geometry(cpu.clone(), 1000., 2);
        let (surface, upload) = GpuCandidateSurface::upload(
            source.clone(),
            assembly,
            None,
            CandidateUploadLimits::default(),
            || false,
        )
        .unwrap();
        assert!(upload.represented_triangles > 0);
        let projector = GpuCandidateProjector::new(source).unwrap();
        let mut camera = Camera::fit(&cpu);
        camera.yaw_degrees = yaw;
        camera.pitch_degrees = pitch;
        let request = CandidateProjection {
            surface,
            camera,
            display: ViewState {
                pan: [0.021, -0.017],
                ..Default::default()
            },
            clip: None,
            size: [83, 79],
        };
        let image = projector.render(request.clone(), || false).unwrap();
        assert_eq!(image.statistics().triangles, upload.represented_triangles);
        assert!(image.statistics().candidate_pixels > 0);
        assert!(matches!(
            projector.render_with_limit(
                request.clone(),
                image.statistics().candidate_pixels - 1,
                || false
            ),
            Err("Candidate projection exceeds its bounded pixel-work limit")
        ));
        assert!(image.vertex_error_upper_mm() > 0. && image.vertex_error_upper_mm() < 0.01);
        assert_vertex_bound(&image);
        assert_projection(&image);
        let old = image_pixels(&image);
        if offset == 0. {
            let cap = u64::from(image.statistics().max_triangle_pixels);
            assert!(image.statistics().candidate_pixels > cap);
            let batched = projector
                .render_with_budgets(
                    request.clone(),
                    crate::candidate_projection::MAX_FRAME_CANDIDATE_PIXELS,
                    cap,
                    || false,
                )
                .unwrap();
            assert!(batched.statistics().fenced_batches > 1);
            assert_eq!(image_pixels(&batched), old);
        }
        let center = cpu.center();
        let mut clipped = request;
        clipped.clip = Some(PhysicalClip {
            minimum_mm: [center[0] - 10., center[1] - 10., center[2] - 10.],
            maximum_mm: [center[0] + 0.25, center[1] + 10., center[2] + 10.],
        });
        let clipped = projector.render(clipped, || false).unwrap();
        assert_projection(&clipped);
        assert_eq!(image_pixels(&image), old);
    }
}

#[test]
fn candidate_projection_is_stable_under_large_world_translation() {
    let (device, queue) = device();
    let mut results = Vec::new();
    for offset in [0., 1_000_000.] {
        let cpu = volume_geometry([3, 3, 2], offset, true, true);
        let source = GpuVolumeSource::new(&device, &queue, cpu.clone(), || false).unwrap();
        let (surface, _) = GpuCandidateSurface::upload(
            source.clone(),
            geometry(cpu.clone(), 1000., 2),
            None,
            CandidateUploadLimits::default(),
            || false,
        )
        .unwrap();
        let mut camera = Camera::fit(&cpu);
        camera.yaw_degrees = 27.;
        camera.pitch_degrees = -17.;
        let image = GpuCandidateProjector::new(source)
            .unwrap()
            .render(
                CandidateProjection {
                    surface,
                    camera,
                    display: ViewState::default(),
                    clip: None,
                    size: [83, 79],
                },
                || false,
            )
            .unwrap();
        results.push(image_pixels(&image));
    }
    assert_eq!(results[0].0, results[1].0);
    for (a, b) in results[0].1.iter().zip(&results[1].1) {
        assert!((a - b).abs() < 1e-5);
    }
    assert_eq!(results[0].2, results[1].2);
}

#[test]
fn candidate_projection_handles_empty_sources_scopes_and_cancellation() {
    let (device, queue) = device();
    let cpu = volume();
    let source = GpuVolumeSource::new(&device, &queue, cpu.clone(), || false).unwrap();
    let field = MaskField::new(
        MaskOwner::Manual(Arc::new(Labels::empty(cpu.clone()).unwrap())),
        1,
    )
    .unwrap();
    let limits = grid_limits(1000., 2);
    let grid = field
        .candidate_grid([0; 3], [2, 2, 1], 2, limits, || false)
        .unwrap();
    let (surface, _) = GpuCandidateSurface::upload(
        source.clone(),
        assembly(&grid, limits.seams),
        None,
        CandidateUploadLimits::default(),
        || false,
    )
    .unwrap();
    assert!(surface.chunks().is_empty());
    let projector = GpuCandidateProjector::new(source.clone()).unwrap();
    let request = CandidateProjection {
        surface,
        camera: Camera::fit(&cpu),
        display: ViewState::default(),
        clip: None,
        size: [41, 37],
    };
    let image = projector.render(request.clone(), || false).unwrap();
    assert_eq!(image.statistics().triangles, 0);
    assert!(image_pixels(&image).0.iter().all(|v| *v == [0; 4]));
    assert!(projector.render(request.clone(), || true).is_err());
    let gpu_before = MemoryBudget::gpu().used();
    let cpu_before = MemoryBudget::volumes().used();
    let entered = AtomicBool::new(false);
    assert!(matches!(
        projector.render(request.clone(), || {
            let allocated = MemoryBudget::gpu().used() > gpu_before;
            entered.fetch_or(allocated, Ordering::AcqRel);
            allocated
        }),
        Err("Cancelled")
    ));
    assert!(entered.load(Ordering::Acquire));
    fence(&device);
    assert_eq!(MemoryBudget::gpu().used(), gpu_before);
    assert_eq!(MemoryBudget::volumes().used(), cpu_before);
    let mut invalid = request.clone();
    invalid.size = [0, 37];
    assert!(projector.render(invalid, || false).is_err());
    let mut invalid = request.clone();
    invalid.camera.vertical_span_mm = 0.0001;
    invalid.display.zoom = 50.;
    assert!(matches!(
        projector.render(invalid, || false),
        Err("Candidate screen projection exceeds GPU coordinate precision")
    ));
    let mut invalid = request.clone();
    invalid.clip = Some(PhysicalClip {
        minimum_mm: [1.; 3],
        maximum_mm: [0.; 3],
    });
    assert!(projector.render(invalid, || false).is_err());
    let other =
        GpuVolumeSource::new(&device, &queue, volume_dimensions([3, 3, 2]), || false).unwrap();
    assert!(
        GpuCandidateProjector::new(other)
            .unwrap()
            .render(request, || false)
            .is_err()
    );
}

fn candidate_material(luminance: f32, extinction: f32) -> Arc<VolumeMaterial> {
    Arc::new(
        VolumeMaterial::new(&[-1000., 1000.].map(|value| MaterialPoint {
            value,
            luminance,
            extinction_per_mm: extinction,
        }))
        .unwrap(),
    )
}
fn optical_pixels(image: &CandidateOpaqueImage) -> Vec<[f64; 4]> {
    let source = image.scene().surface.request().surface.source();
    texture_bytes(source, image.texture(), 8)
        .as_chunks::<8>()
        .0
        .iter()
        .map(|bytes| {
            std::array::from_fn(|i| {
                half(u16::from_le_bytes(
                    bytes[i * 2..i * 2 + 2].try_into().unwrap(),
                ))
            })
        })
        .collect()
}
// Independent homogeneous Beer-Lambert reference, with f64 ray/triangle
// intersection and source-cell field normals. The generated source is constant.
fn optical_reference(scene: &CandidateOpaqueScene, pixel: [u32; 2]) -> Option<f64> {
    let request = scene.surface.request();
    let source = request.surface.source().volume();
    let frame = scene
        .volume
        .camera
        .frame(
            &scene.navigation,
            f64::from(request.size[0]) / f64::from(request.size[1]),
        )
        .unwrap();
    let (world, direction) = frame.ray(std::array::from_fn(|a| {
        (f64::from(pixel[a]) + 0.5) / f64::from(request.size[a])
    }));
    let hit = reference(request, pixel, 0., 0.);
    if hit.as_ref().is_some_and(|hit| hit.margin <= 0.005) {
        return None;
    }
    if hit.is_none() && reference(request, pixel, 0.005, 0.001).is_some() {
        return None;
    }
    let mut interval = source.clipped_ray_interval(world, direction, scene.volume.clip.as_ref());
    if let (Some(range), Some(hit)) = (&mut interval, &hit) {
        let point = source.voxel_to_world(hit.voxel);
        range[1] = range[1].min(dot(sub(point, world), direction));
    }
    let mut length = 0.;
    if let Some(range) = interval
        && range[0] < range[1]
    {
        let voxel_origin = source.world_to_voxel(world);
        let voxel_direction = source.world_vector_to_voxel(direction);
        let mut breaks = vec![range[0], range[1]];
        for axis in 0..3 {
            if voxel_direction[axis] != 0. {
                for plane in 0..source.dimensions()[axis] {
                    let t = (plane as f64 - voxel_origin[axis]) / voxel_direction[axis];
                    if t > range[0] && t < range[1] {
                        breaks.push(t);
                    }
                }
            }
        }
        breaks.sort_by(f64::total_cmp);
        breaks.dedup();
        for interval in breaks.windows(2) {
            let midpoint = (interval[0] + interval[1]) * 0.5;
            if source
                .sample(std::array::from_fn(|a| world[a] + direction[a] * midpoint))
                .is_some()
            {
                length += interval[1] - interval[0];
            }
        }
    }
    let optics = scene.material.sample(10.).unwrap();
    let transmission = (-optics.extinction_per_mm * length).exp();
    let background = if let Some(hit) = hit {
        let normal = reference_normal(request, &hit)?;
        f64::from(scene.surface_luminance)
            * (f64::from(scene.surface_lighting.ambient)
                + f64::from(scene.surface_lighting.diffuse) * dot(normal, direction).abs())
    } else {
        0.0030352698
    };
    Some(optics.luminance * (1. - transmission) + transmission * background)
}
fn assert_optical(image: &CandidateOpaqueImage) -> (usize, usize) {
    let scene = image.scene();
    let size = scene.surface.request().size;
    let mut checked = 0;
    let mut hits = 0;
    let mut misses = 0;
    for (i, pixel) in optical_pixels(image).iter().enumerate() {
        let position = [i as u32 % size[0], i as u32 / size[0]];
        let Some(expected) = optical_reference(scene, position) else {
            continue;
        };
        checked += 1;
        if reference(scene.surface.request(), position, 0., 0.).is_some() {
            hits += 1;
        } else {
            misses += 1;
        }
        assert!(
            pixel[..3].iter().all(|v| (*v - expected).abs() < 0.002),
            "pixel {position:?}: {pixel:?} vs {expected}"
        );
        assert_eq!(pixel[3], 1.);
    }
    assert!(checked > 100 && hits > 10 && misses > 10);
    (hits, misses)
}

#[test]
fn candidate_composition_orders_source_and_partial_surface_against_physical_reference() {
    let (device, queue) = device();
    let cpu = volume_geometry([3, 3, 2], 1_000_000., true, true);
    let source = GpuVolumeSource::new(&device, &queue, cpu.clone(), || false).unwrap();
    let (surface, _) = GpuCandidateSurface::upload(
        source.clone(),
        geometry(cpu.clone(), 1000., 2),
        None,
        CandidateUploadLimits::default(),
        || false,
    )
    .unwrap();
    let mut volume = VolumeView::fit(&cpu);
    volume.projection = Projection::Composite;
    volume.camera.yaw_degrees = 27.;
    volume.camera.pitch_degrees = -17.;
    let navigation = ViewState {
        pan: [0.021, -0.017],
        ..Default::default()
    };
    let projector = GpuCandidateProjector::new(source.clone()).unwrap();
    let candidate = projector
        .render(
            CandidateProjection {
                surface: surface.clone(),
                camera: volume.camera.clone(),
                display: navigation.clone(),
                clip: None,
                size: [37, 33],
            },
            || false,
        )
        .unwrap();
    let renderer =
        CandidateOpaqueRenderer::new(source.clone(), wgpu::TextureFormat::Rgba8Unorm).unwrap();
    let mut scene = CandidateOpaqueScene {
        surface: candidate,
        navigation,
        volume,
        material: candidate_material(0.2, 0.4),
        lighting: Lighting::UNLIT,
        surface_lighting: Lighting::HEADLIGHT,
        surface_luminance: 0.85,
    };
    for (sigma, lighting) in [
        (0., Lighting::UNLIT),
        (0.4, Lighting::HEADLIGHT),
        (2., Lighting::UNLIT),
    ] {
        scene.material = candidate_material(0.2, sigma);
        scene.surface_lighting = lighting;
        let image = renderer.render(scene.clone(), || false).unwrap();
        assert_optical(&image);
    }
    let center = cpu.center();
    scene.volume.clip = Some(PhysicalClip {
        minimum_mm: [center[0] - 10., center[1] - 10., center[2] - 10.],
        maximum_mm: [center[0] + 0.25, center[1] + 10., center[2] + 10.],
    });
    scene.surface = projector
        .render(
            CandidateProjection {
                surface,
                camera: scene.volume.camera.clone(),
                display: scene.navigation.clone(),
                clip: scene.volume.clip.clone(),
                size: [37, 33],
            },
            || false,
        )
        .unwrap();
    scene.material = candidate_material(0.2, 0.4);
    scene.surface_lighting = Lighting::HEADLIGHT;
    assert_optical(&renderer.render(scene, || false).unwrap());
}

#[test]
fn candidate_composition_preserves_missing_source_stencils() {
    let (device, queue) = device();
    let cpu = volume_with_missing_sample();
    assert!(cpu.sample(cpu.voxel_to_world([0.5, 0.5, 0.5])).is_none());
    assert!(cpu.sample(cpu.voxel_to_world([1.5, 1.5, 0.5])).is_some());
    let source = GpuVolumeSource::new(&device, &queue, cpu.clone(), || false).unwrap();
    let assembly = geometry(cpu.clone(), 1000., 2);
    assert!(assembly.cells().iter().any(|cell| matches!(
        cell.state,
        CandidateCellState::Graph(orv_core::surface::field::CellGraphState::Unavailable { .. })
    )));
    let (surface, _) = GpuCandidateSurface::upload(
        source.clone(),
        assembly,
        None,
        CandidateUploadLimits::default(),
        || false,
    )
    .unwrap();
    assert!(!surface.chunks().is_empty());
    let mut volume = VolumeView::fit(&cpu);
    volume.projection = Projection::Composite;
    let navigation = ViewState::default();
    let candidate = GpuCandidateProjector::new(source.clone())
        .unwrap()
        .render(
            CandidateProjection {
                surface,
                camera: volume.camera.clone(),
                display: navigation.clone(),
                clip: None,
                size: [37, 33],
            },
            || false,
        )
        .unwrap();
    let scene = CandidateOpaqueScene {
        surface: candidate,
        navigation,
        volume,
        material: candidate_material(0.2, 2.),
        lighting: Lighting::UNLIT,
        surface_lighting: Lighting::UNLIT,
        surface_luminance: 0.85,
    };
    let image = CandidateOpaqueRenderer::new(source, wgpu::TextureFormat::Rgba8Unorm)
        .unwrap()
        .render(scene, || false)
        .unwrap();
    assert_optical(&image);
}

#[test]
fn candidate_composition_refuses_lighting_without_a_valid_normal() {
    let (device, queue) = device();
    let cpu = volume();
    let source = GpuVolumeSource::new(&device, &queue, cpu.clone(), || false).unwrap();
    let (surface, _) = GpuCandidateSurface::upload(
        source.clone(),
        geometry(cpu.clone(), 1000., 2),
        None,
        CandidateUploadLimits::default(),
        || false,
    )
    .unwrap();
    let mut volume = VolumeView::fit(&cpu);
    volume.projection = Projection::Composite;
    let navigation = ViewState::default();
    let candidate = GpuCandidateProjector::new(source.clone())
        .unwrap()
        .render(
            CandidateProjection {
                surface,
                camera: volume.camera.clone(),
                display: navigation.clone(),
                clip: None,
                size: [37, 33],
            },
            || false,
        )
        .unwrap();
    assert!(image_pixels(&candidate).0.iter().any(|cell| cell[3] != 0));
    let mut encoder = device.create_command_encoder(&Default::default());
    let view = candidate.normals().create_view(&Default::default());
    {
        let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("generated missing candidate normals"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
    }
    queue.submit([encoder.finish()]);
    fence(&device);
    let renderer = CandidateOpaqueRenderer::new(source, wgpu::TextureFormat::Rgba8Unorm).unwrap();
    let mut scene = CandidateOpaqueScene {
        surface: candidate,
        navigation,
        volume,
        material: candidate_material(0.2, 0.4),
        lighting: Lighting::UNLIT,
        surface_lighting: Lighting::HEADLIGHT,
        surface_luminance: 0.85,
    };
    assert!(matches!(
        renderer.render(scene.clone(), || false),
        Err("Candidate composition could not resolve this view; no image was published")
    ));
    scene.surface_lighting = Lighting::UNLIT;
    assert_optical(&renderer.render(scene, || false).unwrap());
}

#[test]
fn candidate_composition_rejects_stale_scopes_and_preserves_old_images_on_cancel() {
    let (device, queue) = device();
    let cpu = volume();
    let source = GpuVolumeSource::new(&device, &queue, cpu.clone(), || false).unwrap();
    let (surface, _) = GpuCandidateSurface::upload(
        source.clone(),
        geometry(cpu.clone(), 1000., 2),
        None,
        CandidateUploadLimits::default(),
        || false,
    )
    .unwrap();
    let mut volume = VolumeView::fit(&cpu);
    volume.projection = Projection::Composite;
    let navigation = ViewState::default();
    let image = GpuCandidateProjector::new(source.clone())
        .unwrap()
        .render(
            CandidateProjection {
                surface,
                camera: volume.camera.clone(),
                display: navigation.clone(),
                clip: None,
                size: [31, 35],
            },
            || false,
        )
        .unwrap();
    let scene = CandidateOpaqueScene {
        surface: image,
        navigation,
        volume,
        material: candidate_material(0.2, 0.4),
        lighting: Lighting::UNLIT,
        surface_lighting: Lighting::HEADLIGHT,
        surface_luminance: 0.85,
    };
    let renderer =
        CandidateOpaqueRenderer::new(source.clone(), wgpu::TextureFormat::Rgba8Unorm).unwrap();
    let previous = renderer.render(scene.clone(), || false).unwrap();
    let saved = optical_pixels(&previous);
    let mut wrong = scene.clone();
    wrong.volume.camera.yaw_degrees = 11.;
    assert!(renderer.render(wrong, || false).is_err());
    let mut wrong = scene.clone();
    wrong.navigation.pan[0] += 0.1;
    assert!(renderer.render(wrong, || false).is_err());
    let mut wrong = scene.clone();
    wrong.volume.clip = Some(PhysicalClip {
        minimum_mm: [0.; 3],
        maximum_mm: [1.; 3],
    });
    assert!(renderer.render(wrong, || false).is_err());
    let mut wrong = scene.clone();
    wrong.surface_luminance = f32::NAN;
    assert!(renderer.render(wrong, || false).is_err());
    assert!(renderer.render(scene.clone(), || true).is_err());
    let gpu_before = MemoryBudget::gpu().used();
    let cpu_before = MemoryBudget::volumes().used();
    let entered = AtomicBool::new(false);
    assert!(matches!(
        renderer.render(scene, || {
            let allocated = MemoryBudget::gpu().used() > gpu_before;
            entered.fetch_or(allocated, Ordering::AcqRel);
            allocated
        }),
        Err("Cancelled")
    ));
    assert!(entered.load(Ordering::Acquire));
    fence(&device);
    assert_eq!(MemoryBudget::gpu().used(), gpu_before);
    assert_eq!(MemoryBudget::volumes().used(), cpu_before);
    assert_eq!(optical_pixels(&previous), saved);
    let other =
        GpuVolumeSource::new(&device, &queue, volume_dimensions([3, 3, 2]), || false).unwrap();
    assert!(
        CandidateOpaqueRenderer::new(other, wgpu::TextureFormat::Rgba8Unorm)
            .unwrap()
            .render(previous.scene().clone(), || false)
            .is_err()
    );
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
fn equal_triangle_layout_with_different_field_samples_is_not_reused() {
    let (device, queue) = device();
    let cpu = volume();
    let source = GpuVolumeSource::new(&device, &queue, cpu.clone(), || false).unwrap();
    let first = geometry_scaled(cpu.clone(), 1000., 2, 1);
    let second = geometry_scaled(cpu, 1000., 2, 2);
    let (uploaded, _) = GpuCandidateSurface::upload(
        source,
        first,
        None,
        CandidateUploadLimits::default(),
        || false,
    )
    .unwrap();
    let mut checked = 0;
    for chunk in uploaded.chunks() {
        let cell = second
            .cells()
            .iter()
            .find(|cell| cell.origin == chunk.origin().map(|v| v as usize))
            .unwrap();
        let CandidateCellState::Candidate { mesh } = cell.state else {
            continue;
        };
        let mesh = &second.meshes()[usize::from(mesh)];
        let ChunkGeometry::Mesh(old_mesh) = &chunk.geometry else {
            continue;
        };
        if same_triangle_layout(old_mesh, mesh) {
            assert_ne!(
                field_samples(old_mesh).unwrap(),
                field_samples(mesh).unwrap()
            );
            assert!(
                compatible(Some(&uploaded), chunk.origin(), &InputGeometry::Mesh(mesh)).is_none()
            );
            checked += 1;
        }
    }
    assert!(
        checked > 0,
        "the fixture needs equal triangle layouts with different fields"
    );
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
