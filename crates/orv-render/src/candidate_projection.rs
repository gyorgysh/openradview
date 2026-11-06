// SPDX-License-Identifier: GPL-3.0-or-later
//! Source-centred, partial triangle projection. It does not authorize edits.
use crate::{
    candidate_source::{ChunkGeometry, GpuCandidateSurface, MAX_TRIANGLES, TRIANGLE_BYTES},
    volume::{allocation, flush_upload},
    volume_source::GpuVolumeSource,
};
use orv_core::{
    ViewState,
    mask::{MaskKey, MaskRevision},
    resources::{MemoryBudget, Reservation},
    surface::field::{ConformingFacetState, ConformingSurfaceMesh},
    volume::Volume,
    volume_view::{Camera, PhysicalClip},
};
use std::sync::Arc;

pub const MAX_SIDE: u32 = 2048;
pub const MAX_BATCH_TRIANGLES: u32 = 131_072;
pub const MAX_FRAME_CANDIDATE_PIXELS: u64 = 256 * 1024 * 1024;
pub const MAX_BATCH_CANDIDATE_PIXELS: u64 = 8 * 1024 * 1024;
const UNIFORM_BYTES: u64 = 72 * 4;

#[derive(Clone)]
pub struct CandidateProjection {
    pub surface: Arc<GpuCandidateSurface>,
    pub camera: Camera,
    pub display: ViewState,
    pub clip: Option<PhysicalClip>,
    pub size: [u32; 2],
}
/// Nearest represented triangle only. Gaps, unlisted cells, incomplete faces,
/// source composition and authoritative picking are outside this image.
pub struct CandidateImage {
    request: CandidateProjection,
    cells: wgpu::Texture,
    depth: wgpu::Texture,
    normals: wgpu::Texture,
    reference_mm: [f64; 3],
    direction_unit: [f64; 3],
    vertex_error_upper_mm: f64,
    statistics: CandidateProjectionStatistics,
    _data: allocation::GpuReservation,
    _metadata: Reservation,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CandidateProjectionStatistics {
    pub triangles: usize,
    /// Conservative sum of guarded triangle rectangle areas in pixels.
    pub candidate_pixels: u64,
    pub max_triangle_pixels: u32,
    pub draws: usize,
    pub fenced_batches: usize,
}
impl CandidateImage {
    pub fn request(&self) -> &CandidateProjection {
        &self.request
    }
    /// RGBA32Uint: source-cell x/y/z and selected ID; all zero means no hit.
    pub fn cells(&self) -> &wgpu::Texture {
        &self.cells
    }
    /// R32Float signed physical distance along direction from the source centre.
    pub fn depth(&self) -> &wgpu::Texture {
        &self.depth
    }
    /// RGBA16Float outward field normal; alpha zero means no usable normal.
    pub fn normals(&self) -> &wgpu::Texture {
        &self.normals
    }
    pub fn depth_reference_mm(&self) -> [f64; 3] {
        self.reference_mm
    }
    pub fn direction_unit(&self) -> [f64; 3] {
        self.direction_unit
    }
    /// Conservative physical L1 error for reconstructed vertex positions relative
    /// to the source centre. It excludes world-origin rounding, view projection,
    /// rasterization, depth interpolation, clipping and source-surface error.
    pub fn vertex_error_upper_mm(&self) -> f64 {
        self.vertex_error_upper_mm
    }
    pub fn key(&self) -> &MaskKey {
        self.request.surface.assembly().field().key()
    }
    pub fn revision(&self) -> &MaskRevision {
        self.request.surface.assembly().field().revision()
    }
    pub fn statistics(&self) -> CandidateProjectionStatistics {
        self.statistics
    }
}

pub struct GpuCandidateProjector {
    source: Arc<GpuVolumeSource>,
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    _scratch: allocation::GpuReservation,
}
impl GpuCandidateProjector {
    /// Worker-only pipeline creation; no storage or compute feature is needed.
    pub fn new(source: Arc<GpuVolumeSource>) -> Result<Self, &'static str> {
        let device = source.device();
        let limits = device.limits();
        if limits.max_vertex_buffers < 1
            || limits.max_vertex_attributes < 5
            || u64::from(limits.max_vertex_buffer_array_stride) < TRIANGLE_BYTES
            || limits.max_inter_stage_shader_variables < 5
            || limits.max_color_attachments < 3
            || limits.max_color_attachment_bytes_per_sample < 28
            || limits.max_uniform_buffers_per_shader_stage < 1
            || limits.max_uniform_buffer_binding_size < UNIFORM_BYTES
            || limits.max_buffer_size < UNIFORM_BYTES
            || limits.max_bind_groups < 1
            || limits.max_bindings_per_bind_group < 1
        {
            return Err("This GPU cannot bind candidate projection resources");
        }
        let scratch = allocation::reserve_bytes(65536, device, source.queue())?;
        let memory = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("source-centred candidate projection"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(UNIFORM_BYTES),
                },
                count: None,
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("source-centred candidate triangles"),
            source: wgpu::ShaderSource::Wgsl(
                format!(
                    "const CANDIDATE_DENOMINATOR:f32 = {}.0;\n{}\n{}",
                    ConformingSurfaceMesh::POSITION_DENOMINATOR,
                    include_str!("pixel_ray.wgsl"),
                    include_str!("candidate_projection.wgsl")
                )
                .into(),
            ),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("nearest represented candidate"),
            layout: Some(&pipeline_layout),
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
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Uint32x3,
                            offset: 24,
                            shader_location: 2,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Uint32x3,
                            offset: 36,
                            shader_location: 3,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Uint32x4,
                            offset: 48,
                            shader_location: 4,
                        },
                    ],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[
                    Some(target(wgpu::TextureFormat::Rgba32Uint)),
                    Some(target(wgpu::TextureFormat::R32Float)),
                    Some(target(wgpu::TextureFormat::Rgba16Float)),
                ],
            }),
            primitive: wgpu::PrimitiveState {
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        let invalid = pollster::block_on(validation.pop()).is_some();
        let exhausted = pollster::block_on(memory.pop()).is_some();
        if invalid || exhausted {
            return Err("The graphics device could not prepare candidate projection");
        }
        Ok(Self {
            source,
            pipeline,
            layout,
            _scratch: scratch,
        })
    }
    /// Worker-only complete immutable image, with bounded fenced draw batches.
    pub fn render(
        &self,
        request: CandidateProjection,
        cancelled: impl Fn() -> bool,
    ) -> Result<Arc<CandidateImage>, &'static str> {
        self.render_with_limit(request, MAX_FRAME_CANDIDATE_PIXELS, cancelled)
    }
    pub(crate) fn render_with_limit(
        &self,
        request: CandidateProjection,
        work_limit: u64,
        cancelled: impl Fn() -> bool,
    ) -> Result<Arc<CandidateImage>, &'static str> {
        self.render_with_budgets(request, work_limit, MAX_BATCH_CANDIDATE_PIXELS, cancelled)
    }
    pub(crate) fn render_with_budgets(
        &self,
        request: CandidateProjection,
        work_limit: u64,
        batch_limit: u64,
        cancelled: impl Fn() -> bool,
    ) -> Result<Arc<CandidateImage>, &'static str> {
        if !Arc::ptr_eq(request.surface.source(), &self.source) {
            return Err("Candidate projection and GPU sources differ");
        }
        if cancelled() {
            return Err("Cancelled");
        }
        let values = values(&request)?;
        let device = self.source.device();
        let queue = self.source.queue();
        if request
            .size
            .iter()
            .any(|v| *v > device.limits().max_texture_dimension_2d)
        {
            return Err("Candidate image exceeds this GPU's texture limit");
        }
        let plan = plan_work(
            &request,
            &values,
            work_limit.min(MAX_FRAME_CANDIDATE_PIXELS),
            batch_limit.min(MAX_BATCH_CANDIDATE_PIXELS),
            &cancelled,
        )?;
        let [width, height] = request.size;
        let pixels = u64::from(width) * u64::from(height);
        let data = allocation::reserve_bytes(pixels * 28 + 65536, device, queue)?;
        let _temporary =
            allocation::reserve_bytes(pixels * 4 + 65536 + UNIFORM_BYTES, device, queue)?;
        let metadata = MemoryBudget::volumes()
            .reserve(
                (std::mem::size_of::<CandidateImage>() + 2 * std::mem::size_of::<usize>()) as u64,
            )
            .ok_or("Not enough memory for candidate image identity")?;
        let make = |format, usage| {
            allocation::texture(
                device,
                &wgpu::TextureDescriptor {
                    label: Some("immutable candidate image"),
                    size: wgpu::Extent3d {
                        width,
                        height,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage,
                    view_formats: &[],
                },
            )
        };
        let usage = wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC;
        let cells = make(wgpu::TextureFormat::Rgba32Uint, usage)?;
        let depth = make(wgpu::TextureFormat::R32Float, usage)?;
        let normals = make(wgpu::TextureFormat::Rgba16Float, usage)?;
        let hardware_depth = make(
            wgpu::TextureFormat::Depth32Float,
            wgpu::TextureUsages::RENDER_ATTACHMENT,
        )?;
        let cell_view = cells.create_view(&Default::default());
        let depth_view = depth.create_view(&Default::default());
        let normal_view = normals.create_view(&Default::default());
        let hardware_view = hardware_depth.create_view(&Default::default());
        let uniform = allocation::buffer(
            device,
            &wgpu::BufferDescriptor {
                label: Some("source-centred candidate parameters"),
                size: UNIFORM_BYTES,
                usage: wgpu::BufferUsages::UNIFORM,
                mapped_at_creation: true,
            },
        )?;
        {
            let mut mapped = uniform
                .get_mapped_range_mut(..)
                .map_err(|_| "Could not map candidate projection parameters")?;
            mapped.copy_from_slice(bytemuck::cast_slice(&values.uniform));
        }
        uniform.unmap();
        let bindings = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            }],
        });
        let mut chunks = request.surface.chunks().iter();
        let mut current = chunks.next().map(|chunk| (chunk, 0u32));
        let mut first = true;
        let mut statistics = CandidateProjectionStatistics {
            candidate_pixels: plan.total,
            max_triangle_pixels: plan.max_triangle_pixels,
            ..Default::default()
        };
        let mut cost_index = 0usize;
        while first || current.is_some() {
            if cancelled() {
                return Err("Cancelled");
            }
            let mut encoder = device.create_command_encoder(&Default::default());
            {
                let attachment = |view| {
                    Some(wgpu::RenderPassColorAttachment {
                        view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: if first {
                                wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT)
                            } else {
                                wgpu::LoadOp::Load
                            },
                            store: wgpu::StoreOp::Store,
                        },
                    })
                };
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("bounded candidate triangle batch"),
                    color_attachments: &[
                        attachment(&cell_view),
                        attachment(&depth_view),
                        attachment(&normal_view),
                    ],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &hardware_view,
                        depth_ops: Some(wgpu::Operations {
                            load: if first {
                                wgpu::LoadOp::Clear(1.)
                            } else {
                                wgpu::LoadOp::Load
                            },
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: None,
                    }),
                    ..Default::default()
                });
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &bindings, &[]);
                let mut remaining = MAX_BATCH_TRIANGLES;
                let mut batch_pixels = 0u64;
                while remaining > 0
                    && let Some((chunk, start)) = current
                {
                    if cancelled() {
                        return Err("Cancelled");
                    }
                    let count = u32::try_from(chunk.triangle_count())
                        .map_err(|_| "Candidate triangle count exceeds GPU bounds")?;
                    let mut end = start;
                    while end < count && remaining > 0 {
                        let cost = u64::from(
                            *plan
                                .costs
                                .get(cost_index)
                                .ok_or("Candidate work plan is incomplete")?,
                        );
                        if batch_pixels + cost > batch_limit.min(MAX_BATCH_CANDIDATE_PIXELS) {
                            break;
                        }
                        batch_pixels += cost;
                        remaining -= 1;
                        cost_index += 1;
                        end += 1;
                    }
                    if end == start {
                        break;
                    }
                    pass.set_vertex_buffer(0, chunk.buffer().slice(..));
                    pass.draw(0..6, start..end);
                    statistics.triangles += (end - start) as usize;
                    statistics.draws += 1;
                    current = if end == count {
                        chunks.next().map(|chunk| (chunk, 0))
                    } else {
                        Some((chunk, end))
                    };
                }
            }
            queue.submit([encoder.finish()]);
            flush_upload(device, queue, &cancelled)?;
            statistics.fenced_batches += 1;
            first = false;
        }
        if cancelled() {
            return Err("Cancelled");
        }
        if cost_index != plan.costs.len() {
            return Err("Candidate work plan does not match its buffers");
        }
        Ok(Arc::new(CandidateImage {
            request,
            cells,
            depth,
            normals,
            reference_mm: values.reference_mm,
            direction_unit: values.direction_unit,
            vertex_error_upper_mm: values.vertex_error_upper_mm,
            statistics,
            _data: data,
            _metadata: metadata,
        }))
    }
}

