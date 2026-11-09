// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use crate::surface_pick::{
    GpuSurfacePicker, MIN_PICK_TRANSMITTANCE, PickFrame, PickScope, PickState, SurfaceProbeKind,
};

fn completed(gpu: &Arc<GpuVolumeSource>, scene: &OpaqueSurfaceScene) -> PickFrame {
    let mut renderer =
        OpaqueSurfaceRenderer::new(gpu.clone(), wgpu::TextureFormat::Rgba8Unorm).unwrap();
    let image = renderer
        .render_rows(scene, 0, 0..scene.surface.request().size[1], || false)
        .unwrap()
        .unwrap();
    PickFrame::new(image, PickScope::new().unwrap())
}
fn transmission(scene: &OpaqueSurfaceScene, pixel: [u32; 2]) -> Option<(CellFace, f64)> {
    let request = scene.surface.request();
    let hit = reference(request, pixel)?;
    let source = request.surface.source().volume();
    let rays = request
        .camera
        .frame(
            &request.display,
            f64::from(request.size[0]) / f64::from(request.size[1]),
        )
        .unwrap();
    let (origin, direction) = rays.ray(std::array::from_fn(|a| {
        (f64::from(pixel[a]) + 0.5) / f64::from(request.size[a])
    }));
    let mut interval = range(
        source.world_to_voxel(origin),
        source.world_vector_to_voxel(direction),
        [0.; 3],
        source.dimensions().map(|d| (d - 1) as f64),
    );
    if let Some(clip) = &scene.volume.clip {
        interval = interval
            .zip(range(origin, direction, clip.minimum_mm, clip.maximum_mm))
            .map(|(a, b)| [a[0].max(b[0]), a[1].min(b[1])]);
    }
    let offset = dot(
        std::array::from_fn(|a| origin[a] - source.center()[a]),
        direction,
    );
    let length = interval.map_or(0., |r| (r[1].min(hit.depth - offset) - r[0]).max(0.));
    Some((
        hit.face,
        (-scene.material.sample(10.).unwrap().extinction_per_mm * length).exp(),
    ))
}

#[test]
fn one_pixel_probes_match_completed_pixels_physical_membership_and_foreground_visibility() {
    let (device, queue) = device(|_| {});
    for shear in [false, true] {
        let cpu = source([7, 5, 6], shear, shear, if shear { 1000000. } else { 0. });
        let gpu = GpuVolumeSource::new(&device, &queue, cpu.clone(), || false).unwrap();
        let picker = GpuSurfacePicker::new(gpu.clone()).unwrap();
        for pattern in 0..3 {
            let owner = MaskOwner::Manual(labels(cpu.clone(), |_, y, _| {
                if pattern == 1 || (pattern == 2 && y == 2) {
                    7
                } else {
                    0
                }
            }));
            let mut scene = scene(&gpu, owner, 7, [31, 23]);
            if shear {
                scene.volume.camera.yaw_degrees = 31.;
                scene.volume.camera.pitch_degrees = -23.;
                scene.volume.camera.roll_degrees = 11.;
                let owner = scene.surface.request().surface.surface().owner().clone();
                scene.surface = project(&gpu, owner, 7, &scene.volume, &scene.navigation, [31, 23]);
            }
            for sigma in [0., 0.4, 2.] {
                scene.material = material(0.2, sigma);
                let frame = completed(&gpu, &scene);
                let current = PickState::from_frame(&frame);
                for pixel in [
                    [0, 0],
                    [7, 7],
                    [11, 9],
                    [15, 11],
                    [19, 13],
                    [23, 17],
                    [30, 22],
                ] {
                    let result = picker
                        .probe(&frame, &current, pixel, || false)
                        .unwrap_or_else(|error| {
                            panic!("{shear}/{pattern}/{sigma}/{pixel:?}: {error}")
                        });
                    assert!(result.is_current(&current, pixel));
                    assert_eq!(result.pixel(), pixel);
                    if let Some((face, expected)) = transmission(&scene, pixel) {
                        assert!(
                            (f64::from(result.foreground_transmittance()) - expected).abs() < 0.001
                        );
                        let kind = if expected < f64::from(MIN_PICK_TRANSMITTANCE) {
                            SurfaceProbeKind::Obscured
                        } else {
                            SurfaceProbeKind::Hit
                        };
                        assert_eq!(result.kind(), kind, "{shear}/{pattern}/{sigma}/{pixel:?}");
                        if kind == SurfaceProbeKind::Hit {
                            let hit = result.hit().unwrap();
                            assert_eq!(hit.face, face);
                            assert_eq!(&hit.key, scene.surface.key());
                            assert_eq!(&hit.revision, scene.surface.revision());
                        } else {
                            assert!(result.hit().is_none());
                        }
                    } else {
                        assert_eq!(result.kind(), SurfaceProbeKind::Empty);
                        assert!(result.hit().is_none());
                    }
                }
            }
        }
    }
}

