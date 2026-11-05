// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use crate::{
    section_volume::pick::{SectionPickFrame, SectionPickState},
    surface_pick::{MIN_PICK_TRANSMITTANCE, PickScope, SurfaceProbeKind},
    surface_projection::GpuSurfaceProjector,
};

fn complete(renderer: &mut SectionRenderer, scene: &SectionScene) -> SectionPickFrame {
    let image = renderer
        .render_rows(scene, 0, 0..scene.size[1], || false)
        .unwrap()
        .unwrap();
    SectionPickFrame::new(image, PickScope::new().unwrap())
}
#[test]
fn section_probes_agree_with_completed_pixels_faces_planes_and_source_attenuation() {
    let (d, q) = device(None);
    let before = [MemoryBudget::volumes().used(), MemoryBudget::gpu().used()];
    {
        for missing in [false, true] {
            let cpu = source([17, 15, 9], 1, missing, true);
            let gpu = GpuVolumeSource::new(&d, &q, cpu.clone(), || false).unwrap();
            let geometry = surface::geometry(gpu.clone());
            let projector = GpuSurfaceProjector::new(gpu.clone()).unwrap();
            let mut renderer =
                SectionRenderer::new(gpu.clone(), wgpu::TextureFormat::Rgba8UnormSrgb).unwrap();
            let mut scene = scene(gpu);
            scene.volume.camera.yaw_degrees = 23.;
            scene.volume.camera.pitch_degrees = -17.;
            scene.volume.camera.roll_degrees = 31.;
            scene.volume.quality = Quality::Refined;
            scene.surface = Some(SectionSurface {
                image: surface::project(&projector, geometry, &scene),
                luminance: 0.8,
                lighting: Lighting::HEADLIGHT,
            });
            let all_planes = scene.planes.clone();
            let mut counts = [0; 4];
            for (planes, sigma) in [(false, 0.), (true, 0.), (true, 0.8)] {
                scene.planes = if planes {
                    all_planes.clone()
                } else {
                    [None, None, None]
                };
                scene.material = material(sigma);
                let frame = complete(&mut renderer, &scene);
                let current = SectionPickState::from_frame(&frame).unwrap();
                assert!(current.matches(&frame));
                let rays = scene
                    .volume
                    .camera
                    .frame(&scene.navigation, 37. / 29.)
                    .unwrap();
                for y in (1..29).step_by(3) {
                    for x in (1..37).step_by(3) {
                        let pixel = [x, y];
                        let (origin, direction) =
                            rays.ray([(f64::from(x) + 0.5) / 37., (f64::from(y) + 0.5) / 29.]);
                        let face =
                            surface::boundary(scene.surface.as_ref().unwrap(), origin, direction);
                        let mut actor = face.map(|(depth, _)| (depth, true));
                        for p in scene.planes.iter().flatten() {
                            if let Some((depth, _)) =
                                sample(p, origin, direction, scene.volume.clip.as_ref())
                                && actor.is_none_or(|(t, _)| depth < t)
                            {
                                actor = Some((depth, false));
                            }
                        }
                        let probe = renderer
                            .probe(&frame, &current, pixel, || false)
                            .unwrap_or_else(|e| {
                                panic!("{missing}/{planes}/{sigma}/{pixel:?}: {e}")
                            });
                        assert!(probe.is_current(&current, pixel));
                        assert_eq!(probe.pixel(), pixel);
                        let length = cpu.clipped_ray_interval(origin, direction, None).map_or(
                            0.,
                            |mut range| {
                                if let Some((depth, _)) = actor {
                                    range[1] = range[1].min(depth);
                                }
                                valid_length(&cpu, origin, direction, range)
                            },
                        );
                        let expected_t = (-f64::from(sigma) * length).exp();
                        assert!(
                            (f64::from(probe.foreground_transmittance()) - expected_t).abs()
                                < 0.001,
                            "transmission {missing}/{planes}/{sigma}/{pixel:?}"
                        );
                        if probe.kind() == SurfaceProbeKind::Ambiguous {
                            assert!(probe.hit().is_none());
                            counts[3] += 1;
                            continue;
                        }
                        let expected = if face.is_none() {
                            SurfaceProbeKind::Empty
                        } else if actor.is_some_and(|(_, face)| !face)
                            || expected_t < f64::from(MIN_PICK_TRANSMITTANCE)
                        {
                            SurfaceProbeKind::Obscured
                        } else {
                            SurfaceProbeKind::Hit
                        };
                        assert_eq!(
                            probe.kind(),
                            expected,
                            "{missing}/{planes}/{sigma}/{pixel:?}"
                        );
                        match expected {
                            SurfaceProbeKind::Hit => {
                                let hit = probe.hit().unwrap();
                                assert_eq!(hit.key.number, 1);
                                assert_eq!(hit.key, *scene.surface.as_ref().unwrap().image.key());
                                counts[0] += 1;
                            }
                            SurfaceProbeKind::Empty => {
                                assert!(probe.hit().is_none());
                                counts[1] += 1;
                            }
                            SurfaceProbeKind::Obscured => {
                                assert!(probe.hit().is_none());
                                counts[2] += 1;
                            }
                            SurfaceProbeKind::Ambiguous => unreachable!(),
                        }
                    }
                }
            }
            assert!(
                counts[0] > 5 && counts[1] > 50 && counts[2] > 5 && counts[3] < 15,
                "{missing}: {counts:?}"
            );
        }
    }
    fence(&d);
    assert_eq!(
        before,
        [MemoryBudget::volumes().used(), MemoryBudget::gpu().used()]
    );
}

