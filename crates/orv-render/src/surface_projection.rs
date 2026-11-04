// SPDX-License-Identifier: GPL-3.0-or-later
//! Complete known-interface images, distinct from closed-volume mask first hits.
mod parameters;
use crate::{
    surface_source::GpuCellSurface,
    volume::{allocation, flush_upload},
    volume_source::GpuVolumeSource,
};
use orv_core::{
    ViewState,
    mask::{MaskKey, MaskRevision},
    resources::{MemoryBudget, Reservation},
    volume_view::{Camera, PhysicalClip},
};
pub(crate) use parameters::centred_frame;
use std::sync::Arc;

pub const MAX_SIDE: u32 = 2048;
pub const MAX_BATCH_FACES: u32 = 131072;
const UNIFORM_BYTES: u64 = 84 * 4;
#[derive(Clone)]
pub struct SurfaceProjection {
    pub surface: Arc<GpuCellSurface>,
    pub camera: Camera,
    pub display: ViewState,
    pub clip: Option<PhysicalClip>,
    pub size: [u32; 2],
}
/// Nearest two-sided, known cell interface. Unknown/missing interfaces remain
/// open; physical clipping removes fragments without adding cut caps.
/// This image includes no source opacity and grants no edit/sharing authority.
pub struct SurfaceImage {
    request: SurfaceProjection,
    cells: wgpu::Texture,
    depth: wgpu::Texture,
    normals: wgpu::Texture,
    depth_reference_mm: [f64; 3],
    direction_unit: [f64; 3],
    statistics: ProjectionStatistics,
    _data: allocation::GpuReservation,
    _metadata: Reservation,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProjectionStatistics {
    pub faces: usize,
    pub draws: usize,
    pub fenced_batches: usize,
}
impl SurfaceImage {
    pub fn request(&self) -> &SurfaceProjection {
        &self.request
    }
    /// RGBA32Uint: (cell x,y,z, ID | side<<16 | grid-truncation<<24).
    /// ID occupies bits0..15, side bits16..18. Zero word means no hit.
    pub fn cells(&self) -> &wgpu::Texture {
        &self.cells
    }
    /// R32Float signed physical distance from the source-centre plane, along
    /// direction_unit. It is NOT the original camera-ray-origin parameter.
    pub fn depth(&self) -> &wgpu::Texture {
        &self.depth
    }
    /// RGBA16Float outward unit normal with alpha1, or all zero for no hit.
    pub fn normals(&self) -> &wgpu::Texture {
        &self.normals
    }
    pub fn depth_reference_mm(&self) -> [f64; 3] {
        self.depth_reference_mm
    }
    pub fn direction_unit(&self) -> [f64; 3] {
        self.direction_unit
    }
    pub fn key(&self) -> &MaskKey {
        self.request.surface.surface().key()
    }
    pub fn revision(&self) -> &MaskRevision {
        self.request.surface.surface().revision()
    }
    pub fn statistics(&self) -> ProjectionStatistics {
        self.statistics
    }
}
pub struct GpuSurfaceProjector {
    source: Arc<GpuVolumeSource>,
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    _scratch: allocation::GpuReservation,
}
impl GpuSurfaceProjector {
    /// Worker-only pipeline creation. No compute/storage-buffer feature is needed.
    pub fn new(source: Arc<GpuVolumeSource>) -> Result<Self, &'static str> {
        let device = source.device();
        let limits = device.limits();
        if limits.max_vertex_buffers < 1
            || limits.max_vertex_attributes < 1
            || limits.max_vertex_buffer_array_stride < 16
            || limits.max_inter_stage_shader_variables < 4
            || limits.max_color_attachments < 3
            || limits.max_color_attachment_bytes_per_sample < 28
            || limits.max_uniform_buffers_per_shader_stage < 1
            || limits.max_uniform_buffer_binding_size < UNIFORM_BYTES
            || limits.max_buffer_size < UNIFORM_BYTES
            || limits.max_bind_groups < 1
            || limits.max_bindings_per_bind_group < 1
        {
            return Err("This GPU cannot bind the cell-boundary projection resources");
        }
        let scratch = allocation::reserve_bytes(65536, device, source.queue())?;
        let memory = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("source-centred cell projection"),
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
            label: Some("known cell-boundary raster"),
            source: wgpu::ShaderSource::Wgsl(
                format!(
                    "{}\n{}",
                    include_str!("pixel_ray.wgsl"),
                    include_str!("surface_projection/raster.wgsl")
                )
                .into(),
            ),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("nearest known cell interface"),
            layout: Some(&pipeline_layout),
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
            return Err("The graphics device could not prepare cell-boundary projection");
        }
        Ok(Self {
            source,
            pipeline,
            layout,
            _scratch: scratch,
        })
    }
    /// Render the exact requested size on a worker, with bounded fenced draw
    /// batches. Only a completely submitted/fenced immutable image can return.
    pub fn render(
        &self,
        request: SurfaceProjection,
        cancelled: impl Fn() -> bool,
    ) -> Result<Arc<SurfaceImage>, &'static str> {
        if !Arc::ptr_eq(request.surface.source(), &self.source) {
            return Err("Surface projection and GPU sources differ");
        }
        if cancelled() {
            return Err("Cancelled");
        }
        let values = parameters::values(&request)?;
        let device = self.source.device();
        let queue = self.source.queue();
        if request
            .size
            .iter()
            .any(|v| *v > device.limits().max_texture_dimension_2d)
        {
            return Err("Surface image exceeds this GPU's texture limit");
        }
        let [width, height] = request.size;
        let pixels = u64::from(width) * u64::from(height);
        let data = allocation::reserve_bytes(pixels * 28 + 65536, device, queue)?;
        let _temporary =
            allocation::reserve_bytes(pixels * 4 + 65536 + UNIFORM_BYTES, device, queue)?;
        let metadata = MemoryBudget::volumes()
            .reserve(
                (std::mem::size_of::<SurfaceImage>() + 2 * std::mem::size_of::<usize>()) as u64,
            )
            .ok_or("Not enough memory for the surface image identity")?;
        let make = |format, usage| {
            allocation::texture(
                device,
                &wgpu::TextureDescriptor {
                    label: Some("immutable known-interface image"),
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
        let image_usage = wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC;
        let cells = make(wgpu::TextureFormat::Rgba32Uint, image_usage)?;
        let depth = make(wgpu::TextureFormat::R32Float, image_usage)?;
        let normals = make(wgpu::TextureFormat::Rgba16Float, image_usage)?;
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
                label: Some("immutable source-centred projection parameters"),
                size: UNIFORM_BYTES,
                usage: wgpu::BufferUsages::UNIFORM,
                mapped_at_creation: true,
            },
        )?;
        {
            let mut mapped = uniform
                .get_mapped_range_mut(..)
                .map_err(|_| "Could not map surface projection parameters")?;
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
        let mut chunks = request.surface.chunks();
        let mut current = chunks.next().map(|c| (c, 0u32));
        let mut first = true;
        let mut statistics = ProjectionStatistics::default();
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
                    label: Some("bounded cell-boundary batch"),
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
                let mut remaining = MAX_BATCH_FACES;
                while remaining > 0
                    && let Some((chunk, start)) = current
                {
                    if cancelled() {
                        return Err("Cancelled");
                    }
                    let end = (start + remaining).min(chunk.cpu_chunk().faces().len() as u32);
                    pass.set_vertex_buffer(0, chunk.buffer().slice(..));
                    pass.draw(0..6, start..end);
                    remaining -= end - start;
                    statistics.faces += (end - start) as usize;
                    statistics.draws += 1;
                    current = if end == chunk.cpu_chunk().faces().len() as u32 {
                        chunks.next().map(|c| (c, 0))
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
        drop(chunks);
        if cancelled() {
            return Err("Cancelled");
        }
        Ok(Arc::new(SurfaceImage {
            request,
            cells,
            depth,
            normals,
            depth_reference_mm: values.reference_mm,
            direction_unit: values.direction_unit,
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

#[cfg(all(test, feature = "gpu-tests"))]
mod tests;
