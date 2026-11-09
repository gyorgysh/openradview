// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use orv_core::{
    study::{Geometry, demo_study},
    volume_material::physical_gradient,
};
use std::{cell::Cell, sync::mpsc, time::Duration};
use wgpu::util::DeviceExt;
pub(crate) fn device(side: Option<u32>) -> (wgpu::Device, wgpu::Queue) {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        flags: crate::instance_flags(),
        backends: crate::configured_backends().unwrap(),
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapter = pollster::block_on(instance.request_adapter(&Default::default()))
        .expect("Actual GPU required for resident source bricks");
    let mut d = crate::device_descriptor(&adapter);
    if let Some(side) = side {
        d.required_limits.max_texture_dimension_3d = side;
    }
    pollster::block_on(adapter.request_device(&d)).unwrap()
}
pub(crate) fn source(dims: [usize; 3], kind: usize, missing: bool, reverse: bool) -> Arc<Volume> {
    let template = demo_study().series.remove(0).frames.remove(0);
    let frames = (0..dims[2])
        .map(|z| {
            let mut f = (*template).clone();
            f.width = dims[0] as u32;
            f.height = dims[1] as u32;
            f.pixels = (0..dims[0] * dims[1])
                .map(|i| {
                    let x = i % dims[0];
                    let y = i / dims[0];
                    match kind {
                        1 => 1.5 * x as f32 + 2.25 * y as f32 - 0.75 * z as f32,
                        2 => {
                            if (x + y + z).is_multiple_of(2) {
                                1e20
                            } else {
                                -1e20
                            }
                        }
                        3 => 0.,
                        _ => {
                            ((13 * x + 7 * y + 17 * z) % 700) as f32 - 200.
                                + ((x * y) % 13) as f32 * 0.03125
                        }
                    }
                })
                .collect::<Vec<_>>()
                .into();
            f.validity = missing.then(|| {
                (0..dims[0] * dims[1])
                    .map(|i| u8::from(!(i + z * dims[0] * dims[1]).is_multiple_of(17)))
                    .collect::<Vec<_>>()
                    .into()
            });
            f.geometry = Some(Geometry {
                position: [
                    1e6 + z as f64 * 0.25,
                    -1e6 + z as f64 * 0.5,
                    1e6 + z as f64 * 2.,
                ],
                orientation: if reverse {
                    [-1., 0., 0., 0., 1., 0.]
                } else {
                    [1., 0., 0., 0., 1., 0.]
                },
                spacing: [1.5, 0.75],
                frame_of_reference: Some("generated resident source bricks".into()),
                nifti_transform: None,
            });
            Arc::new(f)
        })
        .collect();
    Arc::new(Volume::new(frames).unwrap())
}
fn fence(d: &wgpu::Device) {
    d.poll(wgpu::PollType::Wait {
        submission_index: None,
        timeout: Some(Duration::from_secs(30)),
    })
    .unwrap();
}
struct Probe {
    source: Arc<GpuBrickSource>,
    layout: wgpu::BindGroupLayout,
    pipeline: wgpu::ComputePipeline,
    values: wgpu::Buffer,
    validity: wgpu::Buffer,
    analytic: Option<[f64; 3]>,
    _gpu: allocation::GpuReservation,
}
impl Probe {
    fn new(source: Arc<GpuBrickSource>, analytic: Option<[f64; 3]>) -> Self {
        Self::prepare(
            source.clone(),
            analytic,
            source.binding_layout(),
            BRICK_SAMPLING_SHADER,
        )
    }
    fn for_atlas(pool: &GpuBrickAtlasPool) -> Self {
        let shader = format!(
            "{}\n{}",
            ATLAS_SAMPLING_SHADER,
            r#"
struct BrickValue {value:f32,status:u32}
struct BrickGradient {value:vec3f,status:u32}
fn brick_scalar(point:vec3f)->BrickValue {let v=atlas_scalar(point);return BrickValue(v.value,v.status);}
fn brick_index_gradient(point:vec3f)->BrickGradient {let v=atlas_index_gradient(point);return BrickGradient(v.value,v.status);}
"#
        );
        Self::prepare(pool.source().clone(), None, pool.binding_layout(), &shader)
    }
    fn prepare(
        source: Arc<GpuBrickSource>,
        analytic: Option<[f64; 3]>,
        input_layout: &wgpu::BindGroupLayout,
        input_shader: &str,
    ) -> Self {
        let d = source.device();
        let dims = source.source().dimensions();
        let count = dims.iter().product::<usize>();
        let gpu = allocation::reserve_bytes((count * 8 + 65536) as u64, d, source.queue()).unwrap();
        let values = source
            .source()
            .frames()
            .iter()
            .flat_map(|f| f.pixels.iter().copied())
            .collect::<Vec<_>>();
        let valid = source
            .source()
            .frames()
            .iter()
            .flat_map(|f| (0..f.pixels.len()).map(move |i| u32::from(f.has_sample(i))))
            .collect::<Vec<_>>();
        let values = d.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("independent generated dense scalar reference"),
            contents: bytemuck::cast_slice(&values),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let validity = d.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("independent generated dense validity reference"),
            contents: bytemuck::cast_slice(&valid),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let entries: Vec<_> = (0..4)
            .map(|binding| wgpu::BindGroupLayoutEntry {
                binding,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage {
                        read_only: binding != 1,
                    },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            })
            .collect();
        let layout = d.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("generated dense and bricked sample comparison"),
            entries: &entries,
        });
        let text = format!(
            "{}\nconst REF_DIM:vec3u=vec3u({}u,{}u,{}u);\n{}",
            input_shader,
            dims[0],
            dims[1],
            dims[2],
            include_str!("tests.wgsl")
        );
        let shader = d.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("independent source brick sample readback"),
            source: wgpu::ShaderSource::Wgsl(text.into()),
        });
        let p = d.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(input_layout), Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = d.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: None,
            layout: Some(&p),
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        Self {
            source,
            layout,
            pipeline,
            values,
            validity,
            analytic,
            _gpu: gpu,
        }
    }
    fn run(&self, page: &Arc<GpuBrickPage>, points: &[[f32; 4]]) -> Vec<[u32; 12]> {
        assert!(Arc::ptr_eq(page.source(), &self.source));
        self.run_bindings(page.bindings(), points)
    }
    fn run_bindings(
        &self,
        source_bindings: &wgpu::BindGroup,
        points: &[[f32; 4]],
    ) -> Vec<[u32; 12]> {
        let d = self.source.device();
        let q = self.source.queue();
        let bytes = points.len() as u64 * 48;
        let _gpu =
            allocation::reserve_bytes(bytes * 2 + std::mem::size_of_val(points) as u64, d, q)
                .unwrap();
        let input = d.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(points),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let output = d.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: bytes,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let read = d.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: bytes,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let buffers = [&input, &output, &self.values, &self.validity];
        let entries: Vec<_> = buffers
            .into_iter()
            .enumerate()
            .map(|(i, b)| wgpu::BindGroupEntry {
                binding: i as u32,
                resource: b.as_entire_binding(),
            })
            .collect();
        let bindings = d.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.layout,
            entries: &entries,
        });
        let mut e = d.create_command_encoder(&Default::default());
        {
            let mut p = e.begin_compute_pass(&Default::default());
            p.set_pipeline(&self.pipeline);
            p.set_bind_group(0, source_bindings, &[]);
            p.set_bind_group(1, &bindings, &[]);
            p.dispatch_workgroups((points.len() as u32).div_ceil(64), 1, 1);
        }
        e.copy_buffer_to_buffer(&output, 0, &read, 0, bytes);
        q.submit([e.finish()]);
        let (tx, rx) = mpsc::sync_channel(1);
        read.map_async(wgpu::MapMode::Read, .., move |v| {
            let _ = tx.send(v);
        });
        fence(d);
        rx.recv_timeout(Duration::from_secs(10)).unwrap().unwrap();
        let mapped = read.get_mapped_range(..).unwrap();
        let result = mapped
            .as_chunks::<48>()
            .0
            .iter()
            .map(|bytes| {
                std::array::from_fn(|i| {
                    u32::from_le_bytes(bytes[i * 4..i * 4 + 4].try_into().unwrap())
                })
            })
            .collect();
        drop(mapped);
        read.unmap();
        result
    }
    fn verify(&self, page: &Arc<GpuBrickPage>, points: &[[f32; 4]]) -> usize {
        let output = self.run(page, points);
        let mut known = 0;
        for (point, v) in points.iter().zip(output) {
            let p = [point[0] as f64, point[1] as f64, point[2] as f64];
            let expected = match self.source.layout.owner(p) {
                None => BrickSampleStatus::OutsideSource as u32,
                Some(i) if i != page.index() => BrickSampleStatus::NotOwned as u32,
                _ => v[7],
            };
            assert_eq!(v[1], expected, "{p:?}/page{}", page.index());
            if expected == BrickSampleStatus::Known as u32 {
                assert_eq!(v[0], v[6], "scalar {p:?}");
                known += 1;
            }
            let grad_expected = if expected != BrickSampleStatus::Known as u32 {
                expected
            } else if self.source.layout.stencil() != BrickStencil::CentralSecants {
                BrickSampleStatus::StencilUnavailable as u32
            } else {
                v[11]
            };
            assert_eq!(v[5], grad_expected, "gradient {p:?}/page{}", page.index());
            if grad_expected == BrickSampleStatus::Known as u32 {
                assert_eq!(v[2..5], v[8..11], "secants {p:?}");
                let world = self.source.source.voxel_to_world(p);
                if let Some(analytic) = self.analytic {
                    let expected = physical_gradient(&self.source.source, world).unwrap();
                    let g = [
                        f32::from_bits(v[2]) as f64,
                        f32::from_bits(v[3]) as f64,
                        f32::from_bits(v[4]) as f64,
                    ];
                    for a in 0..3 {
                        let mut unit = [0.; 3];
                        unit[a] = 1.;
                        let row = self.source.source.world_vector_to_voxel(unit);
                        let actual = (0..3).map(|i| g[i] * row[i]).sum::<f64>();
                        assert!(
                            (actual - analytic[a]).abs() < 1e-3,
                            "analytic physical gradient {p:?} axis{a}: {actual}/{}",
                            analytic[a]
                        );
                        assert!(
                            (actual - expected[a]).abs() <= 1e-3 * expected[a].abs().max(1.),
                            "physical gradient {p:?} axis{a}: {actual}/{}",
                            expected[a]
                        );
                    }
                }
            }
        }
        known
    }
}
fn points(source: &GpuBrickSource, index: usize) -> Vec<[f32; 4]> {
    let dims = source.layout.dimensions();
    let [lo, hi] = source.layout.cells(index).unwrap();
    let mut p = vec![
        [0.; 4],
        [
            dims[0] as f32 - 1.,
            dims[1] as f32 - 1.,
            dims[2] as f32 - 1.,
            0.,
        ],
        [-0.25, 0., 0., 0.],
        [dims[0] as f32, 0., 0., 0.],
    ];
    for x in [
        1.,
        31.999998,
        32.,
        32f32.next_up(),
        63.999996,
        64.,
        64f32.next_up(),
    ] {
        p.push([x, 1.25, 2.5, 0.]);
    }
    for a in 0..3 {
        for value in [lo[a] as f32, (hi[a] as f32).next_down(), hi[a] as f32] {
            let mut q: [f32; 3] = std::array::from_fn(|i| (lo[i] + hi[i]) as f32 * 0.5);
            q[a] = value;
            p.push([q[0], q[1], q[2], 0.]);
        }
    }
    let mut seed = 17u32;
    for _ in 0..128 {
        let mut q = [0.; 4];
        for a in 0..3 {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            q[a] = lo[a] as f32 + (seed % 65536) as f32 / 65536. * (hi[a] - lo[a]) as f32;
        }
        p.push(q);
    }
    p
}

