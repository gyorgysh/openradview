// SPDX-License-Identifier: GPL-3.0-or-later
//! Physical optical materials with affine-correct headlight shading and linear output.
use crate::{
    linear_display::LinearDisplay,
    material_ranges::GpuMaterialRanges,
    volume::{VolumeTarget, allocation},
    volume_source::GpuVolumeSource,
};
use orv_core::{
    ViewState,
    volume_material::VolumeMaterial,
    volume_view::{Projection, VolumeView},
};
use std::sync::Arc;
use wgpu::util::DeviceExt;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lighting {
    pub ambient: f32,
    pub diffuse: f32,
}
impl Lighting {
    pub const UNLIT: Self = Self {
        ambient: 1.,
        diffuse: 0.,
    };
    pub const HEADLIGHT: Self = Self {
        ambient: 0.25,
        diffuse: 0.75,
    };
    pub fn validate(self) -> Result<(), &'static str> {
        if !self.ambient.is_finite()
            || !self.diffuse.is_finite()
            || self.ambient < 0.
            || self.diffuse < 0.
            || self.ambient + self.diffuse > 1.
        {
            return Err("Lighting needs nonnegative ambient/diffuse weights with sum at most one");
        }
        Ok(())
    }
}
struct Output {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    bindings: wgpu::BindGroup,
    _reservation: allocation::GpuReservation,
}

/// This native derived style is separate from serialized source display scenes.
/// Materials specify linear luminance, physical attenuation and two-sided diffuse
/// lighting. Existing grayscale windows, VOI tables and inversion are not baked in.
pub struct MaterialVolumeRenderer {
    source: Arc<GpuVolumeSource>,
    compute: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
    display: LinearDisplay,
    outputs: [Option<Output>; 2],
    ranges: Option<Arc<GpuMaterialRanges>>,
    use_range_bindings: bool,
    _scratch: allocation::GpuReservation,
}
impl MaterialVolumeRenderer {
    pub fn new(
        source: Arc<GpuVolumeSource>,
        format: wgpu::TextureFormat,
    ) -> Result<Self, &'static str> {
        Self::prepare(source, format, false, None)
    }
    /// Dense reference using the same shared source stencil as acceleration.
    /// It retains the original interpolation and is not a pick/export API.
    pub fn new_dense_reference(
        source: Arc<GpuVolumeSource>,
        format: wgpu::TextureFormat,
    ) -> Result<Self, &'static str> {
        Self::prepare(source, format, true, None)
    }
    /// Skip only certified zero-extinction samples of the source interpolator.
    /// Sample count, positions, validity, lighting and requested size are retained.
    /// The caller retains this renderer through command submission, like `new`.
    pub fn new_accelerated(
        ranges: Arc<GpuMaterialRanges>,
        format: wgpu::TextureFormat,
    ) -> Result<Self, &'static str> {
        Self::prepare(ranges.source().clone(), format, true, Some(ranges))
    }
    fn prepare(
        source: Arc<GpuVolumeSource>,
        format: wgpu::TextureFormat,
        shared_stencil: bool,
        ranges: Option<Arc<GpuMaterialRanges>>,
    ) -> Result<Self, &'static str> {
        let device = source.device();
        crate::volume::check_limits(&device.limits())?;
        if source.volume.is_color() {
            return Err("Optical scalar materials do not accept color sources");
        }
        let use_range_bindings = ranges.as_ref().is_some_and(|r| r.transparent_tiles() > 0);
        if use_range_bindings
            && (device.limits().max_bind_groups < 2
                || device.limits().max_sampled_textures_per_shader_stage < 3
                || device.limits().max_uniform_buffers_per_shader_stage < 2)
        {
            return Err("This GPU cannot bind scalar range acceleration resources");
        }
        let scratch = allocation::reserve_bytes(65536, device, source.queue())?;
        let memory = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("physical material volume bindings"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D3,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(1344),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: wgpu::TextureFormat::Rgba16Float,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Uint,
                        view_dimension: wgpu::TextureViewDimension::D3,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let mut layouts = vec![Some(&layout)];
        if use_range_bindings && let Some(ranges) = &ranges {
            layouts.push(Some(ranges.layout()));
        }
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("physical material volume layout"),
            bind_group_layouts: &layouts,
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("physical optical volume"),
            source: wgpu::ShaderSource::Wgsl(
                format!(
                    "{}\n{}\n{}",
                    crate::VOLUME_SAMPLING_SHADER,
                    optics_shader(shared_stencil, use_range_bindings),
                    include_str!("material_volume.wgsl")
                )
                .into(),
            ),
        });
        let compute = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("physical optical integration"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        let display = LinearDisplay::new(device, format);
        let invalid = pollster::block_on(validation.pop()).is_some();
        let exhausted = pollster::block_on(memory.pop()).is_some();
        if invalid || exhausted {
            return Err("The graphics device could not prepare optical volume rendering");
        }
        Ok(Self {
            source,
            compute,
            layout,
            display,
            outputs: Default::default(),
            ranges,
            use_range_bindings,
            _scratch: scratch,
        })
    }
    /// Bounded native scheduling must submit before dropping this renderer and
    /// retain the previous completed target while a tiled target is incomplete.
    pub fn render_target(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        navigation: &ViewState,
        state: &VolumeView,
        material: &VolumeMaterial,
        lighting: Lighting,
        target: VolumeTarget,
    ) -> Result<(), &'static str> {
        let VolumeTarget { slot, size, region } = target;
        if slot >= self.outputs.len() {
            return Err("Unsupported material volume output slot");
        }
        if self
            .ranges
            .as_ref()
            .is_some_and(|ranges| ranges.material().as_ref() != material)
        {
            return Err("Material range classification no longer matches the requested material");
        }
        let parameters = parameters(
            &self.source,
            navigation,
            state,
            material,
            lighting,
            size,
            region,
        )?;
        let device = self.source.device();
        if size
            .iter()
            .any(|n| *n > device.limits().max_texture_dimension_2d)
        {
            return Err("Material output exceeds this GPU's texture limit");
        }
        if self.outputs[slot]
            .as_ref()
            .is_none_or(|o| [o.texture.width(), o.texture.height()] != size)
        {
            let reservation = allocation::reserve_bytes(
                u64::from(size[0]) * u64::from(size[1]) * 8,
                device,
                self.source.queue(),
            )?;
            let texture = allocation::texture(
                device,
                &wgpu::TextureDescriptor {
                    label: Some("linear material radiance"),
                    size: wgpu::Extent3d {
                        width: size[0],
                        height: size[1],
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba16Float,
                    usage: wgpu::TextureUsages::STORAGE_BINDING
                        | wgpu::TextureUsages::TEXTURE_BINDING
                        | wgpu::TextureUsages::COPY_SRC,
                    view_formats: &[],
                },
            )?;
            let view = texture.create_view(&Default::default());
            let bindings = self.display.bindings(device, &view);
            self.outputs[slot] = Some(Output {
                texture,
                view,
                bindings,
                _reservation: reservation,
            });
        }
        let output = self.outputs[slot]
            .as_ref()
            .ok_or("Material output unavailable")?;
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("immutable material volume parameters"),
            contents: bytemuck::cast_slice(&parameters),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let bindings = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("physical material dispatch"),
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
                    resource: wgpu::BindingResource::TextureView(&output.view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&self.source.validity),
                },
            ],
        });
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("optical volume tile"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.compute);
        pass.set_bind_group(0, &bindings, &[]);
        if self.use_range_bindings
            && let Some(ranges) = &self.ranges
        {
            pass.set_bind_group(1, ranges.bindings(), &[]);
        }
        pass.dispatch_workgroups(region[1][0].div_ceil(8), region[1][1].div_ceil(8), 1);
        Ok(())
    }
    pub fn output_texture(&self, slot: usize) -> Option<&wgpu::Texture> {
        self.outputs.get(slot)?.as_ref().map(|o| &o.texture)
    }
    pub fn paint_target(&self, slot: usize, pass: &mut wgpu::RenderPass<'_>) {
        if let Some(Some(output)) = self.outputs.get(slot) {
            self.display.paint(&output.bindings, pass);
        }
    }
}

