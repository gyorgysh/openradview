// SPDX-License-Identifier: GPL-3.0-or-later
//! Bounded local probes, tied to a completed scene and independent mask membership.
use crate::{
    material_ranges::GpuMaterialRanges,
    material_volume::Lighting,
    opaque_surface::{self, OpaqueSurfaceImage},
    volume::{allocation, flush_upload},
    volume_source::GpuVolumeSource,
};
use orv_core::{
    ViewState,
    mask::MaskOwner,
    resources::{MemoryBudget, Reservation},
    surface::{FaceBoundary, InterfaceHit, InterfaceRay, InterfaceRayError},
    volume_material::VolumeMaterial,
    volume_view::VolumeView,
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};

/// View rule for local picking, not a confidence or medical measurement.
pub const MIN_PICK_TRANSMITTANCE: f32 = 0.05;
const READBACK_BYTES: u64 = 56;

/// A caller renews this opaque RAM-only scope when a source/session/tool boundary
/// changes. Capture it with the originating image job; never retag an old front.
pub struct PickScope {
    _memory: Reservation,
    active: AtomicBool,
}
impl PickScope {
    pub fn new() -> Result<Arc<Self>, &'static str> {
        let memory = MemoryBudget::volumes()
            .reserve((std::mem::size_of::<Self>() + 2 * std::mem::size_of::<usize>()) as u64)
            .ok_or("Not enough memory for the local view scope")?;
        Ok(Arc::new(Self {
            _memory: memory,
            active: AtomicBool::new(true),
        }))
    }
    pub fn revoke(&self) {
        self.active.store(false, Ordering::Release);
    }
    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }
}
#[derive(Clone)]
pub struct PickFrame {
    image: Arc<OpaqueSurfaceImage>,
    scope: Arc<PickScope>,
}
impl PickFrame {
    /// Use the scope that originated this completed image, including when an old
    /// coherent front is still displayed during camera motion or preparation.
    pub fn new(image: Arc<OpaqueSurfaceImage>, scope: Arc<PickScope>) -> Self {
        Self { image, scope }
    }
    pub fn image(&self) -> &Arc<OpaqueSurfaceImage> {
        &self.image
    }
    pub fn scope(&self) -> &Arc<PickScope> {
        &self.scope
    }
}
/// Current desired state can exist before a new GPU image or upload is ready.
/// A worker captures it; the UI must also revalidate the result before use.
#[derive(Clone)]
pub struct PickState {
    pub scope: Arc<PickScope>,
    pub map: MaskOwner,
    pub number: u16,
    pub navigation: ViewState,
    pub volume: VolumeView,
    pub material: Arc<VolumeMaterial>,
    pub lighting: Lighting,
    pub surface_lighting: Lighting,
    pub surface_luminance: f32,
    pub size: [u32; 2],
}
impl PickState {
    /// Snapshot an already-current frame. For pending view/data changes, build
    /// this from live desired state; an old front cannot prove it is current.
    pub fn from_frame(frame: &PickFrame) -> Self {
        let scene = frame.image.scene();
        let request = scene.surface.request();
        let cpu = request.surface.surface();
        Self {
            scope: frame.scope.clone(),
            map: cpu.owner().clone(),
            number: cpu.key().number,
            navigation: scene.navigation.clone(),
            volume: scene.volume.clone(),
            material: scene.material.clone(),
            lighting: scene.lighting,
            surface_lighting: scene.surface_lighting,
            surface_luminance: scene.surface_luminance,
            size: request.size,
        }
    }
    pub fn matches(&self, frame: &PickFrame) -> bool {
        let Ok(view) = self.map.view(self.number) else {
            return false;
        };
        let scene = frame.image.scene();
        let request = scene.surface.request();
        self.scope.is_active()
            && frame.scope.is_active()
            && Arc::ptr_eq(&self.scope, &frame.scope)
            && std::ptr::eq(view.source(), request.surface.source().volume().as_ref())
            && view.key() == *scene.surface.key()
            && view.revision() == scene.surface.revision()
            && self.navigation == scene.navigation
            && self.volume == scene.volume
            && Arc::ptr_eq(&self.material, &scene.material)
            && self.lighting == scene.lighting
            && self.surface_lighting == scene.surface_lighting
            && self.surface_luminance == scene.surface_luminance
            && self.size == request.size
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SurfaceProbeKind {
    Hit,
    Empty,
    Obscured,
    Ambiguous,
}
/// Local geometry and display-visibility readout. This grants no mutation or
/// disclosure permission; supplied masks remain read-only. Revalidate is_current
/// against the live source/tool/view before displaying or using any outcome.
pub struct SurfaceProbe {
    frame: PickFrame,
    pixel: [u32; 2],
    kind: SurfaceProbeKind,
    hit: Option<InterfaceHit>,
    transmittance: f32,
    _metadata: Reservation,
}
impl SurfaceProbe {
    pub fn kind(&self) -> SurfaceProbeKind {
        self.kind
    }
    pub fn hit(&self) -> Option<&InterfaceHit> {
        self.hit.as_ref()
    }
    pub fn pixel(&self) -> [u32; 2] {
        self.pixel
    }
    pub fn foreground_transmittance(&self) -> f32 {
        self.transmittance
    }
    pub fn is_current(&self, current: &PickState, pixel: [u32; 2]) -> bool {
        pixel == self.pixel && current.matches(&self.frame)
    }
}
pub struct GpuSurfacePicker {
    source: Arc<GpuVolumeSource>,
    pipeline: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
    ranges: Option<Arc<GpuMaterialRanges>>,
    _scratch: allocation::GpuReservation,
}
impl GpuSurfacePicker {
    /// Worker-only. A separate picker leaves surface display available on a
    /// device that cannot allocate/bind this optional probe pipeline.
    pub fn new(source: Arc<GpuVolumeSource>) -> Result<Self, &'static str> {
        Self::prepare(source, None)
    }
    /// Match the classification retained by the completed accelerated image.
    /// Scope/current-view checks still apply; acceleration grants no authority.
    pub fn new_accelerated(ranges: Arc<GpuMaterialRanges>) -> Result<Self, &'static str> {
        Self::prepare(ranges.source().clone(), Some(ranges))
    }
    fn prepare(
        source: Arc<GpuVolumeSource>,
        ranges: Option<Arc<GpuMaterialRanges>>,
    ) -> Result<Self, &'static str> {
        let ranges = ranges.filter(|r| r.transparent_tiles() > 0);
        let device = source.device();
        let limits = device.limits();
        crate::volume::check_limits(&limits)?;
        if source.volume().is_color()
            || limits.max_storage_buffers_per_shader_stage < 1
            || limits.max_storage_buffer_binding_size < 4
            || limits.max_sampled_textures_per_shader_stage < 4
            || limits.max_uniform_buffers_per_shader_stage < 2
            || limits.max_uniform_buffer_binding_size < 1344
            || limits.max_buffer_size < 1344
            || limits.max_bindings_per_bind_group < 8
            || limits.max_bind_groups < 1
        {
            return Err("This GPU cannot bind the local surface probe");
        }
        if ranges.is_some()
            && (limits.max_bind_groups < 2
                || limits.max_sampled_textures_per_shader_stage < 5
                || limits.max_uniform_buffers_per_shader_stage < 3)
        {
            return Err("This GPU cannot bind accelerated local probe resources");
        }
        let scratch = allocation::reserve_bytes(65536, device, source.queue())?;
        let memory = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let layout = opaque_surface::composition_layout(device, wgpu::TextureFormat::Rgba32Float);
        let mut layouts = vec![Some(&layout)];
        if let Some(ranges) = &ranges {
            layouts.push(Some(ranges.layout()));
        }
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("local surface probe layout"),
            bind_group_layouts: &layouts,
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("local surface probe"),
            source: wgpu::ShaderSource::Wgsl(
                opaque_surface::shader(include_str!("surface_pick.wgsl"), ranges.is_some()).into(),
            ),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("checked one-pixel surface probe"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        let invalid = pollster::block_on(validation.pop()).is_some();
        let exhausted = pollster::block_on(memory.pop()).is_some();
        if invalid || exhausted {
            return Err("The graphics device could not prepare the local surface probe");
        }
        Ok(Self {
            source,
            pipeline,
            layout,
            ranges,
            _scratch: scratch,
        })
    }
    /// Synchronous only on a worker. Reads 56 bytes for one selected pixel, uses
    /// the original viewport ray, and returns only after GPU/f64 checks finish.
    pub fn probe(
        &self,
        frame: &PickFrame,
        current: &PickState,
        pixel: [u32; 2],
        cancelled: impl Fn() -> bool,
    ) -> Result<SurfaceProbe, &'static str> {
        let obsolete = || cancelled() || !current.scope.is_active();
        if obsolete() {
            return Err("Cancelled");
        }
        if !current.matches(frame) {
            return Err("The completed surface is no longer the current view");
        }
        let scene = frame.image.scene();
        let request = scene.surface.request();
        if !Arc::ptr_eq(request.surface.source(), &self.source) {
            return Err("The local probe and GPU source differ");
        }
        if !match (&self.ranges, frame.image.material_ranges()) {
            (None, None) => true,
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            _ => false,
        } {
            return Err("The local probe and completed image acceleration differ");
        }
        if (0..2).any(|a| pixel[a] >= request.size[a]) {
            return Err("The local probe pixel is outside the completed image");
        }
        let metadata = MemoryBudget::volumes()
            .reserve(std::mem::size_of::<SurfaceProbe>() as u64 + READBACK_BYTES)
            .ok_or("Not enough memory for the local surface probe")?;
        let bytes = self.read(frame, pixel, &obsolete)?;
        if u32_at(&bytes, 20)? != 0 {
            return Err("Surface probe arithmetic could not resolve this pixel");
        }
        let cells = [
            u32_at(&bytes, 0)?,
            u32_at(&bytes, 4)?,
            u32_at(&bytes, 8)?,
            u32_at(&bytes, 12)?,
        ];
        let depth = f32_at(&bytes, 16)?;
        let radiance = f32_at(&bytes, 32)?;
        let transmittance = f32_at(&bytes, 36)?;
        if !depth.is_finite()
            || !radiance.is_finite()
            || !(0. ..=1.00001).contains(&radiance)
            || !transmittance.is_finite()
            || !(0. ..=1.).contains(&transmittance)
        {
            return Err("The local surface probe returned invalid values");
        }
        for a in 0..4 {
            let offset = 48 + a * 2;
            let value = half(u16::from_le_bytes([bytes[offset], bytes[offset + 1]]))
                .ok_or("The completed surface pixel is invalid")?;
            let expected = if a == 3 { 1. } else { radiance };
            if (value - expected).abs() > 0.0006 {
                return Err("The local probe and completed surface pixel differ");
            }
        }
        let rays = request.camera.frame(
            &request.display,
            f64::from(request.size[0]) / f64::from(request.size[1]),
        )?;
        let (origin, direction) = rays.ray(std::array::from_fn(|a| {
            (f64::from(pixel[a]) + 0.5) / f64::from(request.size[a])
        }));
        let view = current.map.view(current.number)?;
        let geometry = InterfaceRay::new(
            view,
            origin,
            direction,
            [-1e30, 1e30],
            request.clip.as_ref(),
        )
        .map_err(ray_error)?
        .first_hit(obsolete);
        let (kind, hit) = match geometry {
            Err(InterfaceRayError::Ambiguous) => (SurfaceProbeKind::Ambiguous, None),
            Err(error) => return Err(ray_error(error)),
            Ok(None) if cells == [0; 4] => (SurfaceProbeKind::Empty, None),
            Ok(Some(hit))
                if matches_hit(
                    &hit,
                    cells,
                    depth,
                    scene.surface.depth_reference_mm(),
                    direction,
                    view.source(),
                ) =>
            {
                if transmittance < MIN_PICK_TRANSMITTANCE {
                    (SurfaceProbeKind::Obscured, None)
                } else {
                    (SurfaceProbeKind::Hit, Some(hit))
                }
            }
            _ => (SurfaceProbeKind::Ambiguous, None),
        };
        if obsolete() || !current.matches(frame) {
            return Err("Cancelled");
        }
        Ok(SurfaceProbe {
            frame: frame.clone(),
            pixel,
            kind,
            hit,
            transmittance,
            _metadata: metadata,
        })
    }
    fn read(
        &self,
        frame: &PickFrame,
        pixel: [u32; 2],
        cancelled: &impl Fn() -> bool,
    ) -> Result<[u8; 56], &'static str> {
        let device = self.source.device();
        let queue = self.source.queue();
        let scene = frame.image.scene();
        let _gpu =
            allocation::reserve_bytes(65536 + 1344 + 16 + 4 + READBACK_BYTES, device, queue)?;
        let parameters = opaque_surface::parameters(&self.source, scene, [pixel, [1; 2]])?;
        let uniform = allocation::initialized_buffer(
            device,
            "immutable probe parameters",
            bytemuck::cast_slice(&parameters),
            wgpu::BufferUsages::UNIFORM,
        )?;
        let surface = allocation::initialized_buffer(
            device,
            "immutable probe surface material",
            bytemuck::cast_slice(&[
                scene.surface_luminance,
                f32::from(scene.surface.key().number),
                scene.surface_lighting.ambient,
                scene.surface_lighting.diffuse,
            ]),
            wgpu::BufferUsages::UNIFORM,
        )?;
        let status = allocation::initialized_buffer(
            device,
            "local probe validation flag",
            &[0; 4],
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        )?;
        let target = allocation::texture(
            device,
            &wgpu::TextureDescriptor {
                label: Some("one-pixel local probe"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba32Float,
                usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            },
        )?;
        let output = target.create_view(&Default::default());
        let cells = scene.surface.cells().create_view(&Default::default());
        let depth = scene.surface.depth().create_view(&Default::default());
        let bindings = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("local surface probe resources"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&self.source.source),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&output),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&self.source.validity),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(&cells),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::TextureView(&depth),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: status.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: surface.as_entire_binding(),
                },
            ],
        });
        let buffer = allocation::buffer(
            device,
            &wgpu::BufferDescriptor {
                label: Some("bounded local surface probe readback"),
                size: READBACK_BYTES,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            },
        )?;
        if cancelled() {
            return Err("Cancelled");
        }
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bindings, &[]);
            if let Some(ranges) = &self.ranges {
                pass.set_bind_group(1, ranges.bindings(), &[]);
            }
            pass.dispatch_workgroups(1, 1, 1);
        }
        for (texture, origin, offset) in [
            (scene.surface.cells(), pixel, 0),
            (scene.surface.depth(), pixel, 16),
            (&target, [0; 2], 32),
            (frame.image.texture(), pixel, 48),
        ] {
            let mut copy = texture.as_image_copy();
            copy.origin = wgpu::Origin3d {
                x: origin[0],
                y: origin[1],
                z: 0,
            };
            encoder.copy_texture_to_buffer(
                copy,
                wgpu::TexelCopyBufferInfo {
                    buffer: &buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset,
                        // The tested Metal path loses packed destination offsets
                        // for a one-row copy with an unspecified stride. Explicit
                        // aligned row pitch preserves all four pixel fields.
                        bytes_per_row: Some(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT),
                        rows_per_image: None,
                    },
                },
                wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
            );
        }
        encoder.copy_buffer_to_buffer(&status, 0, &buffer, 20, 4);
        queue.submit([encoder.finish()]);
        let (tx, rx) = mpsc::sync_channel(1);
        buffer.map_async(wgpu::MapMode::Read, .., move |result| {
            let _ = tx.send(result);
        });
        flush_upload(device, queue, cancelled)?;
        rx.try_recv()
            .map_err(|_| "Local probe readback unavailable")?
            .map_err(|_| "Local probe readback failed")?;
        let bytes = {
            let data = buffer
                .get_mapped_range(..)
                .map_err(|_| "Local probe mapping failed")?;
            <[u8; 56]>::try_from(&data[..]).map_err(|_| "Local probe readback length is invalid")
        };
        buffer.unmap();
        bytes
    }
}
fn ray_error(error: InterfaceRayError) -> &'static str {
    match error {
        InterfaceRayError::Cancelled => "Cancelled",
        InterfaceRayError::Ambiguous => "Ambiguous surface contact",
        InterfaceRayError::Geometry(message) => message,
    }
}
fn matches_hit(
    hit: &InterfaceHit,
    cell: [u32; 4],
    depth: f32,
    reference: [f64; 3],
    direction: [f64; 3],
    source: &orv_core::volume::Volume,
) -> bool {
    let face = hit.face;
    let [x, y, z] = face.cell();
    let word = u32::from(hit.key.number)
        | ((face.side() as u32) << 16)
        | (u32::from(face.boundary() == FaceBoundary::Grid) << 24);
    let expected = (0..3)
        .map(|a| (hit.position_mm[a] - reference[a]) * direction[a])
        .sum::<f64>();
    let speed = source
        .world_vector_to_voxel(direction)
        .into_iter()
        .map(f64::abs)
        .fold(0., f64::max);
    cell == [x, y, z, word] && (f64::from(depth) - expected).abs() * speed <= 0.005
}
fn u32_at(bytes: &[u8; 56], offset: usize) -> Result<u32, &'static str> {
    let word = bytes
        .get(offset..offset + 4)
        .ok_or("Invalid probe word")?
        .try_into()
        .map_err(|_| "Invalid probe word")?;
    Ok(u32::from_le_bytes(word))
}
fn f32_at(bytes: &[u8; 56], offset: usize) -> Result<f32, &'static str> {
    Ok(f32::from_bits(u32_at(bytes, offset)?))
}
fn half(bits: u16) -> Option<f32> {
    let exponent = (bits >> 10) & 31;
    if exponent == 31 {
        return None;
    }
    let fraction = f32::from(bits & 1023);
    let value = if exponent == 0 {
        fraction * 2f32.powi(-24)
    } else {
        (1. + fraction / 1024.) * 2f32.powi(i32::from(exponent) - 15)
    };
    Some(if bits & 32768 == 0 { value } else { -value })
}
