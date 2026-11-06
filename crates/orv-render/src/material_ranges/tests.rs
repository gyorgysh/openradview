// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use crate::{
    material_volume::{Lighting, MaterialVolumeRenderer},
    volume::VolumeTarget,
};
use orv_core::{
    ViewState,
    study::{Geometry, demo_study},
    volume::Volume,
    volume_material::MaterialPoint,
    volume_view::{Camera, PhysicalClip, Projection, Quality, VolumeView},
};
use std::{cell::Cell, sync::mpsc, time::Duration};

fn device(change: impl FnOnce(&mut wgpu::Limits)) -> (wgpu::Device, wgpu::Queue) {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: crate::configured_backends().unwrap(),
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapter = pollster::block_on(instance.request_adapter(&Default::default()))
        .expect("Actual GPU required");
    let mut descriptor = crate::device_descriptor(&adapter);
    change(&mut descriptor.required_limits);
    pollster::block_on(adapter.request_device(&descriptor)).unwrap()
}
fn source(
    dim: [u32; 3],
    reverse: bool,
    offset: f64,
    sample: impl Fn([u32; 3]) -> (f32, bool),
) -> Arc<Volume> {
    let template = demo_study().series.remove(0).frames.remove(0);
    let mut frames: Vec<_> = (0..dim[2])
        .map(|z| {
            let mut frame = (*template).clone();
            frame.width = dim[0];
            frame.height = dim[1];
            let samples: Vec<_> = (0..dim[1])
                .flat_map(|y| (0..dim[0]).map(move |x| [x, y, z]))
                .map(&sample)
                .collect();
            frame.pixels = samples.iter().map(|s| s.0).collect::<Vec<_>>().into();
            frame.validity = Some(
                samples
                    .iter()
                    .map(|s| u8::from(s.1))
                    .collect::<Vec<_>>()
                    .into(),
            );
            frame.geometry = Some(Geometry {
                position: [
                    offset + f64::from(z) * 0.13,
                    -offset + f64::from(z) * 0.27,
                    f64::from(z) * 1.7,
                ],
                orientation: if reverse {
                    [-1., 0., 0., 0., 1., 0.]
                } else {
                    [1., 0., 0., 0., 1., 0.]
                },
                spacing: [1.3, 0.7],
                frame_of_reference: Some("generated acceleration".into()),
                nifti_transform: None,
            });
            Arc::new(frame)
        })
        .collect();
    frames.reverse();
    Arc::new(Volume::new(frames).unwrap())
}
fn material(points: &[(f32, f32)]) -> Arc<VolumeMaterial> {
    Arc::new(
        VolumeMaterial::new(
            &points
                .iter()
                .map(|(value, extinction)| MaterialPoint {
                    value: *value,
                    luminance: 0.67,
                    extinction_per_mm: *extinction,
                })
                .collect::<Vec<_>>(),
        )
        .unwrap(),
    )
}
fn default_material() -> Arc<VolumeMaterial> {
    material(&[
        (-1200., 0.),
        (-800., 0.),
        (-799., 0.015),
        (500., 0.035),
        (1200., 0.),
        (1600., 0.),
    ])
}
fn fence(device: &wgpu::Device) {
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(Duration::from_secs(30)),
        })
        .unwrap();
}
fn bytes(gpu: &GpuVolumeSource, texture: &wgpu::Texture, bpp: u32) -> Vec<u8> {
    let size = texture.size();
    let row = (size.width * bpp).div_ceil(256) * 256;
    let buffer = gpu.device().create_buffer(&wgpu::BufferDescriptor {
        label: Some("generated acceleration readback"),
        size: u64::from(row) * u64::from(size.height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = gpu.device().create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: Some(size.height),
            },
        },
        size,
    );
    gpu.queue().submit([encoder.finish()]);
    let (tx, rx) = mpsc::sync_channel(1);
    buffer.map_async(wgpu::MapMode::Read, .., move |r| {
        let _ = tx.send(r);
    });
    fence(gpu.device());
    rx.recv_timeout(Duration::from_secs(10)).unwrap().unwrap();
    let mapped = buffer.get_mapped_range(..).unwrap();
    let result = mapped
        .chunks_exact(row as usize)
        .flat_map(|r| r[..(size.width * bpp) as usize].iter().copied())
        .collect();
    drop(mapped);
    buffer.unmap();
    result
}
#[allow(clippy::too_many_arguments)]
fn render(
    renderer: &mut MaterialVolumeRenderer,
    gpu: &GpuVolumeSource,
    state: &VolumeView,
    nav: &ViewState,
    material: &VolumeMaterial,
    lighting: Lighting,
    size: [u32; 2],
    rows: u32,
) -> Vec<u8> {
    for first in (0..size[1]).step_by(rows as usize) {
        let mut encoder = gpu.device().create_command_encoder(&Default::default());
        renderer
            .render_target(
                &mut encoder,
                nav,
                state,
                material,
                lighting,
                VolumeTarget {
                    slot: 0,
                    size,
                    region: [[0, first], [size[0], rows.min(size[1] - first)]],
                },
            )
            .unwrap();
        gpu.queue().submit([encoder.finish()]);
        fence(gpu.device());
    }
    bytes(gpu, renderer.output_texture(0).unwrap(), 8)
}
fn half(bits: u16) -> f64 {
    assert_ne!((bits >> 10) & 31, 31, "Non-finite optical output");
    let f = f64::from(bits & 1023);
    let e = i32::from((bits >> 10) & 31);
    let v = if e == 0 {
        f * 2f64.powi(-24)
    } else {
        (1. + f / 1024.) * 2f64.powi(e - 15)
    };
    if bits & 32768 == 0 { v } else { -v }
}
fn channels(data: &[u8]) -> Vec<f64> {
    data.as_chunks::<2>()
        .0
        .iter()
        .map(|b| half(u16::from_le_bytes(*b)))
        .collect()
}

