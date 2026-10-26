// SPDX-License-Identifier: GPL-3.0-or-later
use orv_core::{ViewState, study::Frame};
use wgpu::util::DeviceExt;

pub struct SliceRenderer {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    uniform: wgpu::Buffer,
    texture: Option<wgpu::Texture>,
    validity: Option<wgpu::Texture>,
    bindings: Option<wgpu::BindGroup>,
    size: [u32; 2],
    srgb: bool,
    color: bool,
}

impl SliceRenderer {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("slice bindings"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(48),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Uint,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("slice pipeline layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("scalar DICOM display"),
            source: wgpu::ShaderSource::Wgsl(include_str!("slice.wgsl").into()),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("scalar slice"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("slice params"),
            contents: &[0u8; 48],
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        Self {
            pipeline,
            layout,
            uniform,
            texture: None,
            validity: None,
            bindings: None,
            size: [0, 0],
            srgb: format.is_srgb(),
            color: false,
        }
    }

    pub fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        frame: &Frame,
    ) -> Result<(), &'static str> {
        let limit = device.limits().max_texture_dimension_2d;
        if frame.width == 0
            || frame.height == 0
            || frame.width > limit
            || frame.height > limit
            || !frame.valid_dimensions()
        {
            return Err("Image dimensions exceed GPU limits");
        }
        let color = frame.rgba.is_some();
        if self.size != [frame.width, frame.height] || self.color != color {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("modality pixels"),
                size: wgpu::Extent3d {
                    width: frame.width,
                    height: frame.height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: if color {
                    wgpu::TextureFormat::Rgba8UnormSrgb
                } else {
                    wgpu::TextureFormat::R32Float
                },
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let view = texture.create_view(&Default::default());
            let validity = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("scalar validity mask"),
                size: wgpu::Extent3d {
                    width: frame.width,
                    height: frame.height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::R8Uint,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let mask_view = validity.create_view(&Default::default());
            self.bindings = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("slice image"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: self.uniform.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(&mask_view),
                    },
                ],
            }));
            self.texture = Some(texture);
            self.validity = Some(validity);
            self.size = [frame.width, frame.height];
            self.color = color;
        }
        if let Some(texture) = &self.texture {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                frame
                    .rgba
                    .as_deref()
                    .unwrap_or_else(|| bytemuck::cast_slice(&frame.pixels)),
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(frame.width * 4),
                    rows_per_image: Some(frame.height),
                },
                wgpu::Extent3d {
                    width: frame.width,
                    height: frame.height,
                    depth_or_array_layers: 1,
                },
            );
        }
        if let Some(texture) = &self.validity {
            let mask = if color {
                frame
                    .rgba
                    .as_ref()
                    .map(|rgba| {
                        rgba.as_chunks::<4>()
                            .0
                            .iter()
                            .map(|p| u8::from(p[3] == 255))
                            .collect()
                    })
                    .unwrap_or_default()
            } else {
                frame
                    .pixels
                    .iter()
                    .enumerate()
                    .map(|(i, _)| u8::from(frame.has_sample(i)))
                    .collect::<Vec<_>>()
            };
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &mask,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(frame.width),
                    rows_per_image: Some(frame.height),
                },
                wgpu::Extent3d {
                    width: frame.width,
                    height: frame.height,
                    depth_or_array_layers: 1,
                },
            );
        }
        Ok(())
    }

    pub fn set_view(
        &self,
        queue: &wgpu::Queue,
        frame: &Frame,
        view: &ViewState,
        viewport_aspect: f32,
    ) -> Result<(), &'static str> {
        view.validate()?;
        if !viewport_aspect.is_finite()
            || viewport_aspect <= 0.0
            || !frame.aspect_ratio().is_finite()
            || frame.aspect_ratio() <= 0.0
        {
            return Err("Invalid image or viewport aspect ratio");
        }
        let invert = view.invert ^ frame.monochrome1;
        let params = [
            view.window.width,
            view.window.center,
            u8::from(invert) as f32,
            view.zoom,
            view.pan[0],
            view.pan[1],
            frame.aspect_ratio(),
            viewport_aspect,
            u8::from(self.srgb) as f32,
            u8::from(frame.rgba.is_some()) as f32,
            0.0,
            0.0,
        ];
        queue.write_buffer(&self.uniform, 0, bytemuck::cast_slice(&params));
        Ok(())
    }

    pub fn paint(&self, pass: &mut wgpu::RenderPass<'_>) {
        if let Some(bindings) = &self.bindings {
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, bindings, &[]);
            pass.draw(0..3, 0..1);
        }
    }
}
