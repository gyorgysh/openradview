// SPDX-License-Identifier: GPL-3.0-or-later
use super::fixed::{FIXED_WORDS, fixed_parameters};
use orv_core::resources::{MemoryBudget, Reservation};

#[derive(Clone, Copy, Debug, Default)]
pub struct FrameReadbackStatistics {
    pub planes: usize,
    pub readback_bytes: u64,
    pub corrected_pixels: usize,
    pub correction_uploaded_bytes: u64,
}
/// Immutable corrected scalar texture and matching CPU samples for local tools.
pub struct GpuReformatImage {
    source: Arc<GpuVolumeSource>,
    grid: ReformatGrid,
    frame: Arc<Frame>,
    output: Output,
    _metadata: Reservation,
}
impl GpuReformatImage {
    pub fn source(&self) -> &Arc<GpuVolumeSource> {
        &self.source
    }
    pub fn grid(&self) -> &ReformatGrid {
        &self.grid
    }
    pub fn frame(&self) -> &Arc<Frame> {
        &self.frame
    }
    pub fn texture(&self) -> &wgpu::Texture {
        &self.output.texture
    }
    pub(crate) fn view(&self) -> &wgpu::TextureView {
        &self.output.view
    }
}
use super::*;
use orv_core::{study::Frame, volume::ScalarReformatBuffer};
use std::sync::mpsc;

pub(super) struct FramePipeline {
    pipeline: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
    _gpu: allocation::GpuReservation,
}
impl FramePipeline {
    fn new(renderer: &GpuReslicer, cancelled: &impl Fn() -> bool) -> Result<Self, &'static str> {
        let source = &renderer.source;
        let d = source.device();
        let q = source.queue();
        if d.limits().max_bind_groups < 2
            || d.limits().max_uniform_buffers_per_shader_stage < 2
            || d.limits().max_uniform_buffer_binding_size < (FIXED_WORDS * 4) as u64
        {
            return Err("This GPU cannot bind precise scalar reformat coordinates");
        }
        for frame in source.volume().frames() {
            for block in frame.pixels.chunks(16384) {
                if cancelled() {
                    return Err("Cancelled");
                }
                if block.iter().any(|v| v.is_subnormal()) {
                    return Err("Subnormal source values require CPU resampling");
                }
            }
        }
        let gpu = allocation::reserve_bytes(65536, d, q)?;
        let memory = d.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let validation = d.push_error_scope(wgpu::ErrorFilter::Validation);
        let layout = d.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("precise scalar grid coordinates"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::COMPUTE,
                count: None,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new((FIXED_WORDS * 4) as u64),
                },
            }],
        });
        let shader = d.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("fixed-point scalar frame sampling"),
            source: wgpu::ShaderSource::Wgsl(
                format!(
                    "{}\n{}",
                    include_str!("../reslice.wgsl"),
                    include_str!("fixed.wgsl")
                )
                .into(),
            ),
        });
        let pipeline_layout = d.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&renderer.layout), Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = d.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("precise complete scalar frames"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("frame"),
            compilation_options: Default::default(),
            cache: None,
        });
        let validation_error = pollster::block_on(validation.pop());
        #[cfg(test)]
        if let Some(error) = &validation_error {
            eprintln!("{error}");
        }
        let invalid = validation_error.is_some();
        let exhausted = pollster::block_on(memory.pop()).is_some();
        if invalid || exhausted {
            return Err("The GPU could not prepare precise scalar frames");
        }
        Ok(Self {
            pipeline,
            layout,
            _gpu: gpu,
        })
    }
}