#[test]
fn classification_keeps_internal_peaks_isolated_zero_knots_and_subnormal_coverage_conservative() {
    let m = material(&[(-100., 0.), (-50., 0.), (0., 1.), (50., 0.), (100., 0.)]);
    for bounds in [[-1000., -51.], [51., 1000.]] {
        assert!(zero_coefficients(&m, bounds));
    }
    for bounds in [
        [-100., 100.],
        [-50., 50.],
        [-50., -50.],
        [50., 50.],
        [-50., -49.99999],
    ] {
        assert!(!zero_coefficients(&m, bounds), "{bounds:?}");
    }
    assert!(m.transparent_between(50., 50.));
    let tiny = f32::from_bits(1);
    let s = ScalarSummary {
        minimum: tiny,
        maximum: tiny * 2.,
        valid_vertices: 1,
        vertices: 8,
    };
    let interval = scalar_interval(s);
    assert!(interval[0] <= 0. && interval[1] >= tiny * 2.);
    let interval = scalar_interval(ScalarSummary {
        minimum: -tiny * 2.,
        maximum: -tiny,
        ..s
    });
    assert!(interval[0] <= -tiny * 2. && interval[1] >= 0.);
    let nonzero = material(&[(0., tiny), (1., 0.)]);
    assert!(!zero_coefficients(&nonzero, [0., 0.5]));
    assert!(zero_coefficients(
        &material(&[(-1., 0.), (1., 0.)]),
        [-1e20, 1e20]
    ));
}

#[test]
fn accelerated_optics_equal_dense_pixels_for_thin_features_clips_missing_and_affines() {
    let (d, q) = device(|_| {});
    let mut total_skippable = 0;
    for variant in 0..4 {
        let m = if variant < 2 {
            default_material()
        } else {
            material(&[
                (0., 0.),
                (1., 0.005),
                (500., 0.035),
                (1200., 0.),
                (1600., 0.),
            ])
        };
        let volume = source(
            [35, 27, 19],
            variant & 1 != 0,
            if variant >= 2 { 1e6 } else { 0. },
            |[x, y, z]| {
                let sphere = (f64::from(x) - 17.).powi(2)
                    + (f64::from(y) - 13.).powi(2)
                    + (f64::from(z) - 9.).powi(2)
                    < 49.;
                let feature = x == 8 || (z == 16 && (9..19).contains(&y));
                let valid = variant < 2 || !(x == 9 && (7..17).contains(&y)) && z != 11;
                (
                    if sphere || feature {
                        ((x * 31 + y * 17 + z * 13) % 1000) as f32
                    } else if variant < 2 {
                        -1000.
                    } else {
                        0.
                    },
                    valid,
                )
            },
        );
        let gpu = GpuVolumeSource::new(&d, &q, volume.clone(), || false).unwrap();
        let ranges = VolumeRanges::build(volume.clone(), 8, || false).unwrap();
        let flags = GpuMaterialRanges::new(gpu.clone(), ranges, m.clone(), || false).unwrap();
        total_skippable += flags.transparent_tiles();
        let mut accelerated =
            MaterialVolumeRenderer::new_accelerated(flags, wgpu::TextureFormat::Rgba8UnormSrgb)
                .unwrap();
        let mut dense = MaterialVolumeRenderer::new_dense_reference(
            gpu.clone(),
            wgpu::TextureFormat::Rgba8UnormSrgb,
        )
        .unwrap();
        let mut legacy =
            MaterialVolumeRenderer::new(gpu.clone(), wgpu::TextureFormat::Rgba8UnormSrgb).unwrap();
        for (yaw, pitch, roll) in [
            (0., 0., 0.),
            (90., 0., 0.),
            (37., -23., 11.),
            (-159., 21., -8.),
        ] {
            let mut state = VolumeView {
                camera: Camera::fit(&volume),
                projection: Projection::Composite,
                quality: Quality::Refined,
                ..VolumeView::fit(&volume)
            };
            state.camera.yaw_degrees = yaw;
            state.camera.pitch_degrees = pitch;
            state.camera.roll_degrees = roll;
            state.camera.vertical_span_mm *= 0.7;
            if pitch != 0. {
                let [lo, hi] = volume.bounds();
                state.clip = Some(PhysicalClip {
                    minimum_mm: std::array::from_fn(|a| lo[a] + (hi[a] - lo[a]) * 0.13),
                    maximum_mm: std::array::from_fn(|a| hi[a] - (hi[a] - lo[a]) * 0.17),
                });
            }
            let nav = ViewState {
                pan: [0.017, -0.023],
                zoom: 1.17,
                ..Default::default()
            };
            for lighting in [Lighting::UNLIT, Lighting::HEADLIGHT] {
                let actual = render(
                    &mut accelerated,
                    &gpu,
                    &state,
                    &nav,
                    &m,
                    lighting,
                    [53, 41],
                    41,
                );
                let reference = render(&mut dense, &gpu, &state, &nav, &m, lighting, [53, 41], 41);
                assert_eq!(
                    actual, reference,
                    "variant={variant}, yaw={yaw}, lighting={lighting:?}"
                );
                let original = render(&mut legacy, &gpu, &state, &nav, &m, lighting, [53, 41], 41);
                for ((a, b), original) in channels(&actual)
                    .iter()
                    .zip(channels(&reference))
                    .zip(channels(&original))
                {
                    assert!((a - b).abs() < 0.00001);
                    assert!(
                        (a - original).abs() <= 0.001,
                        "Shared-stencil/legacy change: {a} vs {original}"
                    );
                }
            }
        }
    }
    assert!(total_skippable > 10);
}

