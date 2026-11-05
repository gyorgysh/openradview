// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use orv_core::{
    study::{Geometry, demo_study},
    volume::Volume,
    volume_ranges::VolumeRanges,
};

fn field(
    dimensions: [usize; 3],
    reverse: bool,
    sample: impl Fn([usize; 3]) -> (f32, bool),
) -> Arc<Volume> {
    let template = demo_study().series.remove(0).frames.remove(0);
    let frames = (0..dimensions[2])
        .map(|z| {
            let mut frame = (*template).clone();
            frame.width = dimensions[0] as u32;
            frame.height = dimensions[1] as u32;
            let samples: Vec<_> = (0..dimensions[1])
                .flat_map(|y| (0..dimensions[0]).map(move |x| [x, y, z]))
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
                frame_of_reference: Some("generated paged classification".into()),
                nifti_transform: None,
            });
            Arc::new(frame)
        })
        .collect();
    Arc::new(Volume::new(frames).unwrap())
}
fn mapping(points: &[(f32, f32)]) -> Arc<VolumeMaterial> {
    Arc::new(
        VolumeMaterial::new(
            &points
                .iter()
                .enumerate()
                .map(|(i, &(value, extinction_per_mm))| MaterialPoint {
                    value,
                    extinction_per_mm,
                    luminance: [0.1, 0.8, 0.2, 0.7][i % 4],
                })
                .collect::<Vec<_>>(),
        )
        .unwrap(),
    )
}
fn sparse_material() -> Arc<VolumeMaterial> {
    mapping(&[(-1000., 0.), (-700., 0.), (0., 0.03), (1000., 0.008)])
}
fn classified(
    source: &Arc<GpuBrickSource>,
    material: &Arc<VolumeMaterial>,
    side: usize,
) -> Arc<GpuPagedMaterialRanges> {
    let ranges = VolumeRanges::build(source.source().clone(), side, || false).unwrap();
    GpuPagedMaterialRanges::new(source.clone(), ranges, material.clone(), || false).unwrap()
}

#[test]
fn paged_zero_classification_keeps_every_dense_pixel_across_page_and_range_sizes() {
    let (d, q) = device(None);
    for (case, cell, range) in [
        (0, 32, 8),
        (0, 32, 64),
        (0, 32, 128),
        (0, 64, 16),
        (0, 64, 32),
        (0, 128, 8),
        (0, 128, 128),
        (1, 32, 16),
        (1, 64, 64),
        (2, 32, 8),
        (3, 64, 32),
    ] {
        let cpu = field([137, 69, 39], case % 2 != 0, |[x, y, z]| {
            let peak =
                [8, 32, 64, 128].contains(&x) && (29..36).contains(&y) && (15..23).contains(&z);
            match case {
                1 if z.is_multiple_of(11) => (if x % 2 == 0 { 1e20 } else { -1e20 }, false),
                2 => (if x % 2 == 0 { -1000. } else { 1000. }, true),
                3 => (0., true),
                _ => (if peak { 200. } else { -1000. }, true),
            }
        });
        let source =
            GpuBrickSource::new(&d, &q, cpu, cell, BrickStencil::CentralSecants, || false).unwrap();
        let mut scene = scene(source.clone(), [43, 29]);
        scene.material = match case {
            2 => mapping(&[(-1000., 0.), (0., 0.03), (1000., 0.)]),
            3 => mapping(&[(-1., 0.02), (0., 0.), (1., 0.02)]),
            _ => sparse_material(),
        };
        if case == 1 {
            let c = source.source().center();
            scene.volume.clip = Some(PhysicalClip {
                minimum_mm: std::array::from_fn(|a| c[a] - 45.),
                maximum_mm: std::array::from_fn(|a| c[a] + 45.),
            });
        }
        let flags = classified(&source, &scene.material, range);
        if case == 0 && range <= 32 {
            assert!(flags.transparent_tiles() > 0);
        }
        if case >= 2 {
            assert_eq!(flags.transparent_tiles(), 0);
        }
        let expected = dense(&d, &q, &scene);
        for capacity in [1, source.layout().len()] {
            let pool = GpuBrickAtlasPool::new(source.clone(), capacity, || false).unwrap();
            let mut renderer = PagedMaterialRenderer::new_accelerated(
                pool,
                flags.clone(),
                wgpu::TextureFormat::Rgba8Unorm,
                || false,
            )
            .unwrap();
            let image = complete(&mut renderer, &scene, 7);
            assert_eq!(
                read(&d, &q, image.texture()),
                expected,
                "case{case}/cell{cell}/range{range}/capacity{capacity}"
            );
            assert!(Arc::ptr_eq(image.ranges().unwrap(), &flags));
            if capacity == source.layout().len() {
                let required: Vec<_> = flags.required_indices().collect();
                renderer.atlas.load(&required, &|| false).unwrap();
                let next = renderer.render_complete(&scene, 1, || false).unwrap();
                assert_eq!(next.statistics().fenced_waves, 1);
                assert_eq!(next.statistics().source_bytes_uploaded, 0);
                assert_eq!(read(&d, &q, next.texture()), expected);
                assert_eq!(read(&d, &q, image.texture()), expected);
            }
        }
    }
}

