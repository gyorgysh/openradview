// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[test]
fn click_erase_footprints_cover_every_eligible_full_raster_ray_under_affines_rotations_and_clips() {
    let (d, q) = device(None);
    let before = [MemoryBudget::volumes().used(), MemoryBudget::gpu().used()];
    {
        for reverse in [false, true] {
            for missing in [false, true] {
                let gpu =
                    GpuVolumeSource::new(&d, &q, source([17, 15, 9], 1, missing, reverse), || {
                        false
                    })
                    .unwrap();
                let geometry = surface::geometry(gpu.clone());
                let projector = GpuSurfaceProjector::new(gpu.clone()).unwrap();
                let mut renderer =
                    SectionRenderer::new(gpu.clone(), wgpu::TextureFormat::Rgba8UnormSrgb).unwrap();
                let mut scene = scene(gpu);
                scene.size = [93, 71];
                scene.material = material(0.);
                for quarter_turn in [false, true] {
                    scene.volume.camera.yaw_degrees = if quarter_turn { 90. } else { 23. };
                    scene.volume.camera.pitch_degrees = if quarter_turn { 0. } else { -17. };
                    scene.volume.camera.roll_degrees = if quarter_turn { 90. } else { 31. };
                    let centre = renderer.source.volume().center();
                    scene.volume.clip = quarter_turn.then_some(PhysicalClip {
                        minimum_mm: [centre[0] - 2.1, centre[1] - 100., centre[2] - 100.],
                        maximum_mm: centre.map(|v| v + 100.),
                    });
                    scene.surface = Some(SectionSurface {
                        image: surface::project(&projector, geometry.clone(), &scene),
                        luminance: 0.8,
                        lighting: Lighting::HEADLIGHT,
                    });
                    let frame = complete(&mut renderer, &scene);
                    let current = SectionPickState::from_frame(&frame).unwrap();
                    let mut reference = Vec::new();
                    for y in (0..71).step_by(16) {
                        reference.push(
                            renderer
                                .probe_region(
                                    &frame,
                                    &current,
                                    [[0, y], [93, (71 - y).min(16)]],
                                    || false,
                                )
                                .unwrap(),
                        );
                    }
                    let hit = reference
                        .iter()
                        .flat_map(|b| b.samples())
                        .filter(|s| s.kind() == SurfaceProbeKind::Hit)
                        .min_by_key(|s| s.pixel()[0].abs_diff(46) + s.pixel()[1].abs_diff(35))
                        .unwrap();
                    let anchor = hit.pixel();
                    let anchor_cell = hit.hit().unwrap().face.cell();
                    let cpu = renderer.source.volume().clone();
                    let centre = cpu.voxel_to_world(anchor_cell.map(f64::from));
                    for radius in [1e-6, 1.25, 3.5] {
                        let mut settings = brush(&scene);
                        settings.radius_mm = radius;
                        let (proposal, region) = renderer
                            .propose_erase_stamp(&frame, &current, anchor, &settings, || false)
                            .unwrap();
                        assert_eq!(proposal.proposal().points_mm(), &[centre]);
                        assert!(region[1][0] < 93 && region[1][1] < 71);
                        let mut expected = BTreeSet::new();
                        for sample in reference.iter().flat_map(|b| b.samples()) {
                            let Some(hit) = sample.hit() else { continue };
                            let cell = hit.face.cell();
                            let delta = cpu.voxel_vector_to_world(std::array::from_fn(|a| {
                                f64::from(cell[a]) - f64::from(anchor_cell[a])
                            }));
                            let distance = delta[0].hypot(delta[1]).hypot(delta[2]);
                            let p = cpu.voxel_to_world(cell.map(f64::from));
                            let inside_clip = settings.clip.as_ref().is_none_or(|clip| {
                                (0..3).all(|a| {
                                    p[a] >= clip.minimum_mm[a] && p[a] <= clip.maximum_mm[a]
                                })
                            });
                            if distance <= radius && inside_clip {
                                let pixel = sample.pixel();
                                assert!(
                                    (0..2).all(|a| pixel[a] >= region[0][a]
                                        && pixel[a] < region[0][a] + region[1][a]),
                                    "{reverse}/{missing}/{quarter_turn}/{radius}/{pixel:?}/{region:?}"
                                );
                                expected.insert(cell.map(|v| v as usize));
                            }
                        }
                        assert!(expected.contains(&anchor_cell.map(|v| v as usize)));
                        assert_eq!(proposal.summary().changed_voxels, expected.len());
                        if radius == 1e-6 {
                            assert_eq!(expected.len(), 1);
                        }
                        let MaskOwner::Manual(base) = &current.map else {
                            unreachable!()
                        };
                        for z in 0..9 {
                            for y in 0..15 {
                                for x in 0..17 {
                                    let cell = [x, y, z];
                                    assert_eq!(
                                        proposal.proposal().preview().value(cell),
                                        if expected.contains(&cell) {
                                            Some(0)
                                        } else {
                                            base.value(cell)
                                        },
                                        "{reverse}/{missing}/{quarter_turn}/{radius}/{cell:?}"
                                    );
                                }
                            }
                        }
                        let v = proposal.visibility();
                        assert_eq!(
                            v.hit + v.empty + v.obscured + v.ambiguous,
                            (region[1][0] * region[1][1]) as usize
                        );
                        let result = proposal
                            .apply(&current, &settings, &[centre], || false)
                            .unwrap();
                        for cell in expected {
                            assert_eq!(result.value(cell), Some(0));
                            assert_eq!(base.value(cell), Some(1));
                        }
                    }
                }
            }
        }
    }
    fence(&d);
    assert_eq!(
        before,
        [MemoryBudget::volumes().used(), MemoryBudget::gpu().used()]
    );
}