#[test]
fn quarter_turn_source_nodes_match_f64_optics_without_invalid_neighbor_bleeding() {
    let (device, queue) = device(|_| {});
    let cpu = source([7, 5, 6], false, false, 0.);
    let cpu = Arc::new(
        Volume::new(
            cpu.frames()
                .iter()
                .map(|f| {
                    let mut f = (**f).clone();
                    f.validity = Some(
                        (0..35)
                            .map(|i| u8::from(i % 7 != 0 && i / 7 != 0))
                            .collect::<Vec<_>>()
                            .into(),
                    );
                    Arc::new(f)
                })
                .collect(),
        )
        .unwrap(),
    );
    let owner = MaskOwner::Manual(labels(cpu.clone(), |_, _, z| if z == 2 { 7 } else { 0 }));
    let gpu = GpuVolumeSource::new(&device, &queue, cpu.clone(), || false).unwrap();
    let mut scene = scene(&gpu, owner.clone(), 7, [1; 2]);
    scene.navigation.pan = [0.; 2];
    scene.volume.camera.target_mm = cpu.voxel_to_world([1., 1., 2.5]);
    let mut renderer =
        OpaqueSurfaceRenderer::new(gpu.clone(), wgpu::TextureFormat::Rgba8Unorm).unwrap();
    for yaw in [-180., 0., 90., 270.] {
        for roll in [-90., 0., 180.] {
            scene.volume.camera.yaw_degrees = yaw;
            scene.volume.camera.pitch_degrees = 90.;
            scene.volume.camera.roll_degrees = roll;
            scene.surface = project(
                &gpu,
                owner.clone(),
                7,
                &scene.volume,
                &scene.navigation,
                [1; 2],
            );
            let image = renderer
                .render_rows(&scene, 0, 0..1, || false)
                .unwrap()
                .unwrap();
            compare(&image);
            let expected = 0.2 + (0.85 - 0.2) * (-0.4 * 3f64).exp();
            assert!((linear(&image)[0][0] - expected).abs() < 0.001);
        }
    }
}

