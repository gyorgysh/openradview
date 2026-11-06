// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use crate::material_ranges::GpuMaterialRanges;
use orv_core::volume_ranges::VolumeRanges;

fn sparse_volume(missing: bool) -> Arc<Volume> {
    let base = volume_geometry([35, 27, 19], 1e6, true, true);
    let frames = base
        .frames()
        .iter()
        .enumerate()
        .map(|(z, frame)| {
            let mut frame = (**frame).clone();
            let mut values = vec![0.; 35 * 27];
            let mut valid = vec![1u8; 35 * 27];
            for y in 0..27 {
                for x in 0..35 {
                    if (12..19).contains(&x) && (10..17).contains(&y) && (7..13).contains(&z) {
                        values[y * 35 + x] = 500.;
                    }
                    if missing && z == 7 && x == 15 && y == 12 {
                        valid[y * 35 + x] = 0;
                    }
                }
            }
            frame.pixels = values.into();
            frame.validity = Some(valid.into());
            Arc::new(frame)
        })
        .collect();
    Arc::new(Volume::new(frames).unwrap())
}
fn sparse_geometry(source: Arc<Volume>) -> Arc<CandidateAssembly> {
    let mut payload = vec![97u8; 35 * 27 * 2];
    for z in 0..2 {
        for y in 0..3 {
            for x in 0..3 {
                if (x % 2 == 0 && y % 2 == 0 && z == 0) || (x == 1 && y == 1 && z == 1) {
                    payload[z * 35 * 27 + (10 + y) * 35 + 12 + x] = 113;
                }
            }
        }
    }
    let mask = Arc::new(
        Segments::from_fractional(
            source,
            &[1],
            &[(1, 8), (1, 9)],
            &payload,
            FractionalEncoding {
                kind: FractionalKind::Probability,
                maximum: 200,
            },
            Some(true),
            || false,
        )
        .unwrap(),
    );
    let field = MaskField::new(MaskOwner::Supplied(mask), 1).unwrap();
    let limits = grid_limits(1000., 2);
    let grid = field
        .candidate_grid([12, 10, 8], [2, 2, 1], 2, limits, || false)
        .unwrap();
    assembly(&grid, limits.seams)
}
fn sparse_material() -> Arc<VolumeMaterial> {
    Arc::new(
        VolumeMaterial::new(&[
            MaterialPoint {
                value: 0.,
                luminance: 0.2,
                extinction_per_mm: 0.,
            },
            MaterialPoint {
                value: 100.,
                luminance: 0.2,
                extinction_per_mm: 0.,
            },
            MaterialPoint {
                value: 101.,
                luminance: 0.2,
                extinction_per_mm: 0.03,
            },
            MaterialPoint {
                value: 1000.,
                luminance: 0.4,
                extinction_per_mm: 0.03,
            },
        ])
        .unwrap(),
    )
}
#[test]
fn accelerated_candidate_optics_match_dense_for_sparse_clipped_and_missing_sources() {
    let (device, queue) = device();
    for missing in [false, true] {
        let cpu = sparse_volume(missing);
        let source = GpuVolumeSource::new(&device, &queue, cpu.clone(), || false).unwrap();
        let (surface, _) = GpuCandidateSurface::upload(
            source.clone(),
            sparse_geometry(cpu.clone()),
            None,
            CandidateUploadLimits::default(),
            || false,
        )
        .unwrap();
        assert!(!surface.chunks().is_empty());
        let material = sparse_material();
        let ranges = GpuMaterialRanges::new(
            source.clone(),
            VolumeRanges::build(cpu.clone(), 8, || false).unwrap(),
            material.clone(),
            || false,
        )
        .unwrap();
        assert!(ranges.transparent_tiles() > 0);
        let fast = CandidateOpaqueRenderer::new_accelerated(
            ranges.clone(),
            wgpu::TextureFormat::Rgba8Unorm,
        )
        .unwrap();
        let dense =
            CandidateOpaqueRenderer::new(source.clone(), wgpu::TextureFormat::Rgba8Unorm).unwrap();
        let projector = GpuCandidateProjector::new(source.clone()).unwrap();
        for clipped in [false, true] {
            let mut volume = VolumeView::fit(&cpu);
            volume.projection = Projection::Composite;
            volume.camera.target_mm = cpu.voxel_to_world([13., 11., 8.5]);
            volume.camera.vertical_span_mm = 12.;
            volume.camera.yaw_degrees = 27.;
            volume.camera.pitch_degrees = -17.;
            if clipped {
                let center = volume.camera.target_mm;
                volume.clip = Some(PhysicalClip {
                    minimum_mm: [center[0] - 10., center[1] - 10., center[2] - 10.],
                    maximum_mm: [center[0] + 10., center[1] + 10., center[2] + 10.],
                });
            }
            let navigation = ViewState::default();
            let candidate = projector
                .render(
                    CandidateProjection {
                        surface: surface.clone(),
                        camera: volume.camera.clone(),
                        display: navigation.clone(),
                        clip: volume.clip.clone(),
                        size: [69, 61],
                    },
                    || false,
                )
                .unwrap();
            assert!(image_pixels(&candidate).0.iter().any(|cell| cell[3] != 0));
            let scene = CandidateOpaqueScene {
                surface: candidate,
                navigation,
                volume,
                material: material.clone(),
                lighting: Lighting::HEADLIGHT,
                surface_lighting: Lighting::HEADLIGHT,
                surface_luminance: 0.8,
            };
            let accelerated = fast.render(scene.clone(), || false).unwrap();
            let ordinary = dense.render(scene.clone(), || false).unwrap();
            assert!(Arc::ptr_eq(accelerated.material_ranges().unwrap(), &ranges));
            assert!(ordinary.material_ranges().is_none());
            let a = optical_pixels(&accelerated);
            let b = optical_pixels(&ordinary);
            assert_eq!(a.len(), b.len());
            for (i, (a, b)) in a.iter().zip(&b).enumerate() {
                for channel in 0..4 {
                    assert!(
                        (a[channel] - b[channel]).abs() <= 0.001,
                        "pixel {i}, channel {channel}: {} vs {}",
                        a[channel],
                        b[channel]
                    );
                }
            }
            let mut stale = scene;
            stale.material = candidate_material(0.2, 0.2);
            assert!(fast.render(stale, || false).is_err());
            assert!(fast.render(accelerated.scene().clone(), || true).is_err());
        }
    }
}