fn target(format: wgpu::TextureFormat) -> wgpu::ColorTargetState {
    wgpu::ColorTargetState {
        format,
        blend: None,
        write_mask: wgpu::ColorWrites::ALL,
    }
}
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a.into_iter().zip(b).map(|(a, b)| a * b).sum()
}
struct WorkPlan {
    costs: Vec<u32>,
    total: u64,
    max_triangle_pixels: u32,
    _reservation: Reservation,
}
fn plan_work(
    request: &CandidateProjection,
    values: &Values,
    limit: u64,
    batch_limit: u64,
    cancelled: &impl Fn() -> bool,
) -> Result<WorkPlan, &'static str> {
    let count = request
        .surface
        .chunks()
        .iter()
        .try_fold(0usize, |total, chunk| {
            total.checked_add(chunk.triangle_count())
        })
        .filter(|count| *count <= MAX_TRIANGLES)
        .ok_or("Candidate work plan exceeds its triangle limit")?;
    let bytes = count
        .checked_mul(std::mem::size_of::<u32>())
        .and_then(|n| n.checked_add(std::mem::size_of::<WorkPlan>()))
        .and_then(|n| u64::try_from(n).ok())
        .ok_or("Candidate work plan size overflow")?;
    let reservation = MemoryBudget::volumes()
        .reserve(bytes)
        .ok_or("Not enough memory for candidate work planning")?;
    let mut costs = Vec::new();
    costs
        .try_reserve_exact(count)
        .map_err(|_| "Not enough memory for candidate work planning")?;
    if costs.capacity() != count {
        return Err("Candidate work plan allocation exceeds its budget");
    }
    let mut total = 0u64;
    let mut max_triangle_pixels = 0u32;
    let denominator = f64::from(ConformingSurfaceMesh::POSITION_DENOMINATOR);
    for chunk in request.surface.chunks() {
        if cancelled() {
            return Err("Cancelled");
        }
        let origin = chunk.origin().map(|v| v as usize);
        let mut found = 0usize;
        let mut add = |vertices: [[u32; 3]; 3]| -> Result<(), &'static str> {
            let mut minimum = [f64::INFINITY; 2];
            let mut maximum = [f64::NEG_INFINITY; 2];
            for local in vertices {
                let q: [f64; 3] = std::array::from_fn(|a| {
                    origin[a] as f64 - f64::from(values.uniform[28 + a])
                        + f64::from(local[a]) / denominator
                });
                for row in 0..2 {
                    let offset = row * 4;
                    let ndc = f64::from(values.uniform[offset + 3])
                        + (0..3)
                            .map(|a| f64::from(values.uniform[offset + a]) * q[a])
                            .sum::<f64>();
                    let pixel = (ndc * 0.5 + 0.5) * f64::from(request.size[row]);
                    minimum[row] = minimum[row].min(pixel);
                    maximum[row] = maximum[row].max(pixel);
                }
            }
            // The shader expands by one pixel; three also cover admitted
            // subpixel projection error and fixed-point raster coverage.
            let span: [u64; 2] = std::array::from_fn(|axis| {
                let edge = f64::from(request.size[axis]);
                let low = (minimum[axis] - 3.).floor().clamp(0., edge);
                let high = (maximum[axis] + 3.).ceil().clamp(0., edge);
                (high - low).max(0.) as u64
            });
            let cost = span[0]
                .checked_mul(span[1])
                .ok_or("Candidate pixel-work overflow")?
                .max(1);
            if cost > batch_limit {
                return Err("One candidate exceeds the bounded GPU batch");
            }
            total = total
                .checked_add(cost)
                .ok_or("Candidate pixel-work overflow")?;
            if total > limit {
                return Err("Candidate projection exceeds its bounded pixel-work limit");
            }
            let cost = u32::try_from(cost).map_err(|_| "Candidate pixel-work overflow")?;
            max_triangle_pixels = max_triangle_pixels.max(cost);
            costs.push(cost);
            found += 1;
            Ok(())
        };
        match chunk.geometry() {
            ChunkGeometry::Mesh(mesh) => {
                for (index, triangle) in mesh.triangles().iter().enumerate() {
                    if index.is_multiple_of(256) && cancelled() {
                        return Err("Cancelled");
                    }
                    if !matches!(triangle.state, ConformingFacetState::WithinTolerance { .. }) {
                        continue;
                    }
                    let mut vertices = [[0; 3]; 3];
                    for (target, vertex) in vertices.iter_mut().zip(triangle.vertices) {
                        *target = mesh
                            .vertices()
                            .get(vertex as usize)
                            .and_then(|v| v.position())
                            .ok_or("Candidate vertex is unavailable")?;
                    }
                    add(vertices)?;
                }
            }
            ChunkGeometry::Affine(geometry) => {
                for vertices in geometry.triangles() {
                    if cancelled() {
                        return Err("Cancelled");
                    }
                    add(vertices)?;
                }
            }
        }
        if found != chunk.triangle_count() {
            return Err("Candidate work plan does not match its buffer");
        }
    }
    if costs.len() != count {
        return Err("Candidate work plan does not match its buffers");
    }
    Ok(WorkPlan {
        costs,
        total,
        max_triangle_pixels,
        _reservation: reservation,
    })
}
struct Values {
    uniform: [f32; 72],
    reference_mm: [f64; 3],
    direction_unit: [f64; 3],
    vertex_error_upper_mm: f64,
}
fn values(request: &CandidateProjection) -> Result<Values, &'static str> {
    if request.size.iter().any(|s| !(1..=MAX_SIDE).contains(s)) {
        return Err("Candidate image exceeds its supported bounds");
    }
    if let Some(clip) = &request.clip {
        clip.validate()?;
    }
    let source = request.surface.source().volume();
    let dimensions = source.dimensions();
    if dimensions.iter().any(|d| *d > (1 << 23)) {
        return Err("Candidate source exceeds GPU coordinate precision");
    }
    let reference_mm = source.center();
    let frame = crate::physical_camera::centred_frame(
        &request.camera,
        &request.display,
        request.size,
        reference_mm,
    )?;
    let basis: [[f64; 3]; 3] = std::array::from_fn(|axis| {
        let mut unit = [0.; 3];
        unit[axis] = 1.;
        source.voxel_vector_to_world(unit)
    });
    let rr = dot(frame.right_mm, frame.right_mm);
    let dd = dot(frame.down_mm, frame.down_mm);
    if !rr.is_finite() || !dd.is_finite() || rr <= 0. || dd <= 0. {
        return Err("Candidate camera span cannot be represented");
    }
    let depth_coeff = basis.map(|b| dot(b, frame.direction_unit));
    let radius = depth_coeff
        .into_iter()
        .zip(dimensions)
        .map(|(a, d)| a.abs() * d as f64 * 0.5)
        .sum::<f64>();
    let mut minimum_spacing = f64::INFINITY;
    for a in 0..3 {
        let inverse: [f64; 3] = std::array::from_fn(|i| {
            let mut unit = [0.; 3];
            unit[i] = 1.;
            source.world_vector_to_voxel(unit)[a]
        });
        minimum_spacing = minimum_spacing.min(1. / dot(inverse, inverse).sqrt());
    }
    if !radius.is_finite() || radius <= 0. || 2.02 * radius / minimum_spacing > 1048576. {
        return Err("Candidate depth range exceeds supported GPU precision");
    }
    let delta = frame.origin_mm.map(|v| -v);
    let mut all = [0f64; 72];
    for a in 0..3 {
        all[a] = 2. * dot(basis[a], frame.right_mm) / rr;
        all[4 + a] = -2. * dot(basis[a], frame.down_mm) / dd;
        all[8 + a] = depth_coeff[a] / (2.02 * radius);
        all[12 + a] = depth_coeff[a];
        for axis in 0..3 {
            all[16 + axis * 4 + a] = basis[a][axis];
        }
        all[28 + a] = (dimensions[a] - 1) as f64 * 0.5;
    }
    all[3] = 2. * dot(delta, frame.right_mm) / rr;
    all[7] = -2. * dot(delta, frame.down_mm) / dd;
    all[11] = 0.5;
    if let Some(clip) = &request.clip {
        for a in 0..3 {
            all[32 + a] = clip.minimum_mm[a] - reference_mm[a];
            all[36 + a] = clip.maximum_mm[a] - reference_mm[a];
        }
        if (0..3).any(|a| all[32 + a] as f32 >= all[36 + a] as f32) {
            return Err("Candidate clip exceeds GPU coordinate precision");
        }
        all[41] = 1.;
    }
    all[40] = f64::from(request.surface.assembly().field().key().number);
    let origin = source.world_vector_to_voxel(frame.origin_mm);
    let right = source.world_vector_to_voxel(frame.right_mm);
    let down = source.world_vector_to_voxel(frame.down_mm);
    let direction = source.world_vector_to_voxel(frame.direction_unit);
    for (offset, vector) in [(44, origin), (48, right), (52, down), (56, direction)] {
        all[offset..offset + 3].copy_from_slice(&vector);
    }
    all[47] = 1. / f64::from(request.size[0]);
    all[51] = 1. / f64::from(request.size[1]);
    all[55] = f64::from(request.size[0]);
    all[59] = f64::from(request.size[1]);
    for world in 0..3 {
        let mut unit = [0.; 3];
        unit[world] = 1.;
        let inverse = source.world_vector_to_voxel(unit);
        all[60 + world * 4..63 + world * 4].copy_from_slice(&inverse);
    }
    let uniform = all.map(|v| v as f32);
    if uniform.iter().any(|v| !v.is_finite() || v.abs() > 1e20) {
        return Err("Candidate parameters exceed GPU arithmetic limits");
    }
    for row in 0..2 {
        let error = screen_error_pixels(&all, &uniform, dimensions, request.size, row);
        if !error.is_finite() || error > 0.5 {
            return Err("Candidate screen projection exceeds GPU coordinate precision");
        }
    }
    let row = |i: usize| std::array::from_fn::<_, 3, _>(|a| f64::from(uniform[i * 4 + a]));
    let [a, b, c] = [row(0), row(1), row(2)];
    let determinant = a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0])
        + a[2] * (b[0] * c[1] - b[1] * c[0]);
    if determinant == 0. || !determinant.is_finite() {
        return Err("Candidate projection collapses at GPU precision");
    }
    let vertex_error_upper_mm = vertex_error_upper(source, basis)?;
    Ok(Values {
        uniform,
        reference_mm,
        direction_unit: frame.direction_unit,
        vertex_error_upper_mm,
    })
}