#[test]
fn material_source_binding_rejects_replacements_and_preserves_independent_classifications() {
    let (d, q) = device(|_| {});
    let volume = source([10; 3], false, 0., |_| (-1000., true));
    let other = source([10; 3], false, 0., |_| (-1000., true));
    let gpu = GpuVolumeSource::new(&d, &q, volume.clone(), || false).unwrap();
    let ranges = VolumeRanges::build(volume.clone(), 8, || false).unwrap();
    let m = default_material();
    assert!(
        GpuMaterialRanges::new(
            gpu.clone(),
            VolumeRanges::build(other, 8, || false).unwrap(),
            m.clone(),
            || false
        )
        .is_err()
    );
    let flags = GpuMaterialRanges::new(gpu.clone(), ranges.clone(), m.clone(), || false).unwrap();
    assert_eq!(flags.transparent_tiles(), 8);
    let changed = material(&[(-1200., 0.1), (1600., 0.1)]);
    let opaque =
        GpuMaterialRanges::new(gpu.clone(), ranges.clone(), changed.clone(), || false).unwrap();
    assert_eq!(opaque.transparent_tiles(), 0);
    assert_eq!(flags.transparent_tiles(), 8);
    let mut renderer =
        MaterialVolumeRenderer::new_accelerated(flags.clone(), wgpu::TextureFormat::Rgba8UnormSrgb)
            .unwrap();
    let state = VolumeView {
        camera: Camera::fit(&volume),
        projection: Projection::Composite,
        ..VolumeView::fit(&volume)
    };
    let mut encoder = d.create_command_encoder(&Default::default());
    assert!(
        renderer
            .render_target(
                &mut encoder,
                &ViewState::default(),
                &state,
                &changed,
                Lighting::UNLIT,
                VolumeTarget {
                    slot: 0,
                    size: [3, 3],
                    region: [[0, 0], [3, 3]]
                }
            )
            .is_err()
    );
    assert!(renderer.output_texture(0).is_none());
    assert!(Arc::ptr_eq(flags.source(), &gpu));
    assert!(Arc::ptr_eq(flags.ranges(), &ranges));
    assert!(Arc::ptr_eq(flags.material(), &m));
    let tiny = material(&[(f32::from_bits(1), 0.), (1., 0.)]);
    assert!(GpuMaterialRanges::new(gpu.clone(), ranges.clone(), tiny, || false).is_err());
    let close = material(&[(0., 0.), (1e-21, 0.)]);
    assert!(GpuMaterialRanges::new(gpu, ranges, close, || false).is_err());
}

#[test]
fn tiles_and_requested_2048_output_keep_dense_phase_and_fallback_capabilities() {
    let (d, q) = device(|_| {});
    let volume = source([18, 11, 9], false, 0., |[x, _, _]| {
        (if x == 8 { 0. } else { -1000. }, true)
    });
    let gpu = GpuVolumeSource::new(&d, &q, volume.clone(), || false).unwrap();
    let m = default_material();
    let flags = GpuMaterialRanges::new(
        gpu.clone(),
        VolumeRanges::build(volume.clone(), 8, || false).unwrap(),
        m.clone(),
        || false,
    )
    .unwrap();
    let mut renderer =
        MaterialVolumeRenderer::new_accelerated(flags, wgpu::TextureFormat::Rgba8UnormSrgb)
            .unwrap();
    let mut dense = MaterialVolumeRenderer::new_dense_reference(
        gpu.clone(),
        wgpu::TextureFormat::Rgba8UnormSrgb,
    )
    .unwrap();
    let mut state = VolumeView {
        camera: Camera::fit(&volume),
        projection: Projection::Composite,
        ..VolumeView::fit(&volume)
    };
    state.camera.yaw_degrees = 31.;
    for size in [[73, 57], [2048, 3], [3, 2048]] {
        let expected = render(
            &mut dense,
            &gpu,
            &state,
            &ViewState::default(),
            &m,
            Lighting::HEADLIGHT,
            size,
            size[1],
        );
        let actual = render(
            &mut renderer,
            &gpu,
            &state,
            &ViewState::default(),
            &m,
            Lighting::HEADLIGHT,
            size,
            16,
        );
        assert_eq!(actual, expected);
        let texture = renderer.output_texture(0).unwrap();
        assert_eq!([texture.width(), texture.height()], size);
    }
    for capability in 0..3 {
        let (d, q) = device(|l| match capability {
            0 => l.max_bind_groups = 1,
            1 => l.max_uniform_buffers_per_shader_stage = 1,
            _ => l.max_sampled_textures_per_shader_stage = 2,
        });
        let gpu = GpuVolumeSource::new(&d, &q, volume.clone(), || false).unwrap();
        let flags = GpuMaterialRanges::new(
            gpu.clone(),
            VolumeRanges::build(volume.clone(), 8, || false).unwrap(),
            m.clone(),
            || false,
        )
        .unwrap();
        assert!(
            MaterialVolumeRenderer::new_accelerated(flags, wgpu::TextureFormat::Rgba8UnormSrgb)
                .is_err()
        );
        assert!(MaterialVolumeRenderer::new(gpu, wgpu::TextureFormat::Rgba8UnormSrgb).is_ok());
    }
}