#[test]
fn all_resident_page_scalar_and_secant_samples_match_dense_global_stencils() {
    let (d, q) = device(None);
    let mut known = 0;
    for side in [32, 64, 128] {
        for (kind, missing, reverse) in [
            (0, false, false),
            (0, true, true),
            (1, false, true),
            (2, true, false),
            (3, false, false),
        ] {
            let cpu = source([side * 2 + 5, 39, 35], kind, missing, reverse);
            let source =
                GpuBrickSource::new(&d, &q, cpu, side, BrickStencil::CentralSecants, || false)
                    .unwrap();
            let indices = (0..source.layout.len()).collect::<Vec<_>>();
            let resident = GpuBrickResidency::prepare(
                source.clone(),
                &indices,
                None,
                MAX_RESIDENT_BRICK_BYTES,
                || false,
            )
            .unwrap();
            let analytic = match kind {
                1 => Some(if reverse {
                    [-2., 1.5, -0.5]
                } else {
                    [2., 1.5, -1.]
                }),
                3 => Some([0.; 3]),
                _ => None,
            };
            let probe = Probe::new(source.clone(), analytic);
            for &i in &indices {
                known += probe.verify(resident.page(i).unwrap(), &points(&source, i));
            }
        }
    }
    assert!(known > 3000);
}