#[test]
fn scopes_cursor_revisions_and_pending_desired_states_reject_stale_results() {
    let (device, queue) = device(|_| {});
    let cpu = source([7, 5, 6], false, false, 0.);
    let map = labels(cpu.clone(), |_, y, _| if y == 2 { 7 } else { 0 });
    let gpu = GpuVolumeSource::new(&device, &queue, cpu.clone(), || false).unwrap();
    let scene = scene(&gpu, MaskOwner::Manual(map.clone()), 7, [31, 23]);
    let frame = completed(&gpu, &scene);
    let current = PickState::from_frame(&frame);
    let picker = GpuSurfacePicker::new(gpu.clone()).unwrap();
    let pixel = [15, 11];
    let result = picker.probe(&frame, &current, pixel, || false).unwrap();
    assert_eq!(result.kind(), SurfaceProbeKind::Hit);
    assert!(!result.is_current(&current, [16, 11]));
    for change in 0..15 {
        let mut changed = current.clone();
        match change {
            0 => changed.scope = PickScope::new().unwrap(),
            1 => changed.number = 9,
            2 => changed.navigation.pan[0] += 0.01,
            3 => changed.navigation.window.center += 1.,
            4 => changed.volume.camera.yaw_degrees += 1.,
            5 => {
                changed.volume.clip = Some(PhysicalClip {
                    minimum_mm: [-1.; 3],
                    maximum_mm: [1.; 3],
                })
            }
            6 => changed.volume.quality = Quality::Interactive,
            7 => changed.material = material(0.2, 0.4),
            8 => changed.lighting = Lighting::HEADLIGHT,
            9 => changed.surface_lighting = Lighting::HEADLIGHT,
            10 => changed.surface_luminance = 0.3,
            11 => changed.size[0] += 1,
            12 => {
                changed.map =
                    MaskOwner::Manual(labels(cpu.clone(), |_, y, _| if y == 2 { 7 } else { 0 }))
            }
            13 => {
                let mut edited = map.try_clone(|| false).unwrap();
                edited.paint(0, &[[1., 1.]], 1, 7, || false).unwrap();
                changed.map = MaskOwner::Manual(Arc::new(edited));
            }
            _ => {
                changed.map =
                    MaskOwner::Manual(labels(source([7, 5, 6], false, false, 0.), |_, y, _| {
                        if y == 2 { 7 } else { 0 }
                    }))
            }
        }
        assert!(!changed.matches(&frame), "change{change}");
        assert!(!result.is_current(&changed, pixel));
        assert!(picker.probe(&frame, &changed, pixel, || false).is_err());
    }
    for point in [[31, 0], [0, 23], [u32::MAX, 0]] {
        assert!(picker.probe(&frame, &current, point, || false).is_err());
    }
    let other = GpuVolumeSource::new(&device, &queue, cpu, || false).unwrap();
    let foreign = GpuSurfacePicker::new(other).unwrap();
    assert!(foreign.probe(&frame, &current, pixel, || false).is_err());
    assert!(result.is_current(&current, pixel));
    frame.scope().revoke();
    assert!(!result.is_current(&current, pixel));
    assert!(picker.probe(&frame, &current, pixel, || false).is_err());
}

#[test]
fn visibility_threshold_and_exact_edge_contacts_never_authorize_hidden_or_ambiguous_hits() {
    let (device, queue) = device(|_| {});
    let cpu = source([7, 5, 6], false, false, 0.);
    let gpu = GpuVolumeSource::new(&device, &queue, cpu.clone(), || false).unwrap();
    let picker = GpuSurfacePicker::new(gpu.clone()).unwrap();
    let mut scene = scene(
        &gpu,
        MaskOwner::Manual(labels(cpu.clone(), |_, y, _| if y == 2 { 7 } else { 0 })),
        7,
        [1; 2],
    );
    for target in [0.049f64, 0.051, 0.5] {
        scene.material = material(0.2, (-target.ln() / 2.25) as f32);
        let frame = completed(&gpu, &scene);
        let current = PickState::from_frame(&frame);
        let result = picker.probe(&frame, &current, [0; 2], || false).unwrap();
        assert!((f64::from(result.foreground_transmittance()) - target).abs() < 0.00001);
        assert_eq!(
            result.kind(),
            if target < 0.05 {
                SurfaceProbeKind::Obscured
            } else {
                SurfaceProbeKind::Hit
            }
        );
        assert_eq!(result.hit().is_some(), target > 0.05);
    }
    let owner = MaskOwner::Manual(labels(cpu.clone(), |x, y, z| {
        if [x, y, z] == [3, 2, 2] { 7 } else { 0 }
    }));
    scene.navigation.pan = [0.; 2];
    scene.volume.camera.target_mm = cpu.voxel_to_world([2.5, 2., 2.]);
    scene.surface = project(
        &gpu,
        owner.clone(),
        7,
        &scene.volume,
        &scene.navigation,
        [1; 2],
    );
    let frame = completed(&gpu, &scene);
    let result = picker
        .probe(&frame, &PickState::from_frame(&frame), [0; 2], || false)
        .unwrap();
    assert_eq!(result.kind(), SurfaceProbeKind::Ambiguous);
    assert!(result.hit().is_none());
    scene.volume.camera.target_mm = cpu.voxel_to_world([2.5 + 1e-5, 2., 2.]);
    scene.surface = project(&gpu, owner, 7, &scene.volume, &scene.navigation, [1; 2]);
    let frame = completed(&gpu, &scene);
    let result = picker
        .probe(&frame, &PickState::from_frame(&frame), [0; 2], || false)
        .unwrap();
    assert_eq!(result.kind(), SurfaceProbeKind::Hit);
}