pub(super) struct Readback {
    buffer: wgpu::Buffer,
    bytes: u64,
    _gpu: allocation::GpuReservation,
}
impl GpuReslicer {
    /// Worker-only, complete scalar grids as immutable Frames. One readback and
    /// cancellable fence cover the requested planes; absent slots do no work.
    /// Returned pixels retain their admission independently of this renderer.
    pub fn read_frames(
        &mut self,
        grids: [Option<&ReformatGrid>; 3],
        cancelled: impl Fn() -> bool,
    ) -> Result<[Option<Arc<Frame>>; 3], &'static str> {
        self.render_frames(grids, cancelled)
            .map(|images| images.map(|image| image.map(|image| image.frame.clone())))
    }
    pub fn frame_statistics(&self) -> FrameReadbackStatistics {
        self.frame_statistics
    }
    /// Worker-only. No target escapes before its readback, required CPU
    /// corrections and correction uploads finish. Later jobs allocate new targets.
    pub fn render_frames(
        &mut self,
        grids: [Option<&ReformatGrid>; 3],
        cancelled: impl Fn() -> bool,
    ) -> Result<[Option<Arc<GpuReformatImage>>; 3], &'static str> {
        if cancelled() {
            return Err("Cancelled");
        }
        let source = self.source.clone();
        let d = source.device();
        let q = source.queue();
        let mut staging: [Option<ScalarReformatBuffer>; 3] = std::array::from_fn(|_| None);
        let mut offsets = [0u64; 3];
        let mut strides = [0u32; 3];
        let mut bytes = 0u64;
        let mut fixed = std::array::from_fn::<_, 3, _>(|_| None);
        let mut metadata: [Option<Reservation>; 3] = std::array::from_fn(|_| None);
        for (slot, grid) in grids.iter().enumerate() {
            let Some(grid) = grid else { continue };
            if cancelled() {
                return Err("Cancelled");
            }
            // Validate owner, arithmetic and extent before allocating outputs or
            // encoding any work into a batch that might later be refused.
            parameters(&source, grid, [[0, 0], grid.dimensions()])?;
            fixed[slot] = Some(fixed_parameters(&source, grid)?);
            metadata[slot] = Some(
                MemoryBudget::volumes()
                    .reserve(
                        (std::mem::size_of::<GpuReformatImage>()
                            + 128
                            + grid
                                .geometry()
                                .frame_of_reference
                                .as_ref()
                                .map_or(0, String::len)) as u64,
                    )
                    .ok_or("Not enough memory for scalar image ownership")?,
            );
            staging[slot] = Some(source.volume().scalar_reformat_buffer(grid).map_err(
                |e| match e {
                    orv_core::spatial::GeometryError::Memory => {
                        "Not enough memory for scalar reformat pixels"
                    }
                    _ => "Scalar reformat requires its exact source and grid",
                },
            )?);
            offsets[slot] = bytes;
            strides[slot] =
                (grid.dimensions()[0] * 16).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
            bytes += u64::from(strides[slot]) * u64::from(grid.dimensions()[1]);
        }
        if bytes == 0 {
            self.frame_statistics = Default::default();
            return Ok(std::array::from_fn(|_| None));
        }
        if bytes > d.limits().max_buffer_size {
            return Err("Scalar reformat readback exceeds this GPU's buffer limit");
        }
        if self.frame_pipeline.is_none() {
            self.frame_pipeline = Some(FramePipeline::new(self, &cancelled)?);
        }
        if self.readback.as_ref().is_none_or(|v| v.bytes < bytes) {
            let gpu = allocation::reserve_bytes(bytes, d, q)?;
            let buffer = allocation::buffer(
                d,
                &wgpu::BufferDescriptor {
                    label: Some("complete scalar plane readback"),
                    size: bytes,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                },
            )?;
            self.readback = Some(Readback {
                buffer,
                bytes,
                _gpu: gpu,
            });
        }
        let mut encoder = d.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("complete scalar reformat frames"),
        });
        for (slot, grid) in grids.iter().enumerate() {
            if let Some(grid) = grid {
                if cancelled() {
                    return Err("Cancelled");
                }
                self.outputs[slot].take();
                self.encode_frame(
                    &mut encoder,
                    grid,
                    slot,
                    fixed[slot]
                        .as_ref()
                        .ok_or("Precise coordinates unavailable")?,
                )?;
                let texture = self
                    .output_texture(slot)
                    .ok_or("Scalar reformat target unavailable")?;
                encoder.copy_texture_to_buffer(
                    texture.as_image_copy(),
                    wgpu::TexelCopyBufferInfo {
                        buffer: &self
                            .readback
                            .as_ref()
                            .ok_or("Scalar readback unavailable")?
                            .buffer,
                        layout: wgpu::TexelCopyBufferLayout {
                            offset: offsets[slot],
                            bytes_per_row: Some(strides[slot]),
                            rows_per_image: Some(grid.dimensions()[1]),
                        },
                    },
                    texture.size(),
                );
            }
        }
        if cancelled() {
            return Err("Cancelled");
        }
        q.submit([encoder.finish()]);
        let readback = self
            .readback
            .as_ref()
            .ok_or("Scalar readback unavailable")?;
        let (tx, rx) = mpsc::sync_channel(1);
        readback
            .buffer
            .map_async(wgpu::MapMode::Read, ..bytes, move |r| {
                let _ = tx.send(r);
            });
        let mut statistics = FrameReadbackStatistics {
            planes: grids.iter().flatten().count(),
            readback_bytes: bytes,
            ..Default::default()
        };
        let max_width = grids
            .iter()
            .flatten()
            .map(|g| g.dimensions()[0])
            .max()
            .unwrap_or(0) as usize;
        let mut correction_gpu = None;
        let mut correction_cpu = None;
        let mut row = Vec::<[f32; 4]>::new();
        let mut queued = 0u64;
        let result = (|| {
            crate::volume::flush_upload(d, q, &cancelled)?;
            rx.try_recv()
                .map_err(|_| "Scalar readback did not complete")?
                .map_err(|_| "Scalar readback failed")?;
            let mapped = readback
                .buffer
                .get_mapped_range(..bytes)
                .map_err(|_| "Scalar readback mapping failed")?;
            for slot in 0..3 {
                let (Some(grid), Some(target)) = (grids[slot], staging[slot].as_mut()) else {
                    continue;
                };
                let [width, height] = grid.dimensions();
                let pixels = target.pixels_mut();
                for y in 0..height as usize {
                    if cancelled() {
                        return Err("Cancelled");
                    }
                    let base = offsets[slot] as usize + y * strides[slot] as usize;
                    let mut changed = false;
                    for (x, raw) in mapped[base..base + width as usize * 16]
                        .as_chunks::<16>()
                        .0
                        .iter()
                        .enumerate()
                    {
                        let parts = raw.as_chunks::<4>().0;
                        let values: [f32; 4] =
                            std::array::from_fn(|a| f32::from_le_bytes(parts[a]));
                        if !values[0].is_finite()
                            || ![0., 1.].contains(&values[1])
                            || ![0., 1.].contains(&values[2])
                            || values[3] != 1.
                            || (values[1] == 0. && values[0] != 0.)
                        {
                            return Err(
                                "GPU scalar reformat arithmetic is unsupported for this view",
                            );
                        }
                        pixels[y * width as usize + x] = if values[2] == 1. {
                            changed = true;
                            statistics.corrected_pixels += 1;
                            source
                                .volume()
                                .reformat_sample(grid, [x as u32, y as u32], &cancelled)
                                .map_err(|e| {
                                    if e == orv_core::spatial::GeometryError::Cancelled {
                                        "Cancelled"
                                    } else {
                                        "Scalar correction requires its exact grid"
                                    }
                                })?
                                .unwrap_or(f32::NAN)
                        } else if values[1] == 1. {
                            values[0]
                        } else {
                            f32::NAN
                        };
                    }
                    if changed {
                        if correction_gpu.is_none() {
                            correction_gpu = Some(allocation::reserve_bytes(
                                16 * 1024 * 1024 + (max_width * 16).next_multiple_of(256) as u64,
                                d,
                                q,
                            )?);
                            correction_cpu = Some(
                                MemoryBudget::volumes()
                                    .reserve((max_width * 16 + 64) as u64)
                                    .ok_or("Not enough memory for scalar correction staging")?,
                            );
                            row.try_reserve_exact(max_width)
                                .map_err(|_| "Not enough memory for scalar correction staging")?;
                        }
                        row.clear();
                        row.extend(
                            pixels[y * width as usize..(y + 1) * width as usize]
                                .iter()
                                .map(|&v| {
                                    if v.is_finite() {
                                        [v, 1., 0., 1.]
                                    } else {
                                        [0., 0., 0., 1.]
                                    }
                                }),
                        );
                        let texture = self
                            .output_texture(slot)
                            .ok_or("Scalar correction target unavailable")?;
                        let mut target = texture.as_image_copy();
                        target.origin.y = y as u32;
                        q.write_texture(
                            target,
                            bytemuck::cast_slice(&row),
                            wgpu::TexelCopyBufferLayout {
                                offset: 0,
                                bytes_per_row: Some(width * 16),
                                rows_per_image: Some(1),
                            },
                            wgpu::Extent3d {
                                width,
                                height: 1,
                                depth_or_array_layers: 1,
                            },
                        );
                        queued += u64::from(width) * 16;
                        statistics.correction_uploaded_bytes += u64::from(width) * 16;
                        if queued >= 16 * 1024 * 1024 {
                            crate::volume::flush_upload(d, q, &cancelled)?;
                            queued = 0;
                        }
                    }
                }
            }
            Ok(())
        })();
        readback.buffer.unmap();
        result?;
        if queued != 0 {
            crate::volume::flush_upload(d, q, &cancelled)?;
        }
        if cancelled() {
            return Err("Cancelled");
        }
        let mut images = std::array::from_fn(|_| None);
        for slot in 0..3 {
            if let (Some(grid), Some(buffer)) = (grids[slot], staging[slot].take()) {
                images[slot] = Some(Arc::new(GpuReformatImage {
                    source: source.clone(),
                    grid: grid.clone(),
                    frame: buffer.finish(),
                    output: self.outputs[slot]
                        .take()
                        .ok_or("Completed scalar target unavailable")?,
                    _metadata: metadata[slot]
                        .take()
                        .ok_or("Scalar image ownership unavailable")?,
                }));
            }
        }
        self.frame_statistics = statistics;
        drop((correction_gpu, correction_cpu));
        Ok(images)
    }
    fn encode_frame(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        grid: &ReformatGrid,
        slot: usize,
        fixed: &[u32; FIXED_WORDS],
    ) -> Result<(), &'static str> {
        self.ensure_output(grid.dimensions(), slot)?;
        let source = &self.source;
        let d = source.device();
        let pipeline = self
            .frame_pipeline
            .as_ref()
            .ok_or("Precise scalar pipeline unavailable")?;
        let parameters = parameters(source, grid, [[0, 0], grid.dimensions()])?;
        let uniform = allocation::initialized_buffer(
            d,
            "scalar frame sampling grid",
            bytemuck::cast_slice(&parameters),
            wgpu::BufferUsages::UNIFORM,
        )?;
        let fixed = allocation::initialized_buffer(
            d,
            "fixed scalar frame coordinates",
            bytemuck::cast_slice(fixed),
            wgpu::BufferUsages::UNIFORM,
        )?;
        let bindings = d.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&source.source),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&source.validity),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(
                        &self.outputs[slot]
                            .as_ref()
                            .ok_or("Scalar target unavailable")?
                            .view,
                    ),
                },
            ],
        });
        let precise = d.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipeline.layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: fixed.as_entire_binding(),
            }],
        });
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("complete precise scalar plane"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&pipeline.pipeline);
        pass.set_bind_group(0, &bindings, &[]);
        pass.set_bind_group(1, &precise, &[]);
        let size = grid.dimensions();
        pass.dispatch_workgroups(size[0].div_ceil(8), size[1].div_ceil(8), 1);
        Ok(())
    }
}

#[cfg(all(test, feature = "gpu-tests"))]
mod tests;