#[test]
fn pages_bypass_the_dense_texture_side_and_refuse_undeclared_gradient_stencils() {
    let (d, q) = device(Some(64));
    let cpu = source([129, 9, 7], 1, false, false);
    assert!(crate::volume_source::GpuVolumeSource::new(&d, &q, cpu.clone(), || false).is_err());
    assert!(
        GpuBrickSource::new(
            &d,
            &q,
            cpu.clone(),
            64,
            BrickStencil::CentralSecants,
            || false
        )
        .is_err()
    );
    for stencil in [BrickStencil::Trilinear, BrickStencil::CentralSecants] {
        let source = GpuBrickSource::new(&d, &q, cpu.clone(), 32, stencil, || false).unwrap();
        let resident = GpuBrickResidency::prepare(
            source.clone(),
            &[0, 1, 2, 3],
            None,
            MAX_RESIDENT_BRICK_BYTES,
            || false,
        )
        .unwrap();
        let probe = Probe::new(source.clone(), Some([2., 1.5, -1.]));
        for i in 0..4 {
            assert!(probe.verify(resident.page(i).unwrap(), &points(&source, i)) > 64);
        }
    }
}

#[test]
fn immutable_generations_reuse_exact_pages_and_keep_nonresident_distinct_from_missing() {
    let (d, q) = device(None);
    let cpu = source([70, 35, 5], 1, true, false);
    let source = GpuBrickSource::new(
        &d,
        &q,
        cpu.clone(),
        32,
        BrickStencil::CentralSecants,
        || false,
    )
    .unwrap();
    let empty = GpuBrickResidency::prepare(source.clone(), &[], None, 0, || false).unwrap();
    assert!(matches!(
        empty.lookup([0.; 3]),
        BrickLookup::Nonresident { index: 0 }
    ));
    assert!(matches!(empty.lookup([-1.; 3]), BrickLookup::OutsideSource));
    assert!(matches!(
        empty.lookup([f64::NAN; 3]),
        BrickLookup::OutsideSource
    ));
    let old = GpuBrickResidency::prepare(
        source.clone(),
        &[1, 0],
        Some(&empty),
        MAX_RESIDENT_BRICK_BYTES,
        || false,
    )
    .unwrap();
    let old0 = old.page(0).unwrap().clone();
    let old1 = old.page(1).unwrap().clone();
    let probe = Probe::new(source.clone(), None);
    let at = points(&source, 0);
    let original = probe.run(&old0, &at);
    assert!(
        original
            .iter()
            .any(|v| v[1] == BrickSampleStatus::Missing as u32)
    );
    let next = GpuBrickResidency::prepare(
        source.clone(),
        &[2, 1],
        Some(&old),
        MAX_RESIDENT_BRICK_BYTES,
        || false,
    )
    .unwrap();
    let stats = next.statistics();
    assert_eq!(stats.reused_pages, 1);
    assert_eq!(stats.uploaded_pages, 1);
    assert_eq!(stats.uploaded_bytes, next.page(2).unwrap().uploaded_bytes());
    assert!(Arc::ptr_eq(next.page(1).unwrap(), &old1));
    assert!(next.page(0).is_none());
    assert!(matches!(
        next.lookup([0.; 3]),
        BrickLookup::Nonresident { index: 0 }
    ));
    assert_eq!(probe.run(&old0, &at), original);
    let reused = GpuBrickResidency::prepare(
        source.clone(),
        &[0, 1],
        Some(&old),
        MAX_RESIDENT_BRICK_BYTES,
        || false,
    )
    .unwrap();
    assert_eq!(reused.statistics().uploaded_pages, 0);
    assert_eq!(reused.statistics().uploaded_bytes, 0);
    assert!(Arc::ptr_eq(reused.page(0).unwrap(), &old0));
    for requested in [&[0, 0][..], &[usize::MAX][..]] {
        assert!(
            GpuBrickResidency::prepare(
                source.clone(),
                requested,
                Some(&old),
                MAX_RESIDENT_BRICK_BYTES,
                || false
            )
            .is_err()
        );
    }
    assert!(
        GpuBrickResidency::prepare(
            source.clone(),
            &[0, 1],
            Some(&old),
            old.statistics().referenced_bytes - 1,
            || false
        )
        .is_err()
    );
    assert!(
        GpuBrickResidency::prepare(
            source.clone(),
            &[],
            Some(&old),
            MAX_RESIDENT_BRICK_BYTES + 1,
            || false
        )
        .is_err()
    );
    let replacement =
        GpuBrickSource::new(&d, &q, cpu, 32, BrickStencil::CentralSecants, || false).unwrap();
    assert!(
        GpuBrickResidency::prepare(
            replacement,
            &[0],
            Some(&old),
            MAX_RESIDENT_BRICK_BYTES,
            || false
        )
        .is_err()
    );
    assert_eq!(probe.run(&old0, &at), original);
}