#[test]
fn high_ids_open_boundaries_missing_stencils_and_real_clipped_backfaces_are_probed_in_place() {
    let (device, queue) = device(|_| {});
    let cpu = source([7, 5, 6], false, false, 0.);
    let gpu = GpuVolumeSource::new(&device, &queue, cpu.clone(), || false).unwrap();
    let picker = GpuSurfacePicker::new(gpu.clone()).unwrap();
    let mut payload = vec![0; 70];
    payload[..35].fill(100);
    let segments = Arc::new(
        Segments::from_fractional(
            cpu.clone(),
            &[65535],
            &[(65535, 1), (65535, 2)],
            &payload,
            FractionalEncoding {
                maximum: 200,
                kind: FractionalKind::Probability,
            },
            Some(true),
            || false,
        )
        .unwrap(),
    );
    let owner = MaskOwner::Supplied(segments);
    let mut scene = scene(&gpu, owner.clone(), 65535, [1; 2]);
    scene.volume.camera.pitch_degrees = 90.;
    scene.surface = project(&gpu, owner, 65535, &scene.volume, &scene.navigation, [1; 2]);
    let frame = completed(&gpu, &scene);
    let result = picker
        .probe(&frame, &PickState::from_frame(&frame), [0; 2], || false)
        .unwrap();
    assert_eq!(result.kind(), SurfaceProbeKind::Hit);
    let hit = result.hit().unwrap();
    assert_eq!(hit.key.number, 65535);
    assert_eq!(hit.face.side() as u8, 5);
    let owner = MaskOwner::Manual(labels(cpu.clone(), |_, _, _| 7));
    scene = super::scene(&gpu, owner.clone(), 7, [1; 2]);
    for (clip_y, expected) in [
        (1.1, SurfaceProbeKind::Hit),
        (6.75, SurfaceProbeKind::Ambiguous),
    ] {
        scene.volume.clip = Some(PhysicalClip {
            minimum_mm: [-10., clip_y, -10.],
            maximum_mm: [20.; 3],
        });
        scene.surface = project(
            &gpu,
            owner.clone(),
            7,
            &scene.volume,
            &scene.navigation,
            [1; 2],
        );
        let frame = completed(&gpu, &scene);
        let result = picker
            .probe(&frame, &PickState::from_frame(&frame), [0; 2], || false)
            .unwrap();
        assert_eq!(result.kind(), expected);
        if let Some(hit) = result.hit() {
            assert_eq!(hit.face.side() as u8, 3);
            assert!(
                (f64::from(result.foreground_transmittance()) - (-0.4 * 4.9f64).exp()).abs()
                    < 0.001
            );
        }
    }
    let cpu = Arc::new(
        Volume::new(
            cpu.frames()
                .iter()
                .enumerate()
                .map(|(z, f)| {
                    let mut f = (**f).clone();
                    f.validity = (z == 0).then(|| vec![0; 35].into());
                    Arc::new(f)
                })
                .collect(),
        )
        .unwrap(),
    );
    let owner = MaskOwner::Manual(labels(cpu.clone(), |_, _, z| if z == 1 { 7 } else { 0 }));
    let gpu = GpuVolumeSource::new(&device, &queue, cpu, || false).unwrap();
    let mut scene = super::scene(&gpu, owner.clone(), 7, [1; 2]);
    scene.volume.camera.pitch_degrees = 90.;
    scene.navigation.pan = [0.; 2];
    scene.surface = project(&gpu, owner, 7, &scene.volume, &scene.navigation, [1; 2]);
    let frame = completed(&gpu, &scene);
    let picker = GpuSurfacePicker::new(gpu).unwrap();
    let result = picker
        .probe(&frame, &PickState::from_frame(&frame), [0; 2], || false)
        .unwrap();
    assert_eq!(result.kind(), SurfaceProbeKind::Hit);
    assert!((f64::from(result.foreground_transmittance()) - (-0.4f64).exp()).abs() < 0.001);
}