#[test]
fn proven_zero_samples_need_no_resident_source_even_with_missing_or_extreme_vertices() {
    let (d, q) = device(Some(64));
    for kind in 0..3 {
        let cpu = field([129, 9, 7], false, |[x, y, z]| {
            (
                if kind == 0 {
                    -1000.
                } else if (x + y + z) % 2 == 0 {
                    1e20
                } else {
                    -1e20
                },
                kind != 2,
            )
        });
        assert!(GpuVolumeSource::new(&d, &q, cpu.clone(), || false).is_err());
        let source =
            GpuBrickSource::new(&d, &q, cpu, 32, BrickStencil::CentralSecants, || false).unwrap();
        let mut scene = scene(source.clone(), [2048, 17]);
        scene.material = mapping(&[(-1., 0.), (1., 0.)]);
        let flags = classified(&source, &scene.material, 8);
        assert_eq!(flags.transparent_tiles(), flags.ranges().summaries().len());
        assert_eq!(flags.required_pages(), 0);
        assert_eq!(flags.page_required(source.layout().len()), None);
        if kind == 2 {
            assert!(flags.ranges().summaries().iter().all(|s| s.all_missing()));
        }
        let pool = GpuBrickAtlasPool::new(source.clone(), 1, || false).unwrap();
        let mut renderer = PagedMaterialRenderer::new_accelerated(
            pool,
            flags.clone(),
            wgpu::TextureFormat::Rgba8Unorm,
            || false,
        )
        .unwrap();
        let image = renderer.render_complete(&scene, 0, || false).unwrap();
        assert_eq!(image.statistics().fenced_waves, 1);
        assert_eq!(image.statistics().source_pages_uploaded, 0);
        assert_eq!(renderer.atlas.resident_indices().count(), 0);
        let expected = read(&d, &q, image.texture());
        assert!(expected.iter().all(|p| *p == expected[0]));
        // Background in IEEE float16, independently rounded from the shader's
        // existing linear background constant 0.0030352698.
        assert_eq!(expected[0], [0x1a37, 0x1a37, 0x1a37, 0x3c00]);
        let rows = complete(&mut renderer, &scene, 7);
        assert_eq!(read(&d, &q, rows.texture()), expected);
        assert_eq!(rows.statistics().source_bytes_uploaded, 0);
    }
}

#[test]
fn required_pages_cover_thin_vertex_features_at_range_and_page_boundaries() {
    let (d, q) = device(None);
    let peaks = [[32, 17, 9], [64, 32, 16], [128, 65, 37]];
    let cpu = field([137, 69, 39], false, |p| {
        (if peaks.contains(&p) { 200. } else { -1000. }, true)
    });
    for cell in [32, 64, 128] {
        let source = GpuBrickSource::new(
            &d,
            &q,
            cpu.clone(),
            cell,
            BrickStencil::CentralSecants,
            || false,
        )
        .unwrap();
        for side in [8, 16, 32, 64, 128] {
            let flags = classified(&source, &sparse_material(), side);
            let mut expected = vec![false; source.layout().len()];
            let mut transparent = 0;
            for i in 0..flags.ranges().summaries().len() {
                let tile = flags.ranges().tile(i).unwrap();
                let nonzero = peaks.iter().any(|p| {
                    (0..3).all(|a| p[a] >= tile.origin[a] && p[a] <= tile.origin[a] + tile.cells[a])
                });
                if !nonzero {
                    transparent += 1;
                    continue;
                }
                // Intersect the page's owned cells with the nonzero summary's
                // cells; a feature on a vertex can affect both adjacent tiles.
                for (page, required) in expected.iter_mut().enumerate() {
                    let bounds = source.layout().cells(page).unwrap();
                    *required |= (0..3).all(|a| {
                        bounds[0][a] < tile.origin[a] + tile.cells[a]
                            && bounds[1][a] > tile.origin[a]
                    });
                }
            }
            assert_eq!(flags.transparent_tiles(), transparent);
            assert_eq!(
                flags.required_pages(),
                expected.iter().filter(|v| **v).count()
            );
            for (i, required) in expected.into_iter().enumerate() {
                assert_eq!(
                    flags.page_required(i),
                    Some(required),
                    "cell{cell}/range{side}/page{i}"
                );
            }
        }
    }
}