#[test]
fn cancelled_or_unadmitted_page_generations_preserve_old_data_and_release_all_owned_resources() {
    let (d, q) = device(None);
    let cpu = source([70, 35, 5], 0, true, false);
    let before_gpu = MemoryBudget::gpu().used();
    let before_cpu = MemoryBudget::volumes().used();
    let owner =
        GpuBrickSource::new(&d, &q, cpu, 32, BrickStencil::CentralSecants, || false).unwrap();
    let old = GpuBrickResidency::prepare(
        owner.clone(),
        &[0, 1],
        None,
        MAX_RESIDENT_BRICK_BYTES,
        || false,
    )
    .unwrap();
    let probe = Probe::new(owner.clone(), None);
    let at = points(&owner, 0);
    let original = probe.run(old.page(0).unwrap(), &at);
    fence(&d);
    let gpu = MemoryBudget::gpu();
    let ram = MemoryBudget::volumes();
    let stable_gpu = gpu.used();
    let stable_cpu = ram.used();
    for cutoff in [0, 1, 10, 80, 200] {
        let calls = Cell::new(0);
        let result = GpuBrickResidency::prepare(
            owner.clone(),
            &[0, 2, 3],
            Some(&old),
            MAX_RESIDENT_BRICK_BYTES,
            || {
                let n = calls.get();
                calls.set(n + 1);
                n >= cutoff
            },
        );
        assert!(result.is_err());
        fence(&d);
        assert_eq!(gpu.used(), stable_gpu);
        assert_eq!(ram.used(), stable_cpu);
        assert_eq!(probe.run(old.page(0).unwrap(), &at), original);
        fence(&d);
    }
    let limit = gpu.limit();
    gpu.set_limit(stable_gpu);
    let failed = GpuBrickResidency::prepare(
        owner.clone(),
        &[2],
        Some(&old),
        MAX_RESIDENT_BRICK_BYTES,
        || false,
    );
    gpu.set_limit(limit);
    assert!(failed.is_err());
    fence(&d);
    assert_eq!(gpu.used(), stable_gpu);
    assert_eq!(ram.used(), stable_cpu);
    let limit = ram.limit();
    ram.set_limit(stable_cpu);
    let failed = GpuBrickResidency::prepare(
        owner.clone(),
        &[2],
        Some(&old),
        MAX_RESIDENT_BRICK_BYTES,
        || false,
    );
    ram.set_limit(limit);
    assert!(failed.is_err());
    fence(&d);
    assert_eq!(gpu.used(), stable_gpu);
    assert_eq!(ram.used(), stable_cpu);
    assert_eq!(probe.run(old.page(0).unwrap(), &at), original);
    let page = old.page(0).unwrap().clone();
    let weak = Arc::downgrade(&owner);
    drop(old);
    drop(owner);
    assert!(weak.upgrade().is_some());
    assert_eq!(probe.run(&page, &at), original);
    drop(probe);
    assert!(weak.upgrade().is_some());
    drop(page);
    fence(&d);
    assert!(weak.upgrade().is_none());
    assert_eq!(gpu.used(), before_gpu);
    assert_eq!(ram.used(), before_cpu);
}