#[test]
fn shared_sampler_preserves_conservative_ranges_nodes_and_validity_under_large_and_tiny_values() {
    let (d, q) = device(|_| {});
    for variant in 0..6 {
        let volume = source([3; 3], false, 0., |[x, y, z]| {
            let n = (x + 3 * y + 9 * z) % 8;
            let value = match variant {
                0 => f32::from_bits(1 + n),
                1 => -f32::from_bits(1 + n),
                2 => {
                    if n % 2 == 0 {
                        1e20
                    } else {
                        -1e20
                    }
                }
                3 => {
                    if n == 0 {
                        1.
                    } else {
                        1.0f32.next_up()
                    }
                }
                4 => [
                    0.,
                    -0.,
                    f32::MIN_POSITIVE,
                    -f32::MIN_POSITIVE,
                    1e-20,
                    -1e-20,
                    1e20,
                    -1e20,
                ][n as usize],
                _ => 0.,
            };
            (value, variant != 4 || [x, y, z] != [1, 1, 1])
        });
        let gpu = GpuVolumeSource::new(&d, &q, volume.clone(), || false).unwrap();
        let code = format!(
            "{}\n{}\n{}",
            crate::VOLUME_SAMPLING_SHADER,
            crate::material_volume::optics_shader(true, false),
            r#"
@group(0) @binding(2) var output:texture_storage_2d<rgba32float,write>;
@compute @workgroup_size(8,8,1)
fn main(@builtin(global_invocation_id) gid:vec3u) {
    if gid.x>=289u || gid.y>=17u {return;}
    let point=vec3f(f32(gid.x%17u),f32(gid.x/17u),f32(gid.y))*.125;
    textureStore(output,vec2i(gid.xy),vec4f(stencil_scalar(scalar_stencil(point)),
        select(0.,1.,stencil_valid(scalar_stencil(point))),scalar(point),select(0.,1.,scalar_valid(point))));
}"#
        );
        let shader = d.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("interpolation precision probe"),
            source: wgpu::ShaderSource::Wgsl(code.into()),
        });
        let pipeline = d.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: None,
            layout: None,
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        let texture = d.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: 289,
                height: 17,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba32Float,
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let mut values = [0f32; 336];
        // sampling is the seventh vec4, with validity in its fourth component.
        values[27] = u8::from(volume.has_missing_scalar()) as f32;
        let uniform = allocation::initialized_buffer(
            &d,
            "interpolation precision parameters",
            bytemuck::cast_slice(&values),
            wgpu::BufferUsages::UNIFORM,
        )
        .unwrap();
        let binding = d.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&gpu.source),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(
                        &texture.create_view(&Default::default()),
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&gpu.validity),
                },
            ],
        });
        let mut encoder = d.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &binding, &[]);
            pass.dispatch_workgroups(289u32.div_ceil(8), 17u32.div_ceil(8), 1);
        }
        q.submit([encoder.finish()]);
        let data = bytes(&gpu, &texture, 16);
        for (index, word) in data.as_chunks::<16>().0.iter().enumerate() {
            let actual: [f32; 4] = std::array::from_fn(|i| {
                f32::from_le_bytes(word[i * 4..i * 4 + 4].try_into().unwrap())
            });
            let voxel = [
                (index % 289 % 17) as f64 * 0.125,
                (index % 289 / 17) as f64 * 0.125,
                (index / 289) as f64 * 0.125,
            ];
            let lower = voxel.map(|v| v.floor() as usize);
            let mut minimum = f32::INFINITY;
            let mut maximum = f32::NEG_INFINITY;
            let mut ideal = 0.;
            let mut valid = true;
            for corner in 0..8 {
                let hi = [corner & 1 != 0, corner & 2 != 0, corner & 4 != 0];
                let p: [usize; 3] = std::array::from_fn(|a| (lower[a] + usize::from(hi[a])).min(2));
                let value = volume.frames()[p[2]].pixels[p[1] * 3 + p[0]];
                minimum = minimum.min(value);
                maximum = maximum.max(value);
                let weight: f64 = (0..3)
                    .map(|a| {
                        if hi[a] {
                            voxel[a].fract()
                        } else {
                            1. - voxel[a].fract()
                        }
                    })
                    .product();
                ideal += f64::from(value) * weight;
                if weight > 0. && !volume.frames()[p[2]].has_sample(p[1] * 3 + p[0]) {
                    valid = false;
                }
            }
            let bounds = scalar_interval(ScalarSummary {
                minimum,
                maximum,
                valid_vertices: 8,
                vertices: 8,
            });
            assert!(
                actual[0].is_finite() && actual[0] >= bounds[0] && actual[0] <= bounds[1],
                "variant={variant}, {voxel:?}, {actual:?}, {bounds:?}"
            );
            let scale = f64::from(minimum.abs().max(maximum.abs()));
            assert!(
                (f64::from(actual[0]) - ideal).abs()
                    <= scale * 4e-6 + f64::from(f32::MIN_POSITIVE) * 8.
            );
            assert_eq!(actual[1], u8::from(valid) as f32, "{voxel:?}");
            assert_eq!(actual[1], actual[3]);
            if voxel.iter().all(|v| v.fract() == 0.) {
                let original = volume.frames()[lower[2]].pixels[lower[1] * 3 + lower[0]];
                assert!(
                    actual[0] == original || original.is_subnormal() && actual[0] == 0.,
                    "Node {voxel:?}: {} vs {original}",
                    actual[0]
                );
            }
        }
    }
}