// Bounds vertex projection error relative to the f64 source-centred rows. The
// raster guard is one pixel, so refuse a view if half that guard is insufficient.
fn screen_error_pixels(
    exact: &[f64; 72],
    rounded: &[f32; 72],
    dimensions: [usize; 3],
    size: [u32; 2],
    row: usize,
) -> f64 {
    let epsilon = f64::from(f32::EPSILON);
    let offset = row * 4;
    let mut error = (exact[offset + 3] - f64::from(rounded[offset + 3])).abs();
    let mut magnitude = f64::from(rounded[offset + 3]).abs();
    for (axis, dimension) in dimensions.into_iter().enumerate() {
        let extent = dimension as f64 * 0.5 + 1.;
        let local_error = 2f64.powi(-24) + epsilon * (extent + 1.);
        let coefficient = f64::from(rounded[offset + axis]);
        error +=
            coefficient.abs() * local_error + (exact[offset + axis] - coefficient).abs() * extent;
        magnitude += coefficient.abs() * (extent + local_error);
    }
    (error + 32. * epsilon * magnitude + 64. * f64::from(f32::MIN_POSITIVE))
        * (1. + 64. * f64::EPSILON)
        * f64::from(size[row])
        * 0.5
}

/// Bound integer-to-f32 conversion, source-centred addition, rounded basis and
/// the world-space dot product; it is not a bound for rasterization or topology.
fn vertex_error_upper(source: &Volume, basis: [[f64; 3]; 3]) -> Result<f64, &'static str> {
    let epsilon = f64::from(f32::EPSILON);
    let mut bound = 0.;
    for world in [0usize, 1, 2] {
        let mut component = 0.;
        let mut magnitude = 0.;
        for (axis, vector) in basis.iter().enumerate() {
            let limit = source.dimensions()[axis] as f64 * 0.5 + 1.;
            let q_error = 2f64.powi(-24) + epsilon * (limit + 1.);
            let rounded = f64::from(vector[world] as f32);
            component += rounded.abs() * q_error + (vector[world] - rounded).abs() * limit;
            magnitude += rounded.abs() * (limit + q_error);
        }
        bound += component + 32. * epsilon * magnitude + 64. * f64::from(f32::MIN_POSITIVE);
    }
    bound *= 1. + 64. * f64::EPSILON;
    if !bound.is_finite() {
        return Err("Candidate vertex precision cannot be represented");
    }
    Ok(bound)
}