#[test]
fn zero_coefficients_cannot_bypass_unsupported_values_in_an_unread_gradient_halo() {
    let (d, q) = device(None);
    let cpu = field([69, 9, 9], false, |[x, _, _]| {
        (if x == 34 { 1e30 } else { 0. }, true)
    });
    assert!(GpuVolumeSource::new(&d, &q, cpu.clone(), || false).is_err());
    let source =
        GpuBrickSource::new(&d, &q, cpu, 32, BrickStencil::CentralSecants, || false).unwrap();
    let mut scene = scene(source.clone(), [19, 17]);
    scene.material = mapping(&[(-1., 0.), (1., 0.)]);
    let c = source.source().voxel_to_world([2., 4., 4.]);
    scene.volume.clip = Some(PhysicalClip {
        minimum_mm: c.map(|v| v - 2.),
        maximum_mm: c.map(|v| v + 2.),
    });
    let flags = classified(&source, &scene.material, 8);
    assert_eq!(flags.page_required(0), Some(true));
    assert_eq!(flags.page_required(1), Some(true));
    assert_eq!(flags.page_required(2), Some(false));
    let pool = GpuBrickAtlasPool::new(source, 1, || false).unwrap();
    let mut renderer = PagedMaterialRenderer::new_accelerated(
        pool,
        flags,
        wgpu::TextureFormat::Rgba8Unorm,
        || false,
    )
    .unwrap();
    assert!(renderer.render_complete(&scene, 0, || false).is_err());
}

#[test]
fn classification_refuses_foreign_owners_knots_and_cancelled_or_unadmitted_preparation() {
    let (d, q) = device(None);
    let cpu = field([69, 19, 11], false, |_| (-1000., true));
    let source = GpuBrickSource::new(
        &d,
        &q,
        cpu.clone(),
        32,
        BrickStencil::CentralSecants,
        || false,
    )
    .unwrap();
    let ranges = VolumeRanges::build(cpu, 8, || false).unwrap();
    let m = sparse_material();
    let ram = MemoryBudget::volumes();
    let gpu = MemoryBudget::gpu();
    fence(&d);
    let base = (ram.used(), gpu.used());
    let count_calls = Cell::new(0);
    let flags = GpuPagedMaterialRanges::new(source.clone(), ranges.clone(), m.clone(), || {
        count_calls.set(count_calls.get() + 1);
        false
    })
    .unwrap();
    drop(flags);
    fence(&d);
    assert_eq!((ram.used(), gpu.used()), base);
    for cutoff in [
        0,
        1,
        source.layout().len() + 1,
        source.layout().len() + 1 + ranges.counts()[1] * ranges.counts()[2],
        count_calls.get() - 2,
        count_calls.get() - 1,
    ] {
        let calls = Cell::new(0);
        let result = GpuPagedMaterialRanges::new(source.clone(), ranges.clone(), m.clone(), || {
            let n = calls.get();
            calls.set(n + 1);
            n >= cutoff
        });
        assert!(matches!(result, Err("Cancelled")), "cutoff{cutoff}");
        fence(&d);
        assert_eq!((ram.used(), gpu.used()), base);
    }
    for budget in [&ram, &gpu] {
        let limit = budget.limit();
        budget.set_limit(budget.used());
        let result =
            GpuPagedMaterialRanges::new(source.clone(), ranges.clone(), m.clone(), || false);
        budget.set_limit(limit);
        assert!(result.is_err());
        fence(&d);
        assert_eq!((ram.used(), gpu.used()), base);
    }
    for values in [[0., f32::from_bits(1)], [0., 1e-21]] {
        assert!(
            GpuPagedMaterialRanges::new(
                source.clone(),
                ranges.clone(),
                mapping(&[(values[0], 0.), (values[1], 0.)]),
                || false
            )
            .is_err()
        );
    }
    let foreign = field([69, 19, 11], false, |_| (-1000., true));
    let foreign_ranges = VolumeRanges::build(foreign, 8, || false).unwrap();
    assert!(
        GpuPagedMaterialRanges::new(source.clone(), foreign_ranges, m.clone(), || false).is_err()
    );
    let flags = classified(&source, &m, 8);
    let other = GpuBrickSource::new(
        &d,
        &q,
        source.source().clone(),
        32,
        BrickStencil::CentralSecants,
        || false,
    )
    .unwrap();
    let pool = GpuBrickAtlasPool::new(other, 1, || false).unwrap();
    assert!(
        PagedMaterialRenderer::new_accelerated(
            pool,
            flags,
            wgpu::TextureFormat::Rgba8Unorm,
            || false
        )
        .is_err()
    );
}