#[test]
fn admission_postsubmit_cancellation_and_retained_range_resources_release_exactly() {
    let (d, q) = device(|_| {});
    let volume = source([18; 3], false, 0., |_| (-1000., true));
    let gpu = GpuVolumeSource::new(&d, &q, volume.clone(), || false).unwrap();
    let ranges = VolumeRanges::build(volume, 8, || false).unwrap();
    let m = default_material();
    let cpu = MemoryBudget::volumes();
    let gpu_budget = MemoryBudget::gpu();
    let cpu_base = cpu.used();
    let gpu_base = gpu_budget.used();
    assert!(GpuMaterialRanges::new(gpu.clone(), ranges.clone(), m.clone(), || true).is_err());
    let limit = cpu.limit();
    cpu.set_limit(0);
    let denied = GpuMaterialRanges::new(gpu.clone(), ranges.clone(), m.clone(), || false);
    cpu.set_limit(limit);
    assert!(denied.is_err());
    assert_eq!(cpu.used(), cpu_base);
    let limit = gpu_budget.limit();
    gpu_budget.set_limit(0);
    let denied = GpuMaterialRanges::new(gpu.clone(), ranges.clone(), m.clone(), || false);
    gpu_budget.set_limit(limit);
    assert!(denied.is_err());
    assert_eq!(cpu.used(), cpu_base);
    let calls = Cell::new(0);
    let before_wait = 1 + ranges.counts()[1] * ranges.counts()[2];
    let cancelled = GpuMaterialRanges::new(gpu.clone(), ranges.clone(), m.clone(), || {
        let n = calls.get();
        calls.set(n + 1);
        n >= before_wait
    });
    assert!(matches!(cancelled, Err("Cancelled")));
    assert!(calls.get() > before_wait);
    fence(&d);
    assert_eq!(cpu.used(), cpu_base);
    assert_eq!(gpu_budget.used(), gpu_base);
    let flags = GpuMaterialRanges::new(gpu.clone(), ranges.clone(), m, || false).unwrap();
    let weak = Arc::downgrade(&flags);
    let renderer =
        MaterialVolumeRenderer::new_accelerated(flags.clone(), wgpu::TextureFormat::Rgba8UnormSrgb)
            .unwrap();
    drop(flags);
    assert!(weak.upgrade().is_some());
    assert!(cpu.used() > cpu_base);
    drop(renderer);
    fence(&d);
    assert!(weak.upgrade().is_none());
    assert_eq!(cpu.used(), cpu_base);
    assert_eq!(gpu_budget.used(), gpu_base);
}

#[test]
fn optics_match_independent_physical_slab_attenuation_and_extreme_endpoint_clamping() {
    let (d, q) = device(|_| {});
    for reverse in [false, true] {
        let volume = source([11, 9, 7], reverse, 0., |_| (10., true));
        let gpu = GpuVolumeSource::new(&d, &q, volume.clone(), || false).unwrap();
        for sigma in [0., 0.08] {
            let m = material(&[(-1000., sigma), (1000., sigma)]);
            let flags = GpuMaterialRanges::new(
                gpu.clone(),
                VolumeRanges::build(volume.clone(), 8, || false).unwrap(),
                m.clone(),
                || false,
            )
            .unwrap();
            assert_eq!(flags.transparent_tiles(), if sigma == 0. { 2 } else { 0 });
            let mut renderer =
                MaterialVolumeRenderer::new_accelerated(flags, wgpu::TextureFormat::Rgba8UnormSrgb)
                    .unwrap();
            let mut state = VolumeView::fit(&volume);
            state.projection = Projection::Composite;
            state.camera.yaw_degrees = 37.;
            state.camera.pitch_degrees = -19.;
            state.camera.roll_degrees = 8.;
            state.camera.vertical_span_mm *= 0.7;
            let nav = ViewState::default();
            let size = [31, 27];
            let data = render(
                &mut renderer,
                &gpu,
                &state,
                &nav,
                &m,
                Lighting::UNLIT,
                size,
                size[1],
            );
            let frame = state
                .camera
                .frame(&nav, f64::from(size[0]) / f64::from(size[1]))
                .unwrap();
            let mut hit = 0;
            for (index, pixel) in channels(&data).as_chunks::<4>().0.iter().enumerate() {
                let uv = [
                    (index % size[0] as usize) as f64 + 0.5,
                    (index / size[0] as usize) as f64 + 0.5,
                ];
                let (origin, direction) =
                    frame.ray([uv[0] / f64::from(size[0]), uv[1] / f64::from(size[1])]);
                // Solve the independently transformed scalar-centre slab range.
                let o = volume.world_to_voxel(origin);
                let v = volume.world_vector_to_voxel(direction);
                let mut near = f64::NEG_INFINITY;
                let mut far = f64::INFINITY;
                for axis in 0..3 {
                    if v[axis].abs() < 1e-12 {
                        if o[axis] < 0. || o[axis] > (volume.dimensions()[axis] - 1) as f64 {
                            far = f64::NEG_INFINITY;
                        }
                    } else {
                        let a = -o[axis] / v[axis];
                        let b = ((volume.dimensions()[axis] - 1) as f64 - o[axis]) / v[axis];
                        near = near.max(a.min(b));
                        far = far.min(a.max(b));
                    }
                }
                let length = (far - near).max(0.);
                if length > 0. {
                    hit += 1;
                }
                let transmission = (-f64::from(sigma) * length).exp();
                let expected = 0.67 * (1. - transmission) + 0.0030352698 * transmission;
                assert!(
                    (pixel[0] - expected).abs() < 0.0006,
                    "pixel={index}, actual={} expected={expected}",
                    pixel[0]
                );
                assert_eq!(pixel[0], pixel[1]);
                assert_eq!(pixel[1], pixel[2]);
                assert_eq!(pixel[3], 1.);
            }
            assert!(hit > 50);
        }
    }
    let volume = source([3; 3], false, 0., |_| (-1e20, true));
    let gpu = GpuVolumeSource::new(&d, &q, volume.clone(), || false).unwrap();
    let m = material(&[(0., 0.), (2e-20, 1.)]);
    let flags = GpuMaterialRanges::new(
        gpu.clone(),
        VolumeRanges::build(volume.clone(), 8, || false).unwrap(),
        m.clone(),
        || false,
    )
    .unwrap();
    assert_eq!(flags.transparent_tiles(), 1);
    let state = VolumeView {
        projection: Projection::Composite,
        ..VolumeView::fit(&volume)
    };
    let mut accelerated =
        MaterialVolumeRenderer::new_accelerated(flags, wgpu::TextureFormat::Rgba8UnormSrgb)
            .unwrap();
    let mut dense = MaterialVolumeRenderer::new_dense_reference(
        gpu.clone(),
        wgpu::TextureFormat::Rgba8UnormSrgb,
    )
    .unwrap();
    let actual = render(
        &mut accelerated,
        &gpu,
        &state,
        &ViewState::default(),
        &m,
        Lighting::UNLIT,
        [9, 9],
        9,
    );
    assert_eq!(
        actual,
        render(
            &mut dense,
            &gpu,
            &state,
            &ViewState::default(),
            &m,
            Lighting::UNLIT,
            [9, 9],
            9
        )
    );
    let mut legacy =
        MaterialVolumeRenderer::new(gpu.clone(), wgpu::TextureFormat::Rgba8UnormSrgb).unwrap();
    assert_eq!(
        actual,
        render(
            &mut legacy,
            &gpu,
            &state,
            &ViewState::default(),
            &m,
            Lighting::UNLIT,
            [9, 9],
            9
        )
    );
    for pixel in channels(&actual).as_chunks::<4>().0 {
        assert!((pixel[0] - 0.0030352698).abs() < 0.000002);
        assert_eq!(pixel[3], 1.);
    }
}

