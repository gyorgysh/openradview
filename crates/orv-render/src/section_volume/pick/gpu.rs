// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
mod packing;
use packing::Packing;

pub(in super::super) struct Pipeline {
    pipeline: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
    _gpu: allocation::GpuReservation,
}
impl Pipeline {
    pub(super) fn new(source: &GpuVolumeSource) -> Result<Self, &'static str> {
        let device = source.device();
        let gpu = allocation::reserve_bytes(65536, device, source.queue())?;
        let memory = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let layout = layout(device, wgpu::TextureFormat::Rgba32Float);
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("local source section probe layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("local source section probe"),
            source: wgpu::ShaderSource::Wgsl(
                shader(include_str!("../../section_volume_pick.wgsl")).into(),
            ),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("checked source section probe"),
            layout: Some(&pipeline_layout),
            module: &module,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        let invalid = pollster::block_on(validation.pop()).is_some();
        let exhausted = pollster::block_on(memory.pop()).is_some();
        if invalid || exhausted {
            return Err("The GPU could not prepare the section probe");
        }
        Ok(Self {
            pipeline,
            layout,
            _gpu: gpu,
        })
    }
    pub(super) fn read(
        &self,
        renderer: &SectionRenderer,
        frame: &SectionPickFrame,
        pixel: [u32; 2],
        cancelled: &impl Fn() -> bool,
    ) -> Result<[u8; 56], &'static str> {
        self.read_region(
            renderer,
            frame,
            [pixel, [1; 2]],
            cancelled,
            |data, packing| packing.packet(data, [0; 2]),
        )
    }
    pub(super) fn read_region<T>(
        &self,
        renderer: &SectionRenderer,
        frame: &SectionPickFrame,
        region: [[u32; 2]; 2],
        cancelled: &impl Fn() -> bool,
        consume: impl FnOnce(&[u8], &Packing) -> Result<T, &'static str>,
    ) -> Result<T, &'static str> {
        let source = &renderer.source;
        let device = source.device();
        let queue = source.queue();
        let scene = frame.image.scene();
        let surface = scene
            .surface
            .as_ref()
            .ok_or("No region in the completed scene")?;
        let packing = Packing::new(region[1])?;
        let _cpu = MemoryBudget::volumes()
            .reserve(packing.bytes)
            .ok_or("Not enough memory for section probe staging")?;
        let _gpu = allocation::reserve_bytes(
            65536
                + 1344
                + 576
                + 32
                + 4
                + u64::from(region[1][0]) * u64::from(region[1][1]) * 16
                + packing.bytes,
            device,
            queue,
        )?;
        let uniform = |label, values: &[f32]| {
            allocation::initialized_buffer(
                device,
                label,
                bytemuck::cast_slice(values),
                wgpu::BufferUsages::UNIFORM,
            )
        };
        let optics = uniform(
            "immutable section probe optics",
            &parameters(source, scene, region)?,
        )?;
        let sections = uniform(
            "immutable section probe planes",
            &plane_parameters(source, scene)?,
        )?;
        let boundary = uniform(
            "immutable section probe boundary",
            &surface_parameters(source, scene)?,
        )?;
        let status = allocation::initialized_buffer(
            device,
            "section probe arithmetic flag",
            &[0; 4],
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        )?;
        let target = allocation::texture(
            device,
            &wgpu::TextureDescriptor {
                label: Some("bounded local section probe"),
                size: wgpu::Extent3d {
                    width: region[1][0],
                    height: region[1][1],
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
        let planes = std::array::from_fn(|i| {
            scene.planes[i]
                .as_ref()
                .map_or(&renderer.empty, |p| p.image.texture())
                .create_view(&Default::default())
        });
        let cells = surface.image.cells().create_view(&Default::default());
        let depth = surface.image.depth().create_view(&Default::default());
        let bindings = bindings(
            device,
            &self.layout,
            source,
            Resources {
                output: &output,
                planes: &planes,
                cells: &cells,
                depth: &depth,
                status: &status,
                optics: &optics,
                sections: &sections,
                boundary: &boundary,
            },
        );
        let buffer = allocation::buffer(
            device,
            &wgpu::BufferDescriptor {
                label: Some("bounded section probe readback"),
                size: packing.bytes,
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
            pass.dispatch_workgroups(region[1][0].div_ceil(8), region[1][1].div_ceil(8), 1);
        }
        for (field, (texture, origin)) in [
            (surface.image.cells(), region[0]),
            (surface.image.depth(), region[0]),
            (&target, [0; 2]),
            (frame.image.texture(), region[0]),
        ]
        .into_iter()
        .enumerate()
        {
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
                        offset: packing.offsets[field],
                        // Explicit aligned pitch also preserves packed destination
                        // offsets on the tested Metal one-row copy path.
                        bytes_per_row: Some(packing.strides[field]),
                        rows_per_image: None,
                    },
                },
                wgpu::Extent3d {
                    width: region[1][0],
                    height: region[1][1],
                    depth_or_array_layers: 1,
                },
            );
        }
        encoder.copy_buffer_to_buffer(&status, 0, &buffer, packing.status, 4);
        queue.submit([encoder.finish()]);
        let (tx, rx) = mpsc::sync_channel(1);
        buffer.map_async(wgpu::MapMode::Read, .., move |r| {
            let _ = tx.send(r);
        });
        flush_upload(device, queue, cancelled)?;
        rx.try_recv()
            .map_err(|_| "Section probe readback unavailable")?
            .map_err(|_| "Section probe readback failed")?;
        let bytes = {
            let data = buffer
                .get_mapped_range(..)
                .map_err(|_| "Section probe mapping failed")?;
            consume(&data, &packing)
        };
        buffer.unmap();
        bytes
    }
}