#[test]
fn click_erase_refuses_invalid_settings_oversize_covered_and_unresolved_views() {
    let (d, q) = device(None);
    let gpu = GpuVolumeSource::new(&d, &q, source([17, 15, 9], 1, false, false), || false).unwrap();
    let geometry = surface::geometry(gpu.clone());
    let projector = GpuSurfaceProjector::new(gpu.clone()).unwrap();
    let mut renderer =
        SectionRenderer::new(gpu.clone(), wgpu::TextureFormat::Rgba8UnormSrgb).unwrap();
    let mut scene = scene(gpu);
    scene.size = [512, 256];
    scene.navigation.pan = [0.; 2];
    scene.planes = [None, None, None];
    scene.material = material(0.);
    scene.surface = Some(SectionSurface {
        image: surface::project(&projector, geometry.clone(), &scene),
        luminance: 0.8,
        lighting: Lighting::HEADLIGHT,
    });
    let frame = complete(&mut renderer, &scene);
    let current = SectionPickState::from_frame(&frame).unwrap();
    let pixel = [256, 128];
    let mut settings = brush(&scene);
    settings.radius_mm = 1.;
    for change in 0..7 {
        let mut invalid = settings.clone();
        match change {
            0 => invalid.radius_mm = f64::NAN,
            1 => invalid.radius_mm = 0.,
            2 => invalid.label = 2,
            3 => invalid.protected_ids[1] = true,
            4 => invalid.action = BrushAction::Add,
            5 => {
                invalid.shape = BrushShape::Plane {
                    origin_mm: [0.; 3],
                    normal: [0., 0., 1.],
                    thickness_mm: 1.,
                }
            }
            _ => {
                invalid.clip = Some(PhysicalClip {
                    minimum_mm: [-1e7; 3],
                    maximum_mm: [1e7; 3],
                })
            }
        }
        assert!(
            renderer
                .propose_erase_stamp(&frame, &current, pixel, &invalid, || false)
                .is_err()
        );
        assert!(renderer.probe.is_none());
    }
    let mut changed = current.clone();
    changed.scene.navigation.zoom += 1.;
    assert!(
        renderer
            .propose_erase_stamp(&frame, &changed, pixel, &settings, || false)
            .is_err()
    );
    assert!(renderer.probe.is_none());
    settings.radius_mm = 256.;
    fence(&d);
    let held = [MemoryBudget::volumes().used(), MemoryBudget::gpu().used()];
    let error = renderer
        .propose_erase_stamp(&frame, &current, pixel, &settings, || false)
        .err()
        .unwrap();
    assert!(error.contains("too many image pixels"), "{error}");
    fence(&d);
    let after = [MemoryBudget::volumes().used(), MemoryBudget::gpu().used()];
    assert_eq!(held[0], after[0]);
    // The lazily created reusable pipeline may retain GPU admission; no proof,
    // map or readback survives the failed oversized request.
    assert!(after[1] >= held[1]);
    settings.radius_mm = 1.;
    assert!(
        renderer
            .propose_erase_stamp(&frame, &current, [0, 0], &settings, || false)
            .is_err()
    );
    assert!(
        renderer
            .propose_erase_stamp(&frame, &current, [512, 0], &settings, || false)
            .is_err()
    );
    scene.material = material(10.);
    let covered = complete(&mut renderer, &scene);
    let covered_state = SectionPickState::from_frame(&covered).unwrap();
    assert!(
        renderer
            .propose_erase_stamp(&covered, &covered_state, pixel, &settings, || false)
            .err()
            .unwrap()
            .contains("covered")
    );
    scene.material = material(0.);
    scene.volume.camera.vertical_span_mm = 0.0001;
    scene.navigation.zoom = 50.;
    scene.surface = Some(SectionSurface {
        image: surface::project(&projector, geometry, &scene),
        luminance: 0.8,
        lighting: Lighting::HEADLIGHT,
    });
    let zoomed = complete(&mut renderer, &scene);
    let zoomed_state = SectionPickState::from_frame(&zoomed).unwrap();
    let error = renderer
        .propose_erase_stamp(&zoomed, &zoomed_state, pixel, &settings, || false)
        .err()
        .unwrap();
    assert!(error.contains("bounded at this zoom"), "{error}");
}