#[derive(Clone, Copy)]
struct Interval {
    lo: f64,
    hi: f64,
}
impl Interval {
    fn value(v: f32) -> Self {
        Self {
            lo: f64::from(v),
            hi: f64::from(v),
        }
    }
    fn hull(self, other: Self) -> Self {
        Self {
            lo: self.lo.min(other.lo),
            hi: self.hi.max(other.hi),
        }
    }
    fn rounded(lo: f64, hi: f64) -> Self {
        // One extra f32 ULP outward covers both directed f32 modes and any
        // lost low bits in the f64 interval calculation. Include permitted FTZ.
        let mut lo = f64::from((lo as f32).next_down());
        let mut hi = f64::from((hi as f32).next_up());
        if lo.abs() < f64::from(f32::MIN_POSITIVE) {
            lo = lo.min(0.);
        }
        if hi.abs() < f64::from(f32::MIN_POSITIVE) {
            hi = hi.max(0.);
        }
        Self { lo, hi }
    }
    fn add(self, b: Self) -> Self {
        Self::rounded(self.lo + b.lo, self.hi + b.hi)
    }
    fn mul(self, b: Self) -> Self {
        let p = [
            self.lo * b.lo,
            self.lo * b.hi,
            self.hi * b.lo,
            self.hi * b.hi,
        ];
        Self::rounded(
            p.into_iter().fold(f64::INFINITY, f64::min),
            p.into_iter().fold(f64::NEG_INFINITY, f64::max),
        )
    }
}
fn every_order(terms: &[Interval], multiply: bool) -> Interval {
    let mut bounds = [Interval { lo: 0., hi: 0. }; 16];
    for mask in 1usize..1 << terms.len() {
        if mask.is_power_of_two() {
            bounds[mask] = terms[mask.trailing_zeros() as usize];
            continue;
        }
        let mut result = Interval {
            lo: f64::INFINITY,
            hi: f64::NEG_INFINITY,
        };
        let mut part = (mask - 1) & mask;
        while part != 0 {
            let other = mask ^ part;
            if other != 0 {
                result = result.hull(if multiply {
                    bounds[part].mul(bounds[other])
                } else {
                    bounds[part].add(bounds[other])
                });
            }
            part = (part - 1) & mask;
        }
        bounds[mask] = result;
    }
    bounds[(1 << terms.len()) - 1]
}
fn position_interval(o: f32, d: f32, near: f32, step: f32, index: u32) -> Interval {
    let o = Interval::value(o);
    let d = Interval::value(d);
    let n = Interval::value(near);
    let s = Interval::value(step);
    let i = Interval::value(index as f32);
    let half = Interval::value(0.5);
    let t = Interval::value(index as f32 + 0.5);
    // Cover the nested expression and every ordering/parenthesization of its
    // expanded affine monomials, with outward error and FTZ after every operation.
    let nested = o.add(d.mul(n.add(t.mul(s))));
    let expanded = every_order(
        &[
            o,
            d.mul(n),
            every_order(&[d, i, s], true),
            every_order(&[d, half, s], true),
        ],
        false,
    );
    nested.hull(expanded)
}

