// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use crate::{
    material_ranges::GpuMaterialRanges,
    surface_pick::{GpuSurfacePicker, PickFrame, PickScope, PickState, SurfaceProbeKind},
};
use orv_core::volume_ranges::VolumeRanges;

fn field(base: Arc<Volume>, sample: impl Fn(usize, usize, usize) -> (f32, bool)) -> Arc<Volume> {
    let width = base.dimensions()[0];
    let frames = base
        .frames()
        .iter()
        .enumerate()
        .map(|(z, f)| {
            let mut f = (**f).clone();
            let samples: Vec<_> = (0..f.pixels.len())
                .map(|i| sample(i % width, i / width, z))
                .collect();
            f.pixels = samples.iter().map(|s| s.0).collect::<Vec<_>>().into();
            f.validity = Some(
                samples
                    .iter()
                    .map(|s| u8::from(s.1))
                    .collect::<Vec<_>>()
                    .into(),
            );
            Arc::new(f)
        })
        .collect();
    Arc::new(Volume::new(frames).unwrap())
}
fn optical(background: f32, sigma: f32) -> Arc<VolumeMaterial> {
    Arc::new(
        VolumeMaterial::new(&[
            MaterialPoint {
                value: background,
                luminance: 0.2,
                extinction_per_mm: 0.,
            },
            MaterialPoint {
                value: background + 200.,
                luminance: 0.2,
                extinction_per_mm: 0.,
            },
            MaterialPoint {
                value: background + 201.,
                luminance: 0.2,
                extinction_per_mm: sigma,
            },
            MaterialPoint {
                value: 2000.,
                luminance: 0.4,
                extinction_per_mm: sigma,
            },
        ])
        .unwrap(),
    )
}
fn classify(gpu: &Arc<GpuVolumeSource>, material: Arc<VolumeMaterial>) -> Arc<GpuMaterialRanges> {
    GpuMaterialRanges::new(
        gpu.clone(),
        VolumeRanges::build(gpu.volume().clone(), 8, || false).unwrap(),
        material,
        || false,
    )
    .unwrap()
}
fn complete(
    renderer: &mut OpaqueSurfaceRenderer,
    scene: &OpaqueSurfaceScene,
    rows: u32,
) -> Arc<OpaqueSurfaceImage> {
    let size = scene.surface.request().size;
    let mut image = None;
    for y in (0..size[1]).step_by(rows as usize) {
        image = renderer
            .render_rows(scene, 0, y..(y + rows).min(size[1]), || false)
            .unwrap();
        fence(scene.surface.request().surface.source().device());
    }
    image.unwrap()
}