#[test]
fn click_erase_cancellation_pressure_and_revocation_release_all_unpublished_work() {
    let (d, q) = device(None);
    let before = [MemoryBudget::volumes().used(), MemoryBudget::gpu().used()];
    {
        let gpu =
            GpuVolumeSource::new(&d, &q, source([17, 15, 9], 1, false, false), || false).unwrap();
        let geometry = surface::geometry(gpu.clone());
        let projector = GpuSurfaceProjector::new(gpu.clone()).unwrap();
        let mut renderer =
            SectionRenderer::new(gpu.clone(), wgpu::TextureFormat::Rgba8UnormSrgb).unwrap();
        let mut scene = scene(gpu);
        scene.planes = [None, None, None];
        scene.material = material(0.);
        scene.navigation.pan = [0.; 2];
        scene.size = [93, 71];
        scene.surface = Some(SectionSurface {
            image: surface::project(&projector, geometry, &scene),
            luminance: 0.8,
            lighting: Lighting::HEADLIGHT,
        });
        let frame = complete(&mut renderer, &scene);
        let current = SectionPickState::from_frame(&frame).unwrap();
        let mut settings = brush(&scene);
        settings.radius_mm = 1.25;
        let pixel = [46, 35];
        drop(renderer.probe(&frame, &current, pixel, || false).unwrap());
        fence(&d);
        let held = [MemoryBudget::volumes().used(), MemoryBudget::gpu().used()];
        let calls = Cell::new(0);
        let (p, _) = renderer
            .propose_erase_stamp(&frame, &current, pixel, &settings, || {
                calls.set(calls.get() + 1);
                false
            })
            .unwrap();
        assert!(p.summary().changed_voxels > 0);
        drop(p);
        fence(&d);
        assert_eq!(
            held,
            [MemoryBudget::volumes().used(), MemoryBudget::gpu().used()]
        );
        for stop in [
            0,
            1,
            2,
            calls.get() / 4,
            calls.get() / 2,
            calls.get() - 2,
            calls.get() - 1,
        ] {
            let count = Cell::new(0);
            assert!(
                renderer
                    .propose_erase_stamp(&frame, &current, pixel, &settings, || {
                        let n = count.get();
                        count.set(n + 1);
                        n == stop
                    })
                    .is_err(),
                "callback {stop}"
            );
            fence(&d);
            assert_eq!(
                held,
                [MemoryBudget::volumes().used(), MemoryBudget::gpu().used()]
            );
        }
        for budget in [MemoryBudget::volumes(), MemoryBudget::gpu()] {
            let pressure = budget.reserve(budget.limit() - budget.used()).unwrap();
            assert!(
                renderer
                    .propose_erase_stamp(&frame, &current, pixel, &settings, || false)
                    .is_err()
            );
            drop(pressure);
            fence(&d);
            assert_eq!(
                held,
                [MemoryBudget::volumes().used(), MemoryBudget::gpu().used()]
            );
        }
        let (p, _) = renderer
            .propose_erase_stamp(&frame, &current, pixel, &settings, || false)
            .unwrap();
        drop(p);
        let count = Cell::new(0);
        assert!(
            renderer
                .propose_erase_stamp(&frame, &current, pixel, &settings, || {
                    count.set(count.get() + 1);
                    if count.get() == 40 {
                        frame.scope().revoke();
                    }
                    false
                })
                .is_err()
        );
        assert!(!frame.scope().is_active());
        let MaskOwner::Manual(base) = &current.map else {
            unreachable!()
        };
        assert_eq!(base.value([8, 7, 4]), Some(1));
    }
    fence(&d);
    assert_eq!(
        before,
        [MemoryBudget::volumes().used(), MemoryBudget::gpu().used()]
    );
}