fn lerp_interval(a: Interval, b: Interval, t: Interval) -> Interval {
    let one = Interval::value(1.);
    let negative = |v: Interval| Interval {
        lo: -v.hi,
        hi: -v.lo,
    };
    a.mul(one.add(negative(t)))
        .add(b.mul(t))
        .hull(a.add(b.add(negative(a)).mul(t)))
}
#[test]
fn scalar_bounds_include_directed_rounding_reassociated_lerps_and_flush_to_zero() {
    let fractions = [
        0.,
        f32::from_bits(1),
        0.5,
        0.5f32.next_up(),
        1.0f32.next_down(),
    ];
    let amplitudes = [
        0.,
        f32::from_bits(3),
        f32::MIN_POSITIVE,
        1e-20,
        1.,
        1000.,
        1e20,
    ];
    for amplitude in amplitudes {
        if amplitude == 0. {
            assert_eq!(
                scalar_interval(ScalarSummary {
                    minimum: 0.,
                    maximum: 0.,
                    valid_vertices: 8,
                    vertices: 8
                }),
                [0., 0.]
            );
            continue;
        }
        for pattern in 0u32..256 {
            let vertices: [f32; 8] = std::array::from_fn(|i| {
                if pattern & (1 << i) == 0 {
                    amplitude
                } else {
                    -amplitude
                }
            });
            let bounds = scalar_interval(ScalarSummary {
                minimum: vertices.into_iter().fold(f32::INFINITY, f32::min),
                maximum: vertices.into_iter().fold(f32::NEG_INFINITY, f32::max),
                valid_vertices: 8,
                vertices: 8,
            });
            for x in fractions {
                for y in fractions {
                    for z in fractions {
                        let fraction = |v: f32| {
                            if v.is_subnormal() {
                                Interval {
                                    lo: 0.,
                                    hi: f64::from(v),
                                }
                            } else {
                                Interval::value(v)
                            }
                        };
                        let [x, y, z] = [x, y, z].map(fraction);
                        let v = vertices.map(Interval::value);
                        let c = [
                            lerp_interval(v[0], v[1], x),
                            lerp_interval(v[2], v[3], x),
                            lerp_interval(v[4], v[5], x),
                            lerp_interval(v[6], v[7], x),
                        ];
                        let result = lerp_interval(
                            lerp_interval(c[0], c[1], y),
                            lerp_interval(c[2], c[3], y),
                            z,
                        );
                        assert!(
                            result.lo >= f64::from(bounds[0]) && result.hi <= f64::from(bounds[1]),
                            "amplitude={amplitude}, pattern={pattern}, {}..{} vs {bounds:?}",
                            result.lo,
                            result.hi
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn actual_gpu_tile_jumps_preserve_all_intermediate_indices_under_directed_rounding_and_ftz() {
    let (d, q) = device(|_| {});
    let volume = source([67, 19, 11], false, 0., |[x, _, _]| {
        (if x == 32 { 0. } else { -1000. }, true)
    });
    let gpu = GpuVolumeSource::new(&d, &q, volume.clone(), || false).unwrap();
    let ranges = VolumeRanges::build(volume.clone(), 8, || false).unwrap();
    let flags =
        GpuMaterialRanges::new(gpu.clone(), ranges.clone(), default_material(), || false).unwrap();
    let code = format!(
        "{}\n{}\n{}",
        crate::VOLUME_SAMPLING_SHADER,
        crate::material_volume::optics_shader(true, true),
        r#"
@group(0) @binding(2) var output:texture_storage_2d<rgba32uint,write>;
@compute @workgroup_size(8,1,1)
fn main(@builtin(global_invocation_id) gid:vec3u) {
    if gid.x>=64u {return;}
    let count=u32(p.sampling.y);let index=min(gid.x*count/64u,count-1u);
    let point=material_point(p.origin.xyz,p.display.x,p.sampling.x,index);
    let stencil=scalar_stencil(point);
    var advance=0u;
    if stencil_valid(stencil) {advance=transparent_advance(stencil.lower,point,p.origin.xyz,p.display.x,p.sampling.x,index,count);}
    textureStore(output,vec2i(gid.xy),vec4u(bitcast<vec3u>(point),advance));
}"#
    );
    let layout = d.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None,
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
                    format: wgpu::TextureFormat::Rgba32Uint,
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
    let pipeline_layout = d.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[Some(&layout), Some(flags.layout())],
        immediate_size: 0,
    });
    let shader = d.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: None,
        source: wgpu::ShaderSource::Wgsl(code.into()),
    });
    let pipeline = d.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: None,
        layout: Some(&pipeline_layout),
        module: &shader,
        entry_point: Some("main"),
        compilation_options: Default::default(),
        cache: None,
    });
    let texture = d.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d {
            width: 64,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba32Uint,
        usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let mut jumped = 0;
    let mut checked = 0;
    let mut fallback = 0;
    for (origin, direction, near, step, count, expects_fallback) in [
        ([1., 7., 5.], [1., 0., 0.], 0., 0.125, 512, false),
        ([65., 7., 5.], [-1., 0., 0.], 0., 0.125, 512, false),
        ([-4095., 7., 5.], [1., 0., 0.], 4096., 0.125, 512, false),
        ([-4092., 7., 5.], [1., 0., 0.], 4096., 1e-6, 4096, false),
        ([-4096., 7., 5.], [1., 0., 0.], 4097., 0.125, 512, true),
        ([1., 7., 5.], [1e6, 0., 0.], 0., 1e-6, 64, false),
        (
            [1., 7., 5.],
            [1_000_000.0f32.next_up(), 0., 0.],
            0.,
            1e-6,
            64,
            true,
        ),
        ([1., 7., 5.], [1., 1e-38, -1e-40], 0., 0.125, 512, false),
        ([1., 7., 5.], [1., 0., 0.], 0., 9e-7, 4096, true),
    ] {
        let mut values = [0f32; 336];
        values[..3].copy_from_slice(&origin);
        values[12..15].copy_from_slice(&direction);
        values[16] = near;
        values[24] = step;
        values[25] = count as f32;
        let uniform = allocation::initialized_buffer(
            &d,
            "jump proof ray",
            bytemuck::cast_slice(&values),
            wgpu::BufferUsages::UNIFORM,
        )
        .unwrap();
        let bindings = d.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&gpu.source),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(
                        &texture.create_view(&Default::default()),
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&gpu.validity),
                },
            ],
        });
        let mut encoder = d.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &bindings, &[]);
            pass.set_bind_group(1, flags.bindings(), &[]);
            pass.dispatch_workgroups(8, 1, 1);
        }
        q.submit([encoder.finish()]);
        let data = bytes(&gpu, &texture, 16);
        for (query, word) in data.as_chunks::<16>().0.iter().enumerate() {
            let raw: [u32; 4] = std::array::from_fn(|i| {
                u32::from_le_bytes(word[i * 4..i * 4 + 4].try_into().unwrap())
            });
            let point: [f64; 3] = std::array::from_fn(|a| f64::from(f32::from_bits(raw[a])));
            let advance = raw[3];
            let index = (query as u32 * count / 64).min(count - 1);
            assert!(advance <= count - index);
            if expects_fallback {
                assert!(advance <= 1);
                fallback += 1;
            }
            if advance <= 1 {
                continue;
            }
            jumped += 1;
            let clamped =
                std::array::from_fn(|a| point[a].clamp(0., (volume.dimensions()[a] - 1) as f64));
            let tile = ranges.tile(ranges.owner(clamped).unwrap()).unwrap();
            assert_eq!(tile.summary.minimum, -1000.);
            assert_eq!(tile.summary.maximum, -1000.);
            // Check every skipped index using intervals that include both
            // directed rounding modes, all affine term orders and FTZ.
            for j in index..index + advance {
                for a in 0..3 {
                    let bound = position_interval(origin[a], direction[a], near, step, j);
                    assert!(
                        bound.lo >= tile.origin[a] as f64
                            && bound.hi < (tile.origin[a] + tile.cells[a]) as f64,
                        "query={query}, j={j}, axis={a}, {}..{}, tile={tile:?}",
                        bound.lo,
                        bound.hi
                    );
                }
                checked += 1;
            }
        }
    }
    assert!(jumped > 40);
    assert!(checked > 10000);
    assert!(fallback >= 128);
}