#[test]
fn section_probes_refuse_stale_owners_styles_scopes_pixels_and_cancellation() {
    let (d, q) = device(None);
    let gpu = GpuVolumeSource::new(&d, &q, source([17, 15, 9], 1, false, false), || false).unwrap();
    let geometry = surface::geometry(gpu.clone());
    let projector = GpuSurfaceProjector::new(gpu.clone()).unwrap();
    let mut scene = scene(gpu.clone());
    scene.surface = Some(SectionSurface {
        image: surface::project(&projector, geometry, &scene),
        luminance: 0.8,
        lighting: Lighting::UNLIT,
    });
    let mut renderer = SectionRenderer::new(gpu, wgpu::TextureFormat::Rgba8UnormSrgb).unwrap();
    let frame = complete(&mut renderer, &scene);
    let current = SectionPickState::from_frame(&frame).unwrap();
    let pixel = [17, 13];
    let result = renderer.probe(&frame, &current, pixel, || false).unwrap();
    assert!(result.is_current(&current, pixel));
    assert!(!result.is_current(&current, [18, 13]));
    let edits: [fn(&mut SectionPickState); 13] = [
        |s| s.scope = PickScope::new().unwrap(),
        |s| s.number = 2,
        |s| s.scene.navigation.pan[0] += 0.01,
        |s| s.scene.volume.camera.yaw_degrees += 1.,
        |s| s.scene.size[0] += 1,
        |s| s.scene.volume.quality = Quality::Interactive,
        |s| s.scene.material = Arc::new((*s.scene.material).clone()),
        |s| s.scene.lighting = Lighting::HEADLIGHT,
        |s| s.scene.surface.as_mut().unwrap().luminance = 0.7,
        |s| s.scene.surface.as_mut().unwrap().lighting = Lighting::HEADLIGHT,
        |s| s.scene.planes[0] = None,
        |s| s.scene.planes[1].as_mut().unwrap().invert = true,
        |s| s.scene.planes[2].as_mut().unwrap().window.center += 1.,
    ];
    for edit in edits {
        let mut changed = current.clone();
        edit(&mut changed);
        assert!(!changed.matches(&frame));
        assert!(!result.is_current(&changed, pixel));
        assert!(renderer.probe(&frame, &changed, pixel, || false).is_err());
    }
    let mut reslicer = GpuReslicer::new(renderer.source.clone()).unwrap();
    let replacement = reslicer
        .render_frames(
            [
                Some(scene.planes[0].as_ref().unwrap().image.grid()),
                None,
                None,
            ],
            || false,
        )
        .unwrap()[0]
        .clone()
        .unwrap();
    let mut changed = current.clone();
    changed.scene.planes[0].as_mut().unwrap().image = replacement;
    assert!(
        !changed.matches(&frame),
        "Equal geometry does not replace a completed owner"
    );
    let other_geometry = surface::geometry(renderer.source.clone());
    changed = current.clone();
    changed.map = other_geometry.surface().owner().clone();
    assert!(
        !changed.matches(&frame),
        "Equal mask cells do not replace an owner"
    );
    let orv_core::mask::MaskOwner::Manual(map) = &current.map else {
        panic!("manual fixture")
    };
    let mut edited = map.try_clone(|| false).unwrap();
    edited.paint(0, &[[1., 1.]], 1, 1, || false).unwrap();
    changed = current.clone();
    changed.map = orv_core::mask::MaskOwner::Manual(Arc::new(edited));
    assert!(
        !changed.matches(&frame),
        "A changed revision cannot authorize an earlier front"
    );
    let foreign = GpuVolumeSource::new(&d, &q, renderer.source.volume().clone(), || false).unwrap();
    let mut foreign = SectionRenderer::new(foreign, wgpu::TextureFormat::Rgba8UnormSrgb).unwrap();
    assert!(foreign.probe(&frame, &current, pixel, || false).is_err());
    assert!(renderer.probe(&frame, &current, [37, 0], || false).is_err());
    let calls = Cell::new(0);
    assert!(
        renderer
            .probe(&frame, &current, pixel, || {
                calls.set(calls.get() + 1);
                calls.get() > 2
            })
            .is_err()
    );
    current.scope.revoke();
    assert!(!current.matches(&frame));
    assert!(!result.is_current(&current, pixel));
    assert!(renderer.probe(&frame, &current, pixel, || false).is_err());
}