#[test]
fn accelerated_frames_and_probes_match_dense_views_across_features_affines_and_missing_samples() {
    let (d, q) = device(|_| {});
    let mut probed = 0;
    for variant in 0..3 {
        let background = if variant == 2 { 0. } else { -1000. };
        let cpu = field(
            source(
                [35, 27, 19],
                variant > 0,
                variant > 0,
                if variant == 1 { 1e6 } else { 0. },
            ),
            |x, y, z| {
                let inside =
                    (x as f64 - 17.).powi(2) + (y as f64 - 13.).powi(2) + (z as f64 - 9.).powi(2)
                        < 36.;
                (
                    if inside || x == 8 {
                        400. + ((x * 17 + y * 11 + z * 7) % 300) as f32
                    } else {
                        background
                    },
                    variant != 1 || z != 11 && !(x == 9 && (7..17).contains(&y)),
                )
            },
        );
        let gpu = GpuVolumeSource::new(&d, &q, cpu.clone(), || false).unwrap();
        let m = if variant == 2 {
            Arc::new(
                VolumeMaterial::new(&[
                    MaterialPoint {
                        value: 0.,
                        luminance: 0.2,
                        extinction_per_mm: 0.,
                    },
                    MaterialPoint {
                        value: 1.,
                        luminance: 0.2,
                        extinction_per_mm: 0.02,
                    },
                    MaterialPoint {
                        value: 2000.,
                        luminance: 0.4,
                        extinction_per_mm: 0.02,
                    },
                ])
                .unwrap(),
            )
        } else {
            optical(background, 0.02)
        };
        let ranges = classify(&gpu, m.clone());
        assert!(ranges.transparent_tiles() > 0);
        let mut fast =
            OpaqueSurfaceRenderer::new_accelerated(ranges.clone(), wgpu::TextureFormat::Rgba8Unorm)
                .unwrap();
        let mut dense =
            OpaqueSurfaceRenderer::new(gpu.clone(), wgpu::TextureFormat::Rgba8Unorm).unwrap();
        let fast_picker = GpuSurfacePicker::new_accelerated(ranges.clone()).unwrap();
        let dense_picker = GpuSurfacePicker::new(gpu.clone()).unwrap();
        for pattern in 0..3 {
            let owner = MaskOwner::Manual(labels(cpu.clone(), |_, y, _| {
                if pattern == 1 || pattern == 2 && y == 16 {
                    7
                } else {
                    0
                }
            }));
            let mut scene = scene(&gpu, owner.clone(), 7, [37, 29]);
            scene.material = m.clone();
            for (yaw, pitch) in [(0., 0.), (90., 0.), (31., -23.)] {
                scene.volume.camera.yaw_degrees = yaw;
                scene.volume.camera.pitch_degrees = pitch;
                scene.volume.clip = if pitch != 0. {
                    let [lo, hi] = cpu.bounds();
                    Some(PhysicalClip {
                        minimum_mm: std::array::from_fn(|a| lo[a] + (hi[a] - lo[a]) * 0.13),
                        maximum_mm: std::array::from_fn(|a| hi[a] - (hi[a] - lo[a]) * 0.17),
                    })
                } else {
                    None
                };
                scene.surface = project(
                    &gpu,
                    owner.clone(),
                    7,
                    &scene.volume,
                    &scene.navigation,
                    [37, 29],
                );
                for light in [Lighting::UNLIT, Lighting::HEADLIGHT] {
                    scene.lighting = light;
                    scene.surface_lighting = light;
                    let old = complete(&mut dense, &scene, 29);
                    let new = complete(&mut fast, &scene, 16);
                    assert!(Arc::ptr_eq(new.material_ranges().unwrap(), &ranges));
                    assert!(old.material_ranges().is_none());
                    assert_eq!(
                        bytes(&d, &q, new.texture(), 8),
                        bytes(&d, &q, old.texture(), 8),
                        "variant={variant}, pattern={pattern}, yaw={yaw}, light={light:?}"
                    );
                    let scope = PickScope::new().unwrap();
                    let old = PickFrame::new(old, scope.clone());
                    let new = PickFrame::new(new, scope);
                    for pixel in [[0, 0], [9, 7], [18, 14], [25, 19], [36, 28]] {
                        let a = fast_picker
                            .probe(&new, &PickState::from_frame(&new), pixel, || false)
                            .unwrap();
                        let b = dense_picker
                            .probe(&old, &PickState::from_frame(&old), pixel, || false)
                            .unwrap();
                        assert_eq!(a.kind(), b.kind());
                        assert_eq!(
                            a.foreground_transmittance().to_bits(),
                            b.foreground_transmittance().to_bits()
                        );
                        assert_eq!(a.hit().map(|h| h.face), b.hit().map(|h| h.face));
                        assert!(a.is_current(&PickState::from_frame(&new), pixel));
                        probed += 1;
                    }
                }
            }
        }
    }
    assert_eq!(probed, 270);
}