#[test]
fn cancellation_and_admission_restore_resources_without_reading_a_different_view() {
    let (device, queue) = device(|_| {});
    let cpu = source([7, 5, 6], false, false, 0.);
    let gpu = GpuVolumeSource::new(&device, &queue, cpu.clone(), || false).unwrap();
    let scene = scene(
        &gpu,
        MaskOwner::Manual(labels(cpu, |_, y, _| if y == 2 { 7 } else { 0 })),
        7,
        [31, 23],
    );
    let frame = completed(&gpu, &scene);
    let current = PickState::from_frame(&frame);
    let picker = GpuSurfacePicker::new(gpu).unwrap();
    let point = [15, 11];
    fence(&device);
    let cpu_used = MemoryBudget::volumes().used();
    let gpu_used = MemoryBudget::gpu().used();
    assert!(picker.probe(&frame, &current, point, || true).is_err());
    let calls = Cell::new(0);
    assert!(
        picker
            .probe(&frame, &current, point, || {
                calls.set(calls.get() + 1);
                calls.get() >= 3
            })
            .is_err()
    );
    assert_eq!(calls.get(), 3);
    fence(&device);
    assert_eq!(MemoryBudget::volumes().used(), cpu_used);
    assert_eq!(MemoryBudget::gpu().used(), gpu_used);
    for cpu in [false, true] {
        let budget = if cpu {
            MemoryBudget::volumes()
        } else {
            MemoryBudget::gpu()
        };
        let limit = budget.limit();
        budget.set_limit(0);
        let result = picker.probe(&frame, &current, point, || false);
        budget.set_limit(limit);
        assert!(result.is_err());
    }
    let budget = MemoryBudget::volumes();
    let limit = budget.limit();
    budget.set_limit(0);
    let scope = PickScope::new();
    budget.set_limit(limit);
    assert!(scope.is_err());
    let result = picker.probe(&frame, &current, point, || false).unwrap();
    assert_eq!(result.kind(), SurfaceProbeKind::Hit);
    assert!(MemoryBudget::volumes().used() > cpu_used);
    drop(result);
    fence(&device);
    assert_eq!(MemoryBudget::volumes().used(), cpu_used);
    assert_eq!(MemoryBudget::gpu().used(), gpu_used);
}

#[test]
fn revoking_a_scope_during_probe_prevents_a_completed_hit_from_returning() {
    let (device, queue) = device(|_| {});
    let cpu = source([7, 5, 6], false, false, 0.);
    let gpu = GpuVolumeSource::new(&device, &queue, cpu.clone(), || false).unwrap();
    let scene = scene(
        &gpu,
        MaskOwner::Manual(labels(cpu, |_, y, _| if y == 2 { 7 } else { 0 })),
        7,
        [31, 23],
    );
    let frame = completed(&gpu, &scene);
    let current = PickState::from_frame(&frame);
    let picker = GpuSurfacePicker::new(gpu).unwrap();
    let calls = Cell::new(0);
    let result = picker.probe(&frame, &current, [15, 11], || {
        calls.set(calls.get() + 1);
        if calls.get() == 3 {
            frame.scope().revoke();
        }
        false
    });
    assert!(result.is_err());
    assert!(!frame.scope().is_active());
    fence(&device);
}