fn plane_at(gpu: &Arc<GpuVolumeSource>, plane: Plane, point: [f64; 3]) -> SectionPlane {
    let grid = gpu.volume().reformat_grid(plane, point, 64).unwrap();
    let mut reslicer = GpuReslicer::new(gpu.clone()).unwrap();
    let image = reslicer
        .render_frames([Some(&grid), None, None], || false)
        .unwrap()[0]
        .clone()
        .unwrap();
    SectionPlane {
        image,
        window: WindowLevel::SOFT_TISSUE,
        invert: false,
    }
}

#[test]
fn coincident_faces_win_and_invalid_parallel_or_clipped_planes_cannot_hide_real_backfaces() {
    let (d, q) = device(None);
    let cpu = source([17, 15, 9], 1, false, false);
    let cpu = Arc::new(
        Volume::new(
            cpu.frames()
                .iter()
                .map(|f| {
                    let mut f = (**f).clone();
                    f.validity = Some(
                        (0..255)
                            .map(|i| u8::from(i / 17 != 1))
                            .collect::<Vec<_>>()
                            .into(),
                    );
                    Arc::new(f)
                })
                .collect(),
        )
        .unwrap(),
    );
    let gpu = GpuVolumeSource::new(&d, &q, cpu.clone(), || false).unwrap();
    let geometry = surface::geometry(gpu.clone());
    let projector = GpuSurfaceProjector::new(gpu.clone()).unwrap();
    let mut renderer =
        SectionRenderer::new(gpu.clone(), wgpu::TextureFormat::Rgba8UnormSrgb).unwrap();
    let mut scene = scene(gpu.clone());
    scene.size = [1; 2];
    scene.navigation.pan = [0.; 2];
    scene.volume.camera.target_mm = cpu.voxel_to_world([8., 7., 4.]);
    scene.material = material(0.);
    let face = cpu.voxel_to_world([8., 2.5, 4.]);
    for (plane, point, expected) in [
        (
            Plane::Coronal,
            [face[0], face[1] - 0.25, face[2]],
            SurfaceProbeKind::Obscured,
        ),
        (Plane::Coronal, face, SurfaceProbeKind::Hit),
        (
            Plane::Coronal,
            [face[0], face[1].next_down(), face[2]],
            SurfaceProbeKind::Ambiguous,
        ),
        (
            Plane::Coronal,
            [face[0], face[1].next_up(), face[2]],
            SurfaceProbeKind::Hit,
        ),
        (
            Plane::Coronal,
            [face[0], face[1] + 0.25, face[2]],
            SurfaceProbeKind::Hit,
        ),
        (
            Plane::Coronal,
            cpu.voxel_to_world([8., 0.5, 4.]),
            SurfaceProbeKind::Hit,
        ),
        (Plane::Axial, face, SurfaceProbeKind::Hit),
    ] {
        scene.planes = [Some(plane_at(&gpu, plane, point)), None, None];
        scene.surface = Some(SectionSurface {
            image: surface::project(&projector, geometry.clone(), &scene),
            luminance: 0.8,
            lighting: Lighting::UNLIT,
        });
        let frame = complete(&mut renderer, &scene);
        let result = renderer
            .probe(
                &frame,
                &SectionPickState::from_frame(&frame).unwrap(),
                [0; 2],
                || false,
            )
            .unwrap();
        assert_eq!(result.kind(), expected, "{plane:?}/{point:?}");
        if let Some(hit) = result.hit() {
            assert_eq!(hit.face.side() as u8, 2);
        }
    }
    scene.planes = [None, None, None];
    // Missing y=1 stencils leave only y=2..2.5 (0.75 mm) of foreground source.
    // Values on either side of the visibility rule stay distinct.
    for transmission in [0.049f64, 0.051] {
        scene.material = material((-transmission.ln() / 0.75) as f32);
        scene.surface = Some(SectionSurface {
            image: surface::project(&projector, geometry.clone(), &scene),
            luminance: 0.8,
            lighting: Lighting::UNLIT,
        });
        let frame = complete(&mut renderer, &scene);
        let result = renderer
            .probe(
                &frame,
                &SectionPickState::from_frame(&frame).unwrap(),
                [0; 2],
                || false,
            )
            .unwrap();
        assert!((f64::from(result.foreground_transmittance()) - transmission).abs() < 0.00001);
        assert_eq!(
            result.kind(),
            if transmission < 0.05 {
                SurfaceProbeKind::Obscured
            } else {
                SurfaceProbeKind::Hit
            }
        );
    }
    scene.material = material(0.);
    let center = cpu.center();
    scene.volume.clip = Some(PhysicalClip {
        minimum_mm: [center[0] - 100., center[1], center[2] - 100.],
        maximum_mm: center.map(|v| v + 100.),
    });
    for (offset, expected) in [
        (None, SurfaceProbeKind::Hit),
        (Some(0.), SurfaceProbeKind::Obscured),
        (Some(-1.), SurfaceProbeKind::Hit),
    ] {
        scene.planes = [
            offset.map(|offset| {
                plane_at(
                    &gpu,
                    Plane::Coronal,
                    [center[0], center[1] + offset, center[2]],
                )
            }),
            None,
            None,
        ];
        scene.surface = Some(SectionSurface {
            image: surface::project(&projector, geometry.clone(), &scene),
            luminance: 0.8,
            lighting: Lighting::UNLIT,
        });
        let frame = complete(&mut renderer, &scene);
        let result = renderer
            .probe(
                &frame,
                &SectionPickState::from_frame(&frame).unwrap(),
                [0; 2],
                || false,
            )
            .unwrap();
        assert_eq!(result.kind(), expected, "backface {offset:?}");
        if let Some(hit) = result.hit() {
            assert_eq!(hit.face.side() as u8, 3);
        }
    }
    // An exact source-cell edge stays ambiguous even if display rasterization
    // chose a face. Moving just inside uses its real position without nudging.
    scene.volume.clip = None;
    scene.planes = [None, None, None];
    for (x, expected) in [
        (3.5, SurfaceProbeKind::Ambiguous),
        (3.5 + 1e-5, SurfaceProbeKind::Hit),
    ] {
        scene.volume.camera.target_mm = cpu.voxel_to_world([x, 7., 4.]);
        scene.surface = Some(SectionSurface {
            image: surface::project(&projector, geometry.clone(), &scene),
            luminance: 0.8,
            lighting: Lighting::UNLIT,
        });
        let frame = complete(&mut renderer, &scene);
        let result = renderer
            .probe(
                &frame,
                &SectionPickState::from_frame(&frame).unwrap(),
                [0; 2],
                || false,
            )
            .unwrap();
        assert_eq!(result.kind(), expected, "edge {x}");
        assert_eq!(result.hit().is_some(), expected == SurfaceProbeKind::Hit);
    }
    // A false scene label cannot substitute for the actual completed radiance.
    let mut image = renderer
        .render_rows(&scene, 0, 0..1, || false)
        .unwrap()
        .unwrap();
    Arc::get_mut(&mut image)
        .unwrap()
        .scene
        .surface
        .as_mut()
        .unwrap()
        .luminance = 0.1;
    let frame = SectionPickFrame::new(image, PickScope::new().unwrap());
    assert!(
        renderer
            .probe(
                &frame,
                &SectionPickState::from_frame(&frame).unwrap(),
                [0; 2],
                || false
            )
            .is_err()
    );
}