#[test]
fn accelerated_visibility_keeps_the_display_threshold_after_transparent_foreground_runs() {
    let (d, q) = device(|_| {});
    let cpu = field(source([5, 43, 5], false, false, 0.), |_, y, _| {
        (if (20..=22).contains(&y) { 400. } else { -1000. }, true)
    });
    let gpu = GpuVolumeSource::new(&d, &q, cpu.clone(), || false).unwrap();
    let owner = MaskOwner::Manual(labels(cpu, |_, y, _| if y == 32 { 7 } else { 0 }));
    // The final camera needs a fresh projection. Avoid retaining a redundant
    // 2048-side setup projection against the software adapter's 256 MiB budget.
    let mut scene = scene(&gpu, owner.clone(), 7, [1; 2]);
    scene.navigation.pan = [0.; 2];
    scene.material = optical(-1000., 1.);
    scene.surface = project(&gpu, owner, 7, &scene.volume, &scene.navigation, [1; 2]);
    let mut dense =
        OpaqueSurfaceRenderer::new(gpu.clone(), wgpu::TextureFormat::Rgba8Unorm).unwrap();
    let picker = GpuSurfacePicker::new(gpu.clone()).unwrap();
    let unit = PickFrame::new(complete(&mut dense, &scene, 1), PickScope::new().unwrap());
    let measured = picker
        .probe(&unit, &PickState::from_frame(&unit), [0; 2], || false)
        .unwrap()
        .foreground_transmittance();
    // Optical depth scales linearly with extinction. Calibrate the legacy
    // midpoint quadrature once; threshold comparison must preserve that exact
    // display convention, including a possible f32 step-count boundary.
    let unit_depth = -f64::from(measured).ln();
    assert!(unit_depth > 5. && unit_depth < 7.);
    for target in [0.049f64, 0.051, 0.5] {
        scene.material = optical(-1000., (-target.ln() / unit_depth) as f32);
        let ranges = classify(&gpu, scene.material.clone());
        assert!(ranges.transparent_tiles() > 2);
        let mut renderer =
            OpaqueSurfaceRenderer::new_accelerated(ranges.clone(), wgpu::TextureFormat::Rgba8Unorm)
                .unwrap();
        let probe = GpuSurfacePicker::new_accelerated(ranges).unwrap();
        let old = PickFrame::new(complete(&mut dense, &scene, 1), PickScope::new().unwrap());
        let new = PickFrame::new(
            complete(&mut renderer, &scene, 1),
            PickScope::new().unwrap(),
        );
        let a = probe
            .probe(&new, &PickState::from_frame(&new), [0; 2], || false)
            .unwrap();
        let b = picker
            .probe(&old, &PickState::from_frame(&old), [0; 2], || false)
            .unwrap();
        assert_eq!(
            a.kind(),
            if target < 0.05 {
                SurfaceProbeKind::Obscured
            } else {
                SurfaceProbeKind::Hit
            }
        );
        assert_eq!(a.kind(), b.kind());
        assert_eq!(
            a.foreground_transmittance().to_bits(),
            b.foreground_transmittance().to_bits()
        );
        assert!((f64::from(a.foreground_transmittance()) - target).abs() < 0.00005);
    }
}

#[test]
fn classification_and_view_mismatches_refuse_work_without_replacing_completed_frames() {
    let (d, q) = device(|_| {});
    let cpu = field(source([18, 17, 9], false, false, 0.), |_, _, _| {
        (-1000., true)
    });
    let gpu = GpuVolumeSource::new(&d, &q, cpu.clone(), || false).unwrap();
    let owner = MaskOwner::Manual(labels(cpu.clone(), |_, y, _| if y == 12 { 7 } else { 0 }));
    let mut scene = scene(&gpu, owner, 7, [9, 7]);
    scene.material = optical(-1000., 0.03);
    let ranges = classify(&gpu, scene.material.clone());
    let mut renderer =
        OpaqueSurfaceRenderer::new_accelerated(ranges.clone(), wgpu::TextureFormat::Rgba8Unorm)
            .unwrap();
    let old = complete(&mut renderer, &scene, 7);
    let original = bytes(&d, &q, old.texture(), 8);
    let frame = PickFrame::new(old, PickScope::new().unwrap());
    let current = PickState::from_frame(&frame);
    let legacy = GpuSurfacePicker::new(gpu.clone()).unwrap();
    assert!(legacy.probe(&frame, &current, [4, 3], || false).is_err());
    let other = classify(&gpu, scene.material.clone());
    assert!(!Arc::ptr_eq(&other, &ranges));
    assert!(
        GpuSurfacePicker::new_accelerated(other)
            .unwrap()
            .probe(&frame, &current, [4, 3], || false)
            .is_err()
    );
    let mut plain =
        OpaqueSurfaceRenderer::new(gpu.clone(), wgpu::TextureFormat::Rgba8Unorm).unwrap();
    let plain = PickFrame::new(complete(&mut plain, &scene, 7), PickScope::new().unwrap());
    let picker = GpuSurfacePicker::new_accelerated(ranges).unwrap();
    assert!(
        picker
            .probe(&plain, &PickState::from_frame(&plain), [4, 3], || false)
            .is_err()
    );
    let mut moved = current.clone();
    moved.volume.camera.yaw_degrees += 0.25;
    assert!(picker.probe(&frame, &moved, [4, 3], || false).is_err());
    scene.material = material(0.3, 0.2);
    assert!(renderer.render_rows(&scene, 0, 0..7, || false).is_err());
    assert!(renderer.pending[0].is_none());
    let mut changed = current;
    changed.material = scene.material.clone();
    assert!(picker.probe(&frame, &changed, [4, 3], || false).is_err());
    assert_eq!(bytes(&d, &q, frame.image().texture(), 8), original);
    let other_gpu = GpuVolumeSource::new(&d, &q, cpu, || false).unwrap();
    let flags = classify(&other_gpu, scene.material.clone());
    let mut foreign =
        OpaqueSurfaceRenderer::new_accelerated(flags, wgpu::TextureFormat::Rgba8Unorm).unwrap();
    assert!(foreign.render_rows(&scene, 0, 0..7, || false).is_err());
}