#[test]
fn unsupported_page_values_and_other_device_sources_cannot_replace_old_residency() {
    let (d, q) = device(None);
    let cpu = source([70, 9, 5], 1, false, false);
    let mut frames = cpu.frames().to_vec();
    let mut first = (*frames[0]).clone();
    let mut values = first.pixels.to_vec();
    values[0] = 1e21;
    first.pixels = values.into();
    frames[0] = Arc::new(first);
    let cpu = Arc::new(Volume::new(frames).unwrap());
    let owner = GpuBrickSource::new(
        &d,
        &q,
        cpu.clone(),
        32,
        BrickStencil::CentralSecants,
        || false,
    )
    .unwrap();
    let old =
        GpuBrickResidency::prepare(owner.clone(), &[2], None, MAX_RESIDENT_BRICK_BYTES, || {
            false
        })
        .unwrap();
    assert!(
        GpuBrickResidency::prepare(
            owner.clone(),
            &[0],
            Some(&old),
            MAX_RESIDENT_BRICK_BYTES,
            || false
        )
        .is_err()
    );
    assert_eq!(old.statistics().uploaded_pages, 1);
    assert!(old.page(2).is_some());
    assert!(old.page(0).is_none());
    let (other, other_queue) = device(None);
    let foreign = GpuBrickSource::new(
        &other,
        &other_queue,
        cpu,
        32,
        BrickStencil::CentralSecants,
        || false,
    )
    .unwrap();
    assert!(
        GpuBrickResidency::prepare(foreign, &[2], Some(&old), MAX_RESIDENT_BRICK_BYTES, || {
            false
        })
        .is_err()
    );
    assert!(
        GpuBrickSource::new(
            &d,
            &q,
            source([9; 3], 0, false, false),
            32,
            BrickStencil::Trilinear,
            || true
        )
        .is_err()
    );
}

#[path = "atlas_tests.rs"]
mod atlas_tests;