#[test]
fn empty_range_flags_keep_the_dense_path_and_reject_stale_material_or_source() {
    let (device, queue) = device();
    let cpu = volume();
    let source = GpuVolumeSource::new(&device, &queue, cpu.clone(), || false).unwrap();
    let (surface, _) = GpuCandidateSurface::upload(
        source.clone(),
        geometry(cpu.clone(), 1000., 2),
        None,
        CandidateUploadLimits::default(),
        || false,
    )
    .unwrap();
    let mut volume = VolumeView::fit(&cpu);
    volume.projection = Projection::Composite;
    let navigation = ViewState::default();
    let candidate = GpuCandidateProjector::new(source.clone())
        .unwrap()
        .render(
            CandidateProjection {
                surface,
                camera: volume.camera.clone(),
                display: navigation.clone(),
                clip: None,
                size: [37, 33],
            },
            || false,
        )
        .unwrap();
    let material = candidate_material(0.2, 0.4);
    let ranges = GpuMaterialRanges::new(
        source.clone(),
        VolumeRanges::build(cpu.clone(), 8, || false).unwrap(),
        material.clone(),
        || false,
    )
    .unwrap();
    assert_eq!(ranges.transparent_tiles(), 0);
    let renderer =
        CandidateOpaqueRenderer::new_accelerated(ranges, wgpu::TextureFormat::Rgba8Unorm).unwrap();
    let scene = CandidateOpaqueScene {
        surface: candidate,
        navigation,
        volume,
        material,
        lighting: Lighting::UNLIT,
        surface_lighting: Lighting::HEADLIGHT,
        surface_luminance: 0.85,
    };
    let image = renderer.render(scene.clone(), || false).unwrap();
    assert!(image.material_ranges().is_none());
    assert_optical(&image);
    let before = optical_pixels(&image);
    let calls = Cell::new(0);
    assert!(
        renderer
            .render(scene.clone(), || {
                calls.set(calls.get() + 1);
                calls.get() >= 3
            })
            .is_err()
    );
    assert_eq!(optical_pixels(&image), before);
    let foreign_source = GpuVolumeSource::new(&device, &queue, cpu, || false).unwrap();
    let foreign =
        CandidateOpaqueRenderer::new(foreign_source, wgpu::TextureFormat::Rgba8Unorm).unwrap();
    assert!(foreign.render(scene, || false).is_err());
}