#[test]
fn completed_images_and_probe_results_retain_classification_through_cancellation_and_revoke() {
    let (d, q) = device(|_| {});
    let cpu = field(source([18, 17, 9], false, false, 0.), |_, _, _| {
        (-1000., true)
    });
    let gpu = GpuVolumeSource::new(&d, &q, cpu.clone(), || false).unwrap();
    let owner = MaskOwner::Manual(labels(cpu, |_, y, _| if y == 12 { 7 } else { 0 }));
    let mut scene = scene(&gpu, owner, 7, [17, 13]);
    scene.material = optical(-1000., 0.03);
    let cpu_ranges = VolumeRanges::build(gpu.volume().clone(), 8, || false).unwrap();
    fence(&d);
    let cpu_budget = MemoryBudget::volumes();
    let gpu_budget = MemoryBudget::gpu();
    let cpu_base = cpu_budget.used();
    let gpu_base = gpu_budget.used();
    let flags = GpuMaterialRanges::new(
        gpu.clone(),
        cpu_ranges.clone(),
        scene.material.clone(),
        || false,
    )
    .unwrap();
    let weak = Arc::downgrade(&flags);
    let mut renderer =
        OpaqueSurfaceRenderer::new_accelerated(flags.clone(), wgpu::TextureFormat::Rgba8Unorm)
            .unwrap();
    let picker = GpuSurfacePicker::new_accelerated(flags.clone()).unwrap();
    let front = complete(&mut renderer, &scene, 13);
    let original = bytes(&d, &q, front.texture(), 8);
    let frame = PickFrame::new(front, PickScope::new().unwrap());
    let current = PickState::from_frame(&frame);
    let before_cpu = cpu_budget.used();
    let before_gpu = gpu_budget.used();
    assert!(
        renderer
            .render_rows(&scene, 0, 0..2, || false)
            .unwrap()
            .is_none()
    );
    assert!(renderer.render_rows(&scene, 0, 2..4, || true).is_err());
    fence(&d);
    assert!(renderer.pending[0].is_none());
    assert_eq!(cpu_budget.used(), before_cpu);
    assert_eq!(gpu_budget.used(), before_gpu);
    assert_eq!(bytes(&d, &q, frame.image().texture(), 8), original);
    let result = picker.probe(&frame, &current, [8, 6], || false).unwrap();
    drop(renderer);
    drop(picker);
    drop(flags);
    drop(frame);
    assert!(weak.upgrade().is_some());
    assert!(result.is_current(&current, [8, 6]));
    current.scope.revoke();
    assert!(!result.is_current(&current, [8, 6]));
    drop(result);
    drop(current);
    fence(&d);
    assert!(weak.upgrade().is_none());
    assert_eq!(cpu_budget.used(), cpu_base);
    assert_eq!(gpu_budget.used(), gpu_base);
    let flags =
        GpuMaterialRanges::new(gpu.clone(), cpu_ranges, scene.material.clone(), || false).unwrap();
    let mut renderer =
        OpaqueSurfaceRenderer::new_accelerated(flags.clone(), wgpu::TextureFormat::Rgba8Unorm)
            .unwrap();
    let picker = GpuSurfacePicker::new_accelerated(flags).unwrap();
    let image = complete(&mut renderer, &scene, 13);
    let frame = PickFrame::new(image, PickScope::new().unwrap());
    let current = PickState::from_frame(&frame);
    let before_cpu = cpu_budget.used();
    let before_gpu = gpu_budget.used();
    let calls = Cell::new(0);
    assert!(
        picker
            .probe(&frame, &current, [8, 6], || {
                let n = calls.get();
                calls.set(n + 1);
                if n == 2 {
                    current.scope.revoke();
                }
                false
            })
            .is_err()
    );
    assert!(!current.scope.is_active());
    fence(&d);
    assert_eq!(cpu_budget.used(), before_cpu);
    assert_eq!(gpu_budget.used(), before_gpu);
}

