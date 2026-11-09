// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[test]
fn bounded_probe_rows_match_every_original_pixel_single_probe_with_mixed_actors_and_missing_data() {
    let (d, q) = device(None);
    let before = [MemoryBudget::volumes().used(), MemoryBudget::gpu().used()];
    {
        let mut counts = [0; 4];
        for missing in [false, true] {
            let gpu = GpuVolumeSource::new(&d, &q, source([17, 15, 9], 1, missing, true), || false)
                .unwrap();
            let geometry = surface::geometry(gpu.clone());
            let projector = GpuSurfaceProjector::new(gpu.clone()).unwrap();
            let mut renderer =
                SectionRenderer::new(gpu.clone(), wgpu::TextureFormat::Rgba8UnormSrgb).unwrap();
            let mut scene = scene(gpu);
            scene.volume.camera.yaw_degrees = 23.;
            scene.volume.camera.pitch_degrees = -17.;
            scene.volume.camera.roll_degrees = 31.;
            for clipped in [false, true] {
                let centre = renderer.source.volume().center();
                scene.volume.clip = clipped.then_some(PhysicalClip {
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
                for region in [[[7, 8], [17, 11]], [[0, 0], [37, 1]], [[36, 28], [1, 1]]] {
                    let batch = renderer
                        .probe_region(&frame, &current, region, || false)
                        .unwrap();
                    assert!(batch.is_current(&current, region));
                    assert_eq!(batch.region(), region);
                    assert_eq!(
                        batch.samples().len(),
                        (region[1][0] * region[1][1]) as usize
                    );
                    for (index, sample) in batch.samples().iter().enumerate() {
                        let pixel = [
                            region[0][0] + index as u32 % region[1][0],
                            region[0][1] + index as u32 / region[1][0],
                        ];
                        assert_eq!(sample.pixel(), pixel);
                        let single = renderer.probe(&frame, &current, pixel, || false).unwrap();
                        assert_eq!(
                            sample.kind(),
                            single.kind(),
                            "{missing}/{clipped}/{pixel:?}"
                        );
                        assert_eq!(sample.hit(), single.hit());
                        assert_eq!(
                            sample.foreground_transmittance(),
                            single.foreground_transmittance()
                        );
                        counts[match sample.kind() {
                            SurfaceProbeKind::Hit => 0,
                            SurfaceProbeKind::Empty => 1,
                            SurfaceProbeKind::Obscured => 2,
                            SurfaceProbeKind::Ambiguous => 3,
                        }] += 1;
                    }
                }
            }
        }
        assert!(
            counts[0] > 10 && counts[1] > 100 && counts[2] > 10,
            "{counts:?}"
        );
    }
    fence(&d);
    assert_eq!(
        before,
        [MemoryBudget::volumes().used(), MemoryBudget::gpu().used()]
    );
}
#[test]
fn maximum_probe_rectangles_keep_the_complete_raster_coordinates_without_downsampling() {
    let (d, q) = device(None);
    let gpu = GpuVolumeSource::new(&d, &q, source([17, 15, 9], 1, false, false), || false).unwrap();
    let geometry = surface::geometry(gpu.clone());
    let projector = GpuSurfaceProjector::new(gpu.clone()).unwrap();
    let mut renderer =
        SectionRenderer::new(gpu.clone(), wgpu::TextureFormat::Rgba8UnormSrgb).unwrap();
    let mut scene = scene(gpu);
    scene.size = [2048, 64];
    scene.surface = Some(SectionSurface {
        image: surface::project(&projector, geometry, &scene),
        luminance: 0.8,
        lighting: Lighting::HEADLIGHT,
    });
    let frame = complete(&mut renderer, &scene);
    let current = SectionPickState::from_frame(&frame).unwrap();
    for region in [[[0, 31], [2048, 2]], [[896, 24], [256, 16]]] {
        let batch = renderer
            .probe_region(&frame, &current, region, || false)
            .unwrap();
        assert_eq!(batch.samples().len(), 4096);
        for index in [0, region[1][0] - 1, 2048, 4095] {
            let sample = &batch.samples()[index as usize];
            let pixel = [
                region[0][0] + index % region[1][0],
                region[0][1] + index / region[1][0],
            ];
            assert_eq!(sample.pixel(), pixel);
            let single = renderer.probe(&frame, &current, pixel, || false).unwrap();
            assert_eq!(sample.kind(), single.kind());
            assert_eq!(sample.hit(), single.hit());
            assert_eq!(
                sample.foreground_transmittance(),
                single.foreground_transmittance()
            );
        }
    }
}
#[test]
fn probe_batches_refuse_bounds_stale_state_and_revocation_without_publishing_partial_results() {
    let (d, q) = device(None);
    let gpu = GpuVolumeSource::new(&d, &q, source([17, 15, 9], 1, false, false), || false).unwrap();
    let geometry = surface::geometry(gpu.clone());
    let projector = GpuSurfaceProjector::new(gpu.clone()).unwrap();
    let mut renderer =
        SectionRenderer::new(gpu.clone(), wgpu::TextureFormat::Rgba8UnormSrgb).unwrap();
    let mut scene = scene(gpu);
    scene.size = [257, 33];
    scene.surface = Some(SectionSurface {
        image: surface::project(&projector, geometry, &scene),
        luminance: 0.8,
        lighting: Lighting::HEADLIGHT,
    });
    let frame = complete(&mut renderer, &scene);
    let current = SectionPickState::from_frame(&frame).unwrap();
    for region in [
        [[0, 0], [0, 1]],
        [[0, 0], [1, 0]],
        [[0, 0], [257, 16]],
        [[0, 0], [10, 17]],
        [[256, 0], [2, 1]],
        [[0, 33], [1, 1]],
        [[u32::MAX, 0], [1, 1]],
    ] {
        assert!(
            renderer
                .probe_region(&frame, &current, region, || false)
                .is_err()
        );
        assert!(
            renderer.probe.is_none(),
            "Refused work must not create a pipeline"
        );
    }
    let region = [[120, 12], [13, 9]];
    let mut changed = current.clone();
    changed.scene.planes[1].as_mut().unwrap().invert = true;
    assert!(
        renderer
            .probe_region(&frame, &changed, region, || false)
            .is_err()
    );
    assert!(renderer.probe.is_none());
    let budget = MemoryBudget::volumes();
    let before_cpu = budget.used();
    let pressure = budget.reserve(budget.limit() - before_cpu - 64).unwrap();
    assert!(
        renderer
            .probe_region(&frame, &current, region, || false)
            .is_err()
    );
    assert!(renderer.probe.is_none());
    drop(pressure);
    assert_eq!(budget.used(), before_cpu);
    let batch = renderer
        .probe_region(&frame, &current, region, || false)
        .unwrap();
    assert!(batch.is_current(&current, region));
    assert!(!batch.is_current(&changed, region));
    assert!(!batch.is_current(&current, [[121, 12], [13, 9]]));
    fence(&d);
    let gpu_budget = MemoryBudget::gpu();
    let pressure = gpu_budget
        .reserve(gpu_budget.limit() - gpu_budget.used())
        .unwrap();
    assert!(
        renderer
            .probe_region(&frame, &current, region, || false)
            .is_err()
    );
    drop(pressure);
    assert!(batch.is_current(&current, region));
    let retry = renderer
        .probe_region(&frame, &current, region, || false)
        .unwrap();
    assert_eq!(retry.samples().len(), batch.samples().len());
    drop(retry);
    fence(&d);
    let before = [MemoryBudget::volumes().used(), MemoryBudget::gpu().used()];
    let calls = Cell::new(0);
    let result = renderer.probe_region(&frame, &current, region, || {
        calls.set(calls.get() + 1);
        if calls.get() == 20 {
            current.scope.revoke();
        }
        false
    });
    assert!(result.is_err());
    assert!(calls.get() >= 20);
    assert!(!batch.is_current(&current, region));
    fence(&d);
    assert_eq!(
        before,
        [MemoryBudget::volumes().used(), MemoryBudget::gpu().used()]
    );
    // Exact scene metadata is insufficient if it describes different radiance
    // from the completed image being shown. Refuse the whole result rectangle.
    let mut image = renderer
        .render_rows(&scene, 0, 0..scene.size[1], || false)
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
    let current = SectionPickState::from_frame(&frame).unwrap();
    assert!(
        renderer
            .probe_region(&frame, &current, region, || false)
            .is_err()
    );
}