pub(crate) fn optics_shader(shared_stencil: bool, ranges: bool) -> String {
    format!(
        "const SHARED_OPTICAL_STENCIL:bool={shared_stencil};\n{}\n{}\n{}\n{}\n{}\n{}",
        include_str!("material_parameters.wgsl"),
        include_str!("material_stencil.wgsl"),
        include_str!("material_mapping.wgsl"),
        include_str!("material_sample.wgsl"),
        if ranges {
            concat!(
                include_str!("material_ranges_dense.wgsl"),
                "\n",
                include_str!("material_ranges.wgsl")
            )
        } else {
            "fn transparent_advance(lower:vec3u,point:vec3f,origin:vec3f,near:f32,step:f32,index:u32,count:u32)->u32{return 0u;}"
        },
        include_str!("material_optics.wgsl")
    )
}

pub(crate) fn parameters(
    source: &GpuVolumeSource,
    navigation: &ViewState,
    state: &VolumeView,
    material: &VolumeMaterial,
    lighting: Lighting,
    size: [u32; 2],
    region: [[u32; 2]; 2],
) -> Result<[f32; 336], &'static str> {
    parameters_for_volume(
        source.volume(),
        navigation,
        state,
        material,
        lighting,
        size,
        region,
    )
}

pub(crate) fn parameters_for_volume(
    volume: &orv_core::volume::Volume,
    navigation: &ViewState,
    state: &VolumeView,
    material: &VolumeMaterial,
    lighting: Lighting,
    size: [u32; 2],
    region: [[u32; 2]; 2],
) -> Result<[f32; 336], &'static str> {
    lighting.validate()?;
    if state.projection != Projection::Composite || navigation.invert {
        return Err("Material rendering needs a volume projection without display inversion");
    }
    if material
        .points()
        .windows(2)
        .any(|pair| f64::from(pair[1].value) - f64::from(pair[0].value) < 1e-20)
    {
        return Err("Material scalar knots are too close for GPU arithmetic");
    }
    let mut values = [0.; 336];
    values[..60].copy_from_slice(&crate::volume_uniform::parameters_with_limit(
        volume, navigation, state, size, region, 2048,
    )?);
    let mut vectors = [[0.; 3]; 4];
    for (axis, vector) in vectors[..3].iter_mut().enumerate() {
        let mut unit = [0.; 3];
        unit[axis] = 1.;
        *vector = volume.world_vector_to_voxel(unit);
    }
    vectors[3] = state
        .camera
        .frame(navigation, f64::from(size[0]) / f64::from(size[1]))?
        .direction_unit;
    for (i, vector) in vectors.into_iter().enumerate() {
        for axis in 0..3 {
            values[60 + i * 4 + axis] = vector[axis] as f32;
        }
    }
    values[76..80].copy_from_slice(&[
        material.points().len() as f32,
        lighting.ambient,
        lighting.diffuse,
        0.,
    ]);
    for (i, p) in material.points().iter().enumerate() {
        values[80 + i * 4..84 + i * 4].copy_from_slice(&[
            p.value,
            p.luminance,
            p.extinction_per_mm,
            0.,
        ]);
    }
    if values.iter().any(|v| !v.is_finite() || v.abs() > 1e20) {
        return Err("Material volume exceeds GPU arithmetic limits");
    }
    Ok(values)
}