#[test]
fn optional_acceleration_limits_keep_legacy_and_zero_flag_frames_available() {
    for limit in 0..3 {
        let (d, q) = device(|l| match limit {
            0 => l.max_bind_groups = 1,
            1 => l.max_sampled_textures_per_shader_stage = 4,
            _ => l.max_uniform_buffers_per_shader_stage = 2,
        });
        let cpu = field(source([9, 9, 3], false, false, 0.), |_, _, _| {
            (-1000., true)
        });
        let gpu = GpuVolumeSource::new(&d, &q, cpu.clone(), || false).unwrap();
        let owner = MaskOwner::Manual(labels(cpu, |_, y, _| if y == 6 { 7 } else { 0 }));
        let mut scene = scene(&gpu, owner, 7, [3; 2]);
        scene.material = optical(-1000., 0.03);
        let flags = classify(&gpu, scene.material.clone());
        assert!(flags.transparent_tiles() > 0);
        assert!(
            OpaqueSurfaceRenderer::new_accelerated(flags.clone(), wgpu::TextureFormat::Rgba8Unorm)
                .is_err()
        );
        assert!(GpuSurfacePicker::new_accelerated(flags).is_err());
        let mut legacy =
            OpaqueSurfaceRenderer::new(gpu.clone(), wgpu::TextureFormat::Rgba8Unorm).unwrap();
        let picker = GpuSurfacePicker::new(gpu.clone()).unwrap();
        let image = complete(&mut legacy, &scene, 3);
        assert!(image.material_ranges().is_none());
        let frame = PickFrame::new(image, PickScope::new().unwrap());
        picker
            .probe(&frame, &PickState::from_frame(&frame), [1; 2], || false)
            .unwrap();
        scene.material = material(0.2, 0.2);
        let flags = classify(&gpu, scene.material.clone());
        assert_eq!(flags.transparent_tiles(), 0);
        let mut fallback =
            OpaqueSurfaceRenderer::new_accelerated(flags.clone(), wgpu::TextureFormat::Rgba8Unorm)
                .unwrap();
        let picker = GpuSurfacePicker::new_accelerated(flags).unwrap();
        let image = complete(&mut fallback, &scene, 3);
        assert!(image.material_ranges().is_none());
        let old = complete(&mut legacy, &scene, 3);
        assert_eq!(
            bytes(&d, &q, image.texture(), 8),
            bytes(&d, &q, old.texture(), 8)
        );
        let frame = PickFrame::new(image, PickScope::new().unwrap());
        picker
            .probe(&frame, &PickState::from_frame(&frame), [1; 2], || false)
            .unwrap();
    }
}