#[test]
fn corrupt_cells_depth_and_completed_color_cannot_pass_local_probe_checks() {
    let (device, queue) = device(|_| {});
    let cpu = source([7, 5, 6], false, false, 0.);
    let gpu = GpuVolumeSource::new(&device, &queue, cpu.clone(), || false).unwrap();
    let picker = GpuSurfacePicker::new(gpu.clone()).unwrap();
    for fault in 0..3 {
        let scene = scene(
            &gpu,
            MaskOwner::Manual(labels(cpu.clone(), |_, y, _| if y == 2 { 7 } else { 0 })),
            7,
            [13, 11],
        );
        let frame = completed(&gpu, &scene);
        let current = PickState::from_frame(&frame);
        match fault {
            0 => clear(
                &device,
                &queue,
                scene.surface.cells(),
                wgpu::Color {
                    r: 0.,
                    g: 0.,
                    b: 0.,
                    a: 8.,
                },
            ),
            1 => clear(
                &device,
                &queue,
                scene.surface.depth(),
                wgpu::Color {
                    r: 1000.,
                    g: 0.,
                    b: 0.,
                    a: 0.,
                },
            ),
            _ => paint_invalid_completed(&device, &queue, frame.image().texture()),
        }
        assert!(
            picker.probe(&frame, &current, [6, 5], || false).is_err(),
            "fault{fault}"
        );
    }
}
fn paint_invalid_completed(device: &wgpu::Device, queue: &wgpu::Queue, texture: &wgpu::Texture) {
    let shader=device.create_shader_module(wgpu::ShaderModuleDescriptor {label:Some("deliberately inconsistent generated completed pixel"),source:wgpu::ShaderSource::Wgsl("@group(0) @binding(0) var output:texture_storage_2d<rgba16float,write>;@compute @workgroup_size(8,8,1) fn main(@builtin(global_invocation_id) p:vec3u) {if any(p.xy>=textureDimensions(output)) {return;}textureStore(output,vec2i(p.xy),vec4f(.99,.99,.99,1.));}".into())});
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: None,
        layout: None,
        module: &shader,
        entry_point: Some("main"),
        compilation_options: Default::default(),
        cache: None,
    });
    let view = texture.create_view(&Default::default());
    let bindings = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::TextureView(&view),
        }],
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_compute_pass(&Default::default());
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bindings, &[]);
        pass.dispatch_workgroups(texture.width().div_ceil(8), texture.height().div_ceil(8), 1);
    }
    queue.submit([encoder.finish()]);
}

#[test]
fn large_viewport_probes_use_original_pixel_coordinates_and_refuse_device_limits() {
    let (device, queue) = device(|_| {});
    let cpu = source([7, 5, 6], false, false, 0.);
    let gpu = GpuVolumeSource::new(&device, &queue, cpu.clone(), || false).unwrap();
    let scene = scene(
        &gpu,
        MaskOwner::Manual(labels(cpu, |_, y, _| if y == 2 { 7 } else { 0 })),
        7,
        [2048; 2],
    );
    let frame = completed(&gpu, &scene);
    let current = PickState::from_frame(&frame);
    let picker = GpuSurfacePicker::new(gpu).unwrap();
    for pixel in [[0, 0], [913, 777], [1024, 1024], [1234, 1111], [2047, 2047]] {
        let result = picker.probe(&frame, &current, pixel, || false).unwrap();
        if let Some((face, t)) = transmission(&scene, pixel) {
            assert_eq!(result.kind(), SurfaceProbeKind::Hit);
            assert_eq!(result.hit().unwrap().face, face);
            assert!((f64::from(result.foreground_transmittance()) - t).abs() < 0.001);
        } else {
            assert_eq!(result.kind(), SurfaceProbeKind::Empty);
        }
    }
    for choice in 0..4 {
        let (device, queue) = self::device(|limits| match choice {
            0 => limits.max_storage_buffers_per_shader_stage = 0,
            1 => limits.max_compute_workgroups_per_dimension = 0,
            2 => limits.max_sampled_textures_per_shader_stage = 3,
            _ => limits.max_uniform_buffer_binding_size = 256,
        });
        let cpu = source([7, 5, 6], false, false, 0.);
        let gpu = GpuVolumeSource::new(&device, &queue, cpu, || false).unwrap();
        assert!(GpuSurfacePicker::new(gpu.clone()).is_err());
        if choice < 3 {
            assert!(GpuSurfaceProjector::new(gpu).is_ok());
        }
    }
}
