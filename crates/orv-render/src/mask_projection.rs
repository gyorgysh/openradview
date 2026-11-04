// SPDX-License-Identifier: GPL-3.0-or-later
//! First eligible manual-mask cells and physical ray depth, without image compositing.
mod parameters;
use crate::{label_source::GpuLabels, volume::allocation, volume_source::GpuVolumeSource};
use orv_core::{
    ViewState,
    mask::{MaskKey, MaskRevision},
    volume_view::{Camera, PhysicalClip},
};
use std::{ops::Range, sync::Arc};
use wgpu::util::DeviceExt;

pub const MAX_SIDE: u32 = 2048;
pub const MAX_ROW_DISPATCHES: usize = 256;
/// An arithmetic failure is distinct from an empty ray (ID zero).
pub const RAY_ERROR: u32 = u32::MAX;

#[derive(Clone)]
pub struct MaskProjection {
    pub labels: Arc<GpuLabels>,
    pub number: u8,
    pub camera: Camera,
    pub display: ViewState,
    pub clip: Option<PhysicalClip>,
    pub size: [u32; 2],
}
impl MaskProjection {
    fn same(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.labels, &other.labels)
            && self.number == other.number
            && self.camera == other.camera
            && self.display == other.display
            && self.clip == other.clip
            && self.size == other.size
    }
}
/// A completely encoded immutable image. Submit the encoding commands before
/// reading/presenting it, and retain this Arc through their GPU use.
/// Cells are (x,y,z,selected ID), zero ID means no hit, RAY_ERROR means refusal.
/// Depth is the signed physical parameter from that pixel's camera-ray origin.
/// These are mask-only first hits; source opacity/occlusion is not included.
/// GPU arithmetic is f32; revalidate physical picks against the core f64 mask
/// ray and completed scene before using them to edit source cells.
pub struct MaskImage {
    request: MaskProjection,
    cells: wgpu::Texture,
    depth: wgpu::Texture,
    _reservation: allocation::GpuReservation,
}
impl MaskImage {
    pub fn request(&self) -> &MaskProjection {
        &self.request
    }
    pub fn cells(&self) -> &wgpu::Texture {
        &self.cells
    }
    pub fn depth(&self) -> &wgpu::Texture {
        &self.depth
    }
    pub fn key(&self) -> MaskKey {
        MaskKey {
            object: self.request.labels.labels().object_id().clone(),
            number: u16::from(self.request.number),
        }
    }
    pub fn revision(&self) -> &MaskRevision {
        self.request.labels.labels().revision()
    }
}
struct Pending {
    image: MaskImage,
    cells: wgpu::TextureView,
    depth: wgpu::TextureView,
    next_row: u32,
    dispatches: usize,
}
pub struct GpuMaskProjector {
    source: Arc<GpuVolumeSource>,
    pipeline: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
    pending: [Option<Pending>; 2],
    _scratch: allocation::GpuReservation,
}
impl GpuMaskProjector {
    /// Worker-only pipeline setup on the admitted source's own device/queue.
    pub fn new(source: Arc<GpuVolumeSource>) -> Result<Self, &'static str> {
        let device = source.device();
        crate::volume::check_limits(&device.limits())?;
        let limits = device.limits();
        if limits.max_storage_textures_per_shader_stage < 2
            || limits.max_sampled_textures_per_shader_stage < 2
            || limits.max_uniform_buffers_per_shader_stage < 1
            || limits.max_uniform_buffer_binding_size < 192
            || limits.max_buffer_size < 192
            || limits.max_bindings_per_bind_group < 5
            || limits.max_bind_groups < 1
        {
            return Err("This GPU cannot bind the mask projection resources");
        }
        let scratch = allocation::reserve_bytes(65536, device, source.queue())?;
        let sampled = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Uint,
                view_dimension: wgpu::TextureViewDimension::D3,
                multisampled: false,
            },
            count: None,
        };
        let output = |binding, format| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::StorageTexture {
                access: wgpu::StorageTextureAccess::WriteOnly,
                format,
                view_dimension: wgpu::TextureViewDimension::D2,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("categorical ray bindings"),
            entries: &[
                sampled(0),
                sampled(1),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(192),
                    },
                    count: None,
                },
                output(3, wgpu::TextureFormat::Rgba32Uint),
                output(4, wgpu::TextureFormat::R32Float),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("categorical ray layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("source-cell mask rays"),
            source: wgpu::ShaderSource::Wgsl(include_str!("mask_projection.wgsl").into()),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("first eligible source cell"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        Ok(Self {
            source,
            pipeline,
            layout,
            pending: Default::default(),
            _scratch: scratch,
        })
    }
    /// Encode contiguous full-width row batches; only the final batch returns
    /// an image. Requests cannot change mid-frame or overwrite published images.
    /// At most MAX_ROW_DISPATCHES batches are admitted for one pending image.
    pub fn encode_rows(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        request: &MaskProjection,
        slot: usize,
        rows: Range<u32>,
    ) -> Result<Option<Arc<MaskImage>>, &'static str> {
        if slot >= self.pending.len() {
            return Err("Unsupported mask projection slot");
        }
        if !Arc::ptr_eq(request.labels.source(), &self.source) {
            return Err("Mask projection and GPU source differ");
        }
        let params = parameters::values(request, rows.clone())?;
        let device = self.source.device();
        if request
            .size
            .iter()
            .any(|n| *n > device.limits().max_texture_dimension_2d)
        {
            return Err("Mask output exceeds this GPU's texture limit");
        }
        if [
            request.size[0].div_ceil(8),
            (rows.end - rows.start).div_ceil(8),
        ]
        .into_iter()
        .any(|n| n > device.limits().max_compute_workgroups_per_dimension)
        {
            return Err("Mask row batch exceeds this GPU's dispatch limit");
        }
        if rows.start == 0 {
            if self.pending[slot].is_some() {
                return Err("Finish or discard the pending mask image first");
            }
            self.pending[slot] = Some(self.allocate(request)?);
        }
        let pending = self.pending[slot]
            .as_mut()
            .ok_or("Start mask projection at its first row")?;
        if !pending.image.request.same(request) || pending.next_row != rows.start {
            return Err("Mask projection rows or request no longer match");
        }
        if pending.dispatches >= MAX_ROW_DISPATCHES {
            return Err("Mask projection has too many row batches");
        }
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("immutable categorical ray parameters"),
            contents: bytemuck::cast_slice(&params),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let bindings = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("categorical ray dispatch"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(request.labels.texture_view()),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&self.source.validity),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&pending.cells),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(&pending.depth),
                },
            ],
        });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("exact categorical row batch"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bindings, &[]);
            pass.dispatch_workgroups(
                request.size[0].div_ceil(8),
                (rows.end - rows.start).div_ceil(8),
                1,
            );
        }
        pending.dispatches += 1;
        pending.next_row = rows.end;
        if rows.end == request.size[1] {
            return self.pending[slot]
                .take()
                .map(|pending| Some(Arc::new(pending.image)))
                .ok_or("Mask projection output unavailable");
        }
        Ok(None)
    }
    fn allocate(&self, request: &MaskProjection) -> Result<Pending, &'static str> {
        let device = self.source.device();
        // Include bounded per-dispatch uniforms with conservative 256-byte alignment.
        let reservation = allocation::reserve_bytes(
            u64::from(request.size[0]) * u64::from(request.size[1]) * 20
                + MAX_ROW_DISPATCHES as u64 * 256
                + 65536,
            device,
            self.source.queue(),
        )?;
        let texture = |label, format| {
            allocation::texture(
                device,
                &wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d {
                        width: request.size[0],
                        height: request.size[1],
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::STORAGE_BINDING
                        | wgpu::TextureUsages::TEXTURE_BINDING
                        | wgpu::TextureUsages::COPY_SRC,
                    view_formats: &[],
                },
            )
        };
        let cells = texture("mask hit cells and IDs", wgpu::TextureFormat::Rgba32Uint)?;
        let depth = texture(
            "mask hit physical ray distance",
            wgpu::TextureFormat::R32Float,
        )?;
        let cells_view = cells.create_view(&Default::default());
        let depth_view = depth.create_view(&Default::default());
        Ok(Pending {
            image: MaskImage {
                request: request.clone(),
                cells,
                depth,
                _reservation: reservation,
            },
            cells: cells_view,
            depth: depth_view,
            next_row: 0,
            dispatches: 0,
        })
    }
    /// Drop an unsubmitted encoder, or submit its commands, before discarding
    /// their pending target so GPU retirement admission follows those commands.
    pub fn discard_pending(&mut self, slot: usize) -> Result<(), &'static str> {
        self.pending
            .get_mut(slot)
            .ok_or("Unsupported mask projection slot")?
            .take();
        Ok(())
    }
}

#[cfg(all(test, feature = "gpu-tests"))]
mod tests;