#[test]
fn accelerated_supplied_fractional_regions_keep_high_ids_unknown_boundaries_and_source_identity() {
    let (d, q) = device(|_| {});
    let cpu = field(source([7, 5, 6], false, false, 0.), |_, _, _| {
        (-1000., true)
    });
    let gpu = GpuVolumeSource::new(&d, &q, cpu.clone(), || false).unwrap();
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
    scene.material = optical(-1000., 0.03);
    scene.volume.camera.pitch_degrees = 90.;
    scene.surface = project(&gpu, owner, 65535, &scene.volume, &scene.navigation, [1; 2]);
    let flags = classify(&gpu, scene.material.clone());
    let mut renderer =
        OpaqueSurfaceRenderer::new_accelerated(flags.clone(), wgpu::TextureFormat::Rgba8Unorm)
            .unwrap();
    let picker = GpuSurfacePicker::new_accelerated(flags).unwrap();
    let frame = PickFrame::new(
        complete(&mut renderer, &scene, 1),
        PickScope::new().unwrap(),
    );
    let result = picker
        .probe(&frame, &PickState::from_frame(&frame), [0; 2], || false)
        .unwrap();
    assert_eq!(result.kind(), SurfaceProbeKind::Hit);
    let hit = result.hit().unwrap();
    assert_eq!(hit.key.number, 65535);
    assert_eq!(hit.face.side() as u8, 5);
    assert_eq!(result.foreground_transmittance(), 1.);
    let mut dense = OpaqueSurfaceRenderer::new(gpu, wgpu::TextureFormat::Rgba8Unorm).unwrap();
    assert_eq!(
        bytes(&d, &q, frame.image().texture(), 8),
        bytes(&d, &q, complete(&mut dense, &scene, 1).texture(), 8)
    );
}

#[test]
fn accelerated_complete_2048_image_and_tiled_refinement_keep_the_requested_pixel_grid() {
    let (d, q) = device(|_| {});
    let cpu = field(source([18, 17, 9], true, true, 0.), |x, y, z| {
        (
            if x == 8 && (5..13).contains(&y) {
                400. + z as f32
            } else {
                -1000.
            },
            true,
        )
    });
    let gpu = GpuVolumeSource::new(&d, &q, cpu.clone(), || false).unwrap();
    let owner = MaskOwner::Manual(labels(cpu, |_, y, _| if y == 12 { 7 } else { 0 }));
    // The final camera needs a fresh projection. Avoid retaining a redundant
    // 2048-side setup projection against the software adapter's 256 MiB budget.
    let mut scene = scene(&gpu, owner.clone(), 7, [1; 2]);
    scene.material = optical(-1000., 0.03);
    scene.volume.camera.yaw_degrees = 31.;
    scene.volume.camera.pitch_degrees = -17.;
    // A software adapter can still be executing the setup projection, whose
    // admission is released only when that work completes.
    d.poll(wgpu::PollType::Wait {
        submission_index: None,
        timeout: Some(std::time::Duration::from_secs(30)),
    })
    .unwrap();
    scene.surface = project(&gpu, owner, 7, &scene.volume, &scene.navigation, [2048; 2]);
    let flags = classify(&gpu, scene.material.clone());
    let mut renderer =
        OpaqueSurfaceRenderer::new_accelerated(flags.clone(), wgpu::TextureFormat::Rgba8Unorm)
            .unwrap();
    let image = complete(&mut renderer, &scene, 16);
    assert_eq!(
        [image.texture().width(), image.texture().height()],
        [2048; 2]
    );
    let mut dense =
        OpaqueSurfaceRenderer::new(gpu.clone(), wgpu::TextureFormat::Rgba8Unorm).unwrap();
    let old = complete(&mut dense, &scene, 2048);
    assert_eq!(
        bytes(&d, &q, image.texture(), 8),
        bytes(&d, &q, old.texture(), 8)
    );
    let frame = PickFrame::new(image, PickScope::new().unwrap());
    let picker = GpuSurfacePicker::new_accelerated(flags).unwrap();
    let old = PickFrame::new(old, frame.scope().clone());
    let legacy = GpuSurfacePicker::new(gpu).unwrap();
    for pixel in [[0, 0], [512, 512], [1024, 1024], [1536, 1536], [2047, 2047]] {
        let a = picker
            .probe(&frame, &PickState::from_frame(&frame), pixel, || false)
            .unwrap();
        let b = legacy
            .probe(&old, &PickState::from_frame(&old), pixel, || false)
            .unwrap();
        assert_eq!(a.kind(), b.kind());
        assert_eq!(a.hit().map(|h| h.face), b.hit().map(|h| h.face));
        assert_eq!(
            a.foreground_transmittance().to_bits(),
            b.foreground_transmittance().to_bits()
        );
    }
}