#[test]
fn zero_flag_classifications_keep_the_original_two_group_gpu_fallback() {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: crate::configured_backends().unwrap(),
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapter = pollster::block_on(instance.request_adapter(&Default::default()))
        .expect("Actual GPU required");
    let mut descriptor = crate::device_descriptor(&adapter);
    descriptor.required_limits.max_bind_groups = 2;
    descriptor
        .required_limits
        .max_sampled_textures_per_shader_stage = 2;
    descriptor
        .required_limits
        .max_uniform_buffers_per_shader_stage = 2;
    let (d, q) = pollster::block_on(adapter.request_device(&descriptor)).unwrap();
    let cpu = field([39, 19, 11], false, |_| (-1000., true));
    let source =
        GpuBrickSource::new(&d, &q, cpu, 32, BrickStencil::CentralSecants, || false).unwrap();
    let mut scene = scene(source.clone(), [31, 17]);
    scene.material = mapping(&[(-1000., 0.01), (1000., 0.02)]);
    let flags = classified(&source, &scene.material, 8);
    assert_eq!(flags.transparent_tiles(), 0);
    let pool = GpuBrickAtlasPool::new(source.clone(), 1, || false).unwrap();
    let mut renderer = PagedMaterialRenderer::new_accelerated(
        pool.clone(),
        flags,
        wgpu::TextureFormat::Rgba8Unorm,
        || false,
    )
    .unwrap();
    let image = renderer.render_complete(&scene, 0, || false).unwrap();
    let expected = dense(&d, &q, &scene);
    assert_eq!(read(&d, &q, image.texture()), expected);
    let flags = classified(&source, &sparse_material(), 8);
    assert!(flags.transparent_tiles() > 0);
    assert!(
        PagedMaterialRenderer::new_accelerated(
            pool,
            flags,
            wgpu::TextureFormat::Rgba8Unorm,
            || false
        )
        .is_err()
    );
}

#[test]
fn images_retain_exact_classification_and_reject_even_equal_material_replacements() {
    let (d, q) = device(None);
    for zero in [false, true] {
        let cpu = field([69, 19, 11], false, |_| (-1000., true));
        let source =
            GpuBrickSource::new(&d, &q, cpu, 32, BrickStencil::CentralSecants, || false).unwrap();
        let mut scene = scene(source.clone(), [31, 19]);
        scene.material = if zero {
            sparse_material()
        } else {
            mapping(&[(-1000., 0.01), (1000., 0.02)])
        };
        let ram = MemoryBudget::volumes();
        let gpu = MemoryBudget::gpu();
        fence(&d);
        let base = (ram.used(), gpu.used());
        let flags = classified(&source, &scene.material, 8);
        assert_eq!(flags.transparent_tiles() != 0, zero);
        let weak = Arc::downgrade(&flags);
        let pool = GpuBrickAtlasPool::new(source.clone(), 1, || false).unwrap();
        let mut renderer = PagedMaterialRenderer::new_accelerated(
            pool,
            flags.clone(),
            wgpu::TextureFormat::Rgba8Unorm,
            || false,
        )
        .unwrap();
        let image = renderer.render_complete(&scene, 0, || false).unwrap();
        let expected = read(&d, &q, image.texture());
        let mut stale = scene.clone();
        stale.material = Arc::new((*scene.material).clone());
        assert!(renderer.render_complete(&stale, 0, || false).is_err());
        assert!(renderer.render_rows(&stale, 0, 0..7, || false).is_err());
        assert!(renderer.render_rows(&scene, 0, 0..7, || true).is_err());
        assert_eq!(read(&d, &q, image.texture()), expected);
        drop((renderer, flags));
        fence(&d);
        assert!(weak.upgrade().is_some());
        assert!(Arc::ptr_eq(
            image.ranges().unwrap().material(),
            &scene.material
        ));
        assert_eq!(read(&d, &q, image.texture()), expected);
        drop(image);
        fence(&d);
        assert!(weak.upgrade().is_none());
        assert_eq!((ram.used(), gpu.used()), base);
    }
}