#[test]
fn packed_validity_matches_independent_eight_vertex_checks_at_nodes_and_interiors() {
    let (device, queue) = device(|_| {});
    let volume = source([512, 2, 2], false, 0., |[x, y, z]| {
        let pattern = x / 2;
        let corner = (x % 2) + 2 * y + 4 * z;
        (1., pattern & (1 << corner) != 0)
    });
    let gpu = GpuVolumeSource::new(&device, &queue, volume.clone(), || false).unwrap();
    let code = format!(
        "{}\n{}\n{}",
        crate::VOLUME_SAMPLING_SHADER,
        crate::material_volume::optics_shader(true, false),
        r#"
@group(0) @binding(2) var output:texture_storage_2d<rgba32float,write>;
fn old_valid(point:vec3f)->bool {
    let maximum=vec3i(textureDimensions(source))-1;
    let q=clamp(point,vec3f(0.),vec3f(maximum));
    let a=vec3i(floor(q));let b=min(a+1,maximum);let t=fract(q);
    for(var z=0u;z<2u;z++) {for(var y=0u;y<2u;y++) {for(var x=0u;x<2u;x++) {
        let hi=vec3<bool>(x==1u,y==1u,z==1u);
        let w=select(vec3f(1.)-t,t,hi);
        if w.x*w.y*w.z>0. && (textureLoad(validity,select(a,b,hi),0).r & 1u)==0u {return false;}
    }}}
    return true;
}
@compute @workgroup_size(8,8,1)
fn main(@builtin(global_invocation_id) gid:vec3u) {
    if gid.x>=1280u || gid.y>=25u {return;}
    let fractions=array<f32,5>(0.,0.25,0.5,0.99999994,1.);
    let point=vec3f(f32((gid.x/5u)*2u)+fractions[gid.x%5u],fractions[gid.y%5u],fractions[gid.y/5u]);
    textureStore(output,vec2i(gid.xy),vec4f(select(0.,1.,scalar_valid(point)),select(0.,1.,stencil_valid(scalar_stencil(point))),select(0.,1.,old_valid(point)),1.));
}"#
    );
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("packed validity reference"),
        source: wgpu::ShaderSource::Wgsl(code.into()),
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: None,
        layout: None,
        module: &shader,
        entry_point: Some("main"),
        compilation_options: Default::default(),
        cache: None,
    });
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d {
            width: 1280,
            height: 25,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba32Float,
        usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let mut values = [0f32; 336];
    values[27] = 1.;
    let uniform = allocation::initialized_buffer(
        &device,
        "packed validity parameters",
        bytemuck::cast_slice(&values),
        wgpu::BufferUsages::UNIFORM,
    )
    .unwrap();
    let bindings = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&gpu.source),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: uniform.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(
                    &texture.create_view(&Default::default()),
                ),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::TextureView(&gpu.validity),
            },
        ],
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_compute_pass(&Default::default());
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bindings, &[]);
        pass.dispatch_workgroups(160, 4, 1);
    }
    queue.submit([encoder.finish()]);
    let data = bytes(&gpu, &texture, 16);
    let fractions = [0f32, 0.25, 0.5, 0.99999994, 1.];
    for (index, word) in data.as_chunks::<16>().0.iter().enumerate() {
        let px = index % 1280;
        let py = index / 1280;
        let point = [
            (2 * (px / 5)) as f32 + fractions[px % 5],
            fractions[py % 5],
            fractions[py / 5],
        ];
        let lower = point.map(|v| v.floor() as usize);
        let mut expected = true;
        for corner in 0..8 {
            let mut weight = 1f32;
            let mut coordinate = [0; 3];
            for axis in 0..3 {
                let high = (corner >> axis) & 1;
                let fraction = point[axis].fract();
                weight *= if high == 1 { fraction } else { 1. - fraction };
                coordinate[axis] = (lower[axis] + high).min([511, 1, 1][axis]);
            }
            if weight > 0.
                && !volume.frames()[coordinate[2]].has_sample(coordinate[1] * 512 + coordinate[0])
            {
                expected = false;
            }
        }
        let actual: [f32; 4] =
            std::array::from_fn(|i| f32::from_le_bytes(word[i * 4..i * 4 + 4].try_into().unwrap()));
        assert_eq!(
            actual,
            [
                u8::from(expected) as f32,
                u8::from(expected) as f32,
                u8::from(expected) as f32,
                1.
            ],
            "point {point:?}"
        );
    }
}
