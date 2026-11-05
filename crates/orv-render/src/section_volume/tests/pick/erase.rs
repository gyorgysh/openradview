// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use crate::section_volume::pick::{SectionEraseVisibility, SectionFaceErase};
use orv_core::{
    labels::{BrushAction, BrushShape, PhysicalBrush},
    mask::MaskOwner,
};
use std::{cell::Cell, collections::BTreeSet};

fn brush(scene: &SectionScene) -> PhysicalBrush {
    PhysicalBrush {
        label: 1,
        action: BrushAction::Erase,
        radius_mm: 80.,
        shape: BrushShape::Sphere,
        clip: scene.volume.clip.clone(),
        protected_ids: [false; 256],
    }
}

#[test]
fn checked_section_erase_changes_only_original_visible_cells_with_planes_clips_and_missing_samples()
{
    let (d, q) = device(None);
    let before = [MemoryBudget::volumes().used(), MemoryBudget::gpu().used()];
    {
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
            scene.material = material(0.);
            let planes = scene.planes.clone();
            let centre = renderer.source.volume().center();
            let points = [centre];
            for clipped in [false, true] {
                scene.volume.clip = clipped.then_some(PhysicalClip {
                    minimum_mm: [centre[0] - 2.1, centre[1] - 100., centre[2] - 100.],
                    maximum_mm: centre.map(|v| v + 100.),
                });
                scene.surface = Some(SectionSurface {
                    image: surface::project(&projector, geometry.clone(), &scene),
                    luminance: 0.8,
                    lighting: Lighting::HEADLIGHT,
                });
                for planes_on in [false, true] {
                    scene.planes = if planes_on {
                        planes.clone()
                    } else {
                        [None, None, None]
                    };
                    let frame = complete(&mut renderer, &scene);
                    let current = SectionPickState::from_frame(&frame).unwrap();
                    let batches = [
                        renderer
                            .probe_region(&frame, &current, [[0, 0], [37, 16]], || false)
                            .unwrap(),
                        renderer
                            .probe_region(&frame, &current, [[0, 16], [37, 13]], || false)
                            .unwrap(),
                        renderer
                            .probe_region(&frame, &current, [[7, 8], [17, 11]], || false)
                            .unwrap(),
                    ];
                    let settings = brush(&scene);
                    let proposal =
                        SectionFaceErase::prepare(&current, &batches, &settings, &points, || false)
                            .unwrap();
                    let mut visible = BTreeSet::new();
                    let mut counts = SectionEraseVisibility::default();
                    for sample in batches.iter().flat_map(|b| b.samples()) {
                        match sample.kind() {
                            SurfaceProbeKind::Hit => {
                                counts.hit += 1;
                                visible
                                    .insert(sample.hit().unwrap().face.cell().map(|v| v as usize));
                            }
                            SurfaceProbeKind::Empty => counts.empty += 1,
                            SurfaceProbeKind::Obscured => counts.obscured += 1,
                            SurfaceProbeKind::Ambiguous => counts.ambiguous += 1,
                        }
                    }
                    assert_eq!(proposal.visibility(), counts);
                    assert!(
                        counts.hit > 10 && counts.empty > 100,
                        "{missing}/{clipped}/{planes_on}: {counts:?}"
                    );
                    if planes_on {
                        assert!(counts.obscured > 10);
                    }
                    let MaskOwner::Manual(base) = &current.map else {
                        unreachable!()
                    };
                    let cpu = renderer.source.volume();
                    let mut expected = BTreeSet::new();
                    for z in 0..9 {
                        for y in 0..15 {
                            for x in 0..17 {
                                let cell = [x, y, z];
                                let p = cpu.voxel_to_world(cell.map(|v| v as f64));
                                let inside = settings.clip.as_ref().is_none_or(|c| {
                                    (0..3)
                                        .all(|a| p[a] >= c.minimum_mm[a] && p[a] <= c.maximum_mm[a])
                                });
                                let erased = visible.contains(&cell) && inside;
                                if erased {
                                    expected.insert(cell);
                                }
                                assert_eq!(
                                    proposal.proposal().preview().value(cell),
                                    if erased { Some(0) } else { base.value(cell) },
                                    "{missing}/{clipped}/{planes_on}/{cell:?}"
                                );
                            }
                        }
                    }
                    assert!(!expected.is_empty());
                    assert_eq!(proposal.summary().changed_voxels, expected.len());
                    assert_eq!(proposal.summary().candidate_faces, counts.hit);
                    assert!(
                        counts.hit > expected.len(),
                        "Duplicate rays must not erase a second layer"
                    );
                    assert_eq!(proposal.proposal().preview().value([8, 7, 4]), Some(1));
                    assert!(proposal.is_current(&current, &settings, &points));
                    let result = proposal
                        .apply(&current, &settings, &points, || false)
                        .unwrap();
                    assert_ne!(result.revision(), base.revision());
                    assert_eq!(result.value([8, 7, 4]), Some(1));
                    assert_eq!(base.value([8, 7, 4]), Some(1));
                    for cell in expected {
                        assert_eq!(result.value(cell), Some(0));
                        assert_eq!(base.value(cell), Some(1));
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
fn section_erase_revalidates_scene_base_settings_frozen_image_and_revoked_scope() {
    let (d, q) = device(None);
    let gpu = GpuVolumeSource::new(&d, &q, source([17, 15, 9], 1, false, false), || false).unwrap();
    let geometry = surface::geometry(gpu.clone());
    let projector = GpuSurfaceProjector::new(gpu.clone()).unwrap();
    let mut renderer =
        SectionRenderer::new(gpu.clone(), wgpu::TextureFormat::Rgba8UnormSrgb).unwrap();
    let mut scene = scene(gpu);
    scene.material = material(0.);
    scene.surface = Some(SectionSurface {
        image: surface::project(&projector, geometry, &scene),
        luminance: 0.8,
        lighting: Lighting::HEADLIGHT,
    });
    let frame = complete(&mut renderer, &scene);
    let current = SectionPickState::from_frame(&frame).unwrap();
    let batch = renderer
        .probe_region(&frame, &current, [[7, 8], [17, 11]], || false)
        .unwrap();
    let settings = brush(&scene);
    let points = [renderer.source.volume().center()];
    let make = || {
        SectionFaceErase::prepare(
            &current,
            std::slice::from_ref(&batch),
            &settings,
            &points,
            || false,
        )
        .unwrap()
    };
    let edits: [fn(&mut SectionPickState); 13] = [
        |s| s.scope = PickScope::new().unwrap(),
        |s| s.number = 2,
        |s| s.scene.navigation.pan[0] += 0.01,
        |s| s.scene.volume.camera.yaw_degrees += 1.,
        |s| s.scene.size[0] += 1,
        |s| {
            s.scene.volume.quality = match s.scene.volume.quality {
                Quality::Interactive => Quality::Refined,
                Quality::Refined => Quality::Interactive,
            }
        },
        |s| s.scene.material = Arc::new((*s.scene.material).clone()),
        |s| s.scene.lighting = Lighting::HEADLIGHT,
        |s| s.scene.surface.as_mut().unwrap().luminance = 0.7,
        |s| s.scene.surface.as_mut().unwrap().lighting = Lighting::UNLIT,
        |s| s.scene.planes[0] = None,
        |s| s.scene.planes[1].as_mut().unwrap().invert = true,
        |s| s.scene.planes[2].as_mut().unwrap().window.center += 1.,
    ];
    for (index, edit) in edits.into_iter().enumerate() {
        let mut changed = current.clone();
        edit(&mut changed);
        let proposal = make();
        assert!(
            !proposal.is_current(&changed, &settings, &points),
            "scene edit {index}"
        );
        assert!(
            proposal
                .apply(&changed, &settings, &points, || false)
                .is_err()
        );
        assert!(
            SectionFaceErase::prepare(
                &changed,
                std::slice::from_ref(&batch),
                &settings,
                &points,
                || false
            )
            .is_err()
        );
    }
    let mut copied = current.clone();
    let MaskOwner::Manual(base) = &current.map else {
        unreachable!()
    };
    copied.map = MaskOwner::Manual(Arc::new(base.try_clone(|| false).unwrap()));
    assert!(!make().is_current(&copied, &settings, &points));
    assert!(make().apply(&copied, &settings, &points, || false).is_err());
    let mut altered = settings.clone();
    altered.radius_mm = 79.;
    assert!(make().apply(&current, &altered, &points, || false).is_err());
    assert!(
        make()
            .apply(&current, &settings, &[[0.; 3]], || false)
            .is_err()
    );
    altered.clip = Some(PhysicalClip {
        minimum_mm: [-1e6; 3],
        maximum_mm: [1e6; 3],
    });
    assert!(
        SectionFaceErase::prepare(
            &current,
            std::slice::from_ref(&batch),
            &altered,
            &points,
            || false
        )
        .is_err()
    );
    assert!(SectionFaceErase::prepare(&current, &[], &settings, &points, || false).is_err());
    assert!(make().apply(&current, &settings, &points, || true).is_err());
    let second_image = complete(&mut renderer, &scene);
    let second = SectionPickFrame::new(second_image.image().clone(), frame.scope().clone());
    assert!(current.matches(&second));
    let second_batch = renderer
        .probe_region(&second, &current, [[7, 8], [17, 11]], || false)
        .unwrap();
    let p = make();
    let revoked_during_apply = make();
    assert!(
        SectionFaceErase::prepare(&current, &[batch, second_batch], &settings, &points, || {
            false
        })
        .is_err()
    );
    let calls = Cell::new(0);
    assert!(
        revoked_during_apply
            .apply(&current, &settings, &points, || {
                calls.set(calls.get() + 1);
                if calls.get() == 2 {
                    frame.scope().revoke();
                }
                false
            })
            .is_err()
    );
    assert!(!p.is_current(&current, &settings, &points));
    assert!(p.apply(&current, &settings, &points, || false).is_err());
}

#[test]
fn section_erase_admits_at_most_65536_original_raster_samples_including_duplicates() {
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
        scene.size = [257, 33];
        scene.material = material(0.);
        scene.surface = Some(SectionSurface {
            image: surface::project(&projector, geometry, &scene),
            luminance: 0.8,
            lighting: Lighting::HEADLIGHT,
        });
        let frame = complete(&mut renderer, &scene);
        let current = SectionPickState::from_frame(&frame).unwrap();
        let settings = brush(&scene);
        let points = [renderer.source.volume().center()];
        let mut batches = Vec::new();
        for _ in 0..17 {
            batches.push(
                renderer
                    .probe_region(&frame, &current, [[0, 8], [256, 16]], || false)
                    .unwrap(),
            );
        }
        let held = MemoryBudget::volumes().used();
        assert!(
            SectionFaceErase::prepare(&current, &batches, &settings, &points, || false).is_err()
        );
        assert_eq!(MemoryBudget::volumes().used(), held);
        batches.pop();
        let p =
            SectionFaceErase::prepare(&current, &batches, &settings, &points, || false).unwrap();
        let v = p.visibility();
        assert_eq!(v.hit + v.empty + v.obscured + v.ambiguous, 65536);
        assert!(p.summary().changed_voxels > 0);
        assert!(v.hit > p.summary().changed_voxels);
        drop(p);
        let count = Cell::new(0);
        assert!(
            SectionFaceErase::prepare(&current, &batches, &settings, &points, || {
                count.set(count.get() + 1);
                if count.get() == 40 {
                    frame.scope().revoke();
                }
                false
            })
            .is_err()
        );
        assert!(!frame.scope().is_active());
    }
    fence(&d);
    assert_eq!(
        before,
        [MemoryBudget::volumes().used(), MemoryBudget::gpu().used()]
    );
}

#[test]
fn section_erase_cancellation_memory_pressure_and_fully_covered_noop_do_not_publish_changes() {
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
        scene.surface = Some(SectionSurface {
            image: surface::project(&projector, geometry, &scene),
            luminance: 0.8,
            lighting: Lighting::HEADLIGHT,
        });
        let points = [renderer.source.volume().center()];
        for sigma in [0., 10.] {
            scene.material = material(sigma);
            let settings = brush(&scene);
            let frame = complete(&mut renderer, &scene);
            let current = SectionPickState::from_frame(&frame).unwrap();
            let batch = renderer
                .probe_region(&frame, &current, [[13, 11], [11, 7]], || false)
                .unwrap();
            let batches = std::slice::from_ref(&batch);
            let budget = MemoryBudget::volumes();
            let held = budget.used();
            let calls = Cell::new(0);
            let p = SectionFaceErase::prepare(&current, batches, &settings, &points, || {
                calls.set(calls.get() + 1);
                false
            })
            .unwrap();
            if sigma == 0. {
                assert!(p.summary().changed_voxels > 0);
            } else {
                assert_eq!(p.visibility().hit, 0);
                assert!(p.visibility().obscured > 0);
                assert_eq!(p.summary().changed_voxels, 0);
                let MaskOwner::Manual(base) = &current.map else {
                    unreachable!()
                };
                assert!(Arc::ptr_eq(base, p.proposal().preview()));
            }
            drop(p);
            assert_eq!(budget.used(), held);
            for stop in 0..calls.get() {
                let count = Cell::new(0);
                assert!(
                    SectionFaceErase::prepare(&current, batches, &settings, &points, || {
                        let n = count.get();
                        count.set(n + 1);
                        n == stop
                    })
                    .is_err(),
                    "sigma={sigma}/callback={stop}"
                );
                assert_eq!(budget.used(), held);
            }
            let pressure = budget.reserve(budget.limit() - budget.used()).unwrap();
            assert!(
                SectionFaceErase::prepare(&current, batches, &settings, &points, || false).is_err()
            );
            drop(pressure);
            assert_eq!(budget.used(), held);
            let p =
                SectionFaceErase::prepare(&current, batches, &settings, &points, || false).unwrap();
            let original = p.proposal().base().clone();
            let result = p.apply(&current, &settings, &points, || false).unwrap();
            assert_eq!(Arc::ptr_eq(&result, &original), sigma != 0.);
            assert_eq!(original.value([8, 7, 4]), Some(1));
            drop(result);
            drop(original);
            assert_eq!(budget.used(), held);
        }
    }
    fence(&d);
    assert_eq!(
        before,
        [MemoryBudget::volumes().used(), MemoryBudget::gpu().used()]
    );
}