#[test]
fn fractional_high_id_unknown_coverage_keeps_open_faces_behind_ordered_planes() {
    use crate::surface_source::GpuCellSurface;
    use orv_core::{
        mask::MaskOwner,
        segments::{FractionalEncoding, FractionalKind, Segments},
        surface::{CellSurface, SurfaceLimits},
    };
    let (d, q) = device(None);
    let cpu = source([17, 15, 9], 1, false, false);
    let gpu = GpuVolumeSource::new(&d, &q, cpu.clone(), || false).unwrap();
    let mut payload = vec![0; 510];
    payload[..255].fill(100);
    let map = Arc::new(
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
    let owner = MaskOwner::Supplied(map);
    let surface = CellSurface::build(owner, 65535, None, SurfaceLimits::default(), || false)
        .unwrap()
        .0;
    let geometry = GpuCellSurface::upload(gpu.clone(), surface, None, || false)
        .unwrap()
        .0;
    let projector = GpuSurfaceProjector::new(gpu.clone()).unwrap();
    let mut renderer =
        SectionRenderer::new(gpu.clone(), wgpu::TextureFormat::Rgba8UnormSrgb).unwrap();
    let mut scene = scene(gpu.clone());
    scene.size = [1; 2];
    scene.navigation.pan = [0.; 2];
    scene.volume.camera.pitch_degrees = 90.;
    scene.volume.camera.target_mm = cpu.voxel_to_world([8., 7., 1.]);
    scene.material = material(0.);
    for (z, expected) in [
        (1., SurfaceProbeKind::Obscured),
        (2., SurfaceProbeKind::Hit),
    ] {
        scene.planes = [
            Some(plane_at(
                &gpu,
                Plane::Axial,
                cpu.voxel_to_world([8., 7., z]),
            )),
            None,
            None,
        ];
        scene.surface = Some(SectionSurface {
            image: surface::project(&projector, geometry.clone(), &scene),
            luminance: 0.8,
            lighting: Lighting::UNLIT,
        });
        let frame = complete(&mut renderer, &scene);
        let result = renderer
            .probe(
                &frame,
                &SectionPickState::from_frame(&frame).unwrap(),
                [0; 2],
                || false,
            )
            .unwrap();
        assert_eq!(result.kind(), expected);
        if let Some(hit) = result.hit() {
            assert_eq!(hit.key.number, 65535);
            assert_eq!(hit.face.side() as u8, 5);
        }
    }
}
