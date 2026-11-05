// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use crate::{
    brick_source::tests::{device, source},
    material_volume::MaterialVolumeRenderer,
    volume::VolumeTarget,
    volume_source::GpuVolumeSource,
};
use orv_core::{
    volume_material::MaterialPoint,
    volume_view::{PhysicalClip, Projection, Quality},
};
use std::{cell::Cell, time::Duration};
fn fence(d: &wgpu::Device) {
    d.poll(wgpu::PollType::Wait {
        submission_index: None,
        timeout: Some(Duration::from_secs(30)),
    })
    .unwrap();
}
fn material() -> Arc<VolumeMaterial> {
    Arc::new(
        VolumeMaterial::new(&[
            MaterialPoint {
                value: -200.,
                luminance: 0.2,
                extinction_per_mm: 0.002,
            },
            MaterialPoint {
                value: 0.,
                luminance: 0.8,
                extinction_per_mm: 0.04,
            },
            MaterialPoint {
                value: 150.,
                luminance: 0.1,
                extinction_per_mm: 0.009,
            },
            MaterialPoint {
                value: 350.,
                luminance: 0.7,
                extinction_per_mm: 0.003,
            },
            MaterialPoint {
                value: 600.,
                luminance: 0.9,
                extinction_per_mm: 0.005,
            },
        ])
        .unwrap(),
    )
}
fn scene(source: Arc<GpuBrickSource>, size: [u32; 2]) -> PagedMaterialScene {
    let mut volume = VolumeView::fit(source.source());
    volume.projection = Projection::Composite;
    volume.quality = Quality::Refined;
    volume.camera.yaw_degrees = 23.;
    volume.camera.pitch_degrees = -17.;
    volume.camera.roll_degrees = 11.;
    PagedMaterialScene {
        source,
        navigation: ViewState::default(),
        volume,
        material: material(),
        lighting: Lighting::HEADLIGHT,
        size,
    }
}
fn read(d: &wgpu::Device, q: &wgpu::Queue, texture: &wgpu::Texture) -> Vec<[u16; 4]> {
    let s = texture.size();
    let stride = (s.width * 8).next_multiple_of(256);
    let bytes = u64::from(stride) * u64::from(s.height);
    let _gpu = allocation::reserve_bytes(bytes, d, q).unwrap();
    let _cpu = MemoryBudget::volumes().reserve(bytes).unwrap();
    let buffer = d.create_buffer(&wgpu::BufferDescriptor {
        label: Some("generated paged optical comparison"),
        size: bytes,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = d.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(stride),
                rows_per_image: Some(s.height),
            },
        },
        s,
    );
    q.submit([encoder.finish()]);
    let (tx, rx) = mpsc::sync_channel(1);
    buffer.map_async(wgpu::MapMode::Read, .., move |r| {
        let _ = tx.send(r);
    });
    fence(d);
    rx.recv_timeout(Duration::from_secs(10)).unwrap().unwrap();
    let mapped = buffer.get_mapped_range(..).unwrap();
    let mut values = Vec::new();
    for row in mapped.chunks(stride as usize) {
        for p in row[..s.width as usize * 8].as_chunks::<8>().0 {
            let v = std::array::from_fn(|i| u16::from_le_bytes([p[i * 2], p[i * 2 + 1]]));
            assert!(
                v[..3]
                    .iter()
                    .all(|v| *v & 0x7c00 != 0x7c00 && *v & 0x8000 == 0)
            );
            assert_eq!(v[3], 0x3c00);
            values.push(v);
        }
    }
    drop(mapped);
    buffer.unmap();
    values
}
fn dense(d: &wgpu::Device, q: &wgpu::Queue, scene: &PagedMaterialScene) -> Vec<[u16; 4]> {
    let source = GpuVolumeSource::new(d, q, scene.source.source().clone(), || false).unwrap();
    let mut renderer =
        MaterialVolumeRenderer::new_dense_reference(source, wgpu::TextureFormat::Rgba8Unorm)
            .unwrap();
    let mut encoder = d.create_command_encoder(&Default::default());
    renderer
        .render_target(
            &mut encoder,
            &scene.navigation,
            &scene.volume,
            &scene.material,
            scene.lighting,
            VolumeTarget {
                slot: 0,
                size: scene.size,
                region: [[0, 0], scene.size],
            },
        )
        .unwrap();
    q.submit([encoder.finish()]);
    read(d, q, renderer.output_texture(0).unwrap())
}
fn complete(
    renderer: &mut PagedMaterialRenderer,
    scene: &PagedMaterialScene,
    rows: u32,
) -> Arc<PagedMaterialImage> {
    let mut image = None;
    for start in (0..scene.size[1]).step_by(rows as usize) {
        let end = (start + rows).min(scene.size[1]);
        let next = renderer
            .render_rows(scene, 0, start..end, || false)
            .unwrap();
        if end < scene.size[1] {
            assert!(next.is_none());
        } else {
            image = next;
        }
    }
    image.unwrap()
}
#[test]
fn complete_paged_images_keep_dense_global_sampling_across_tiny_cache_eviction_and_tile_batches() {
    let (d, q) = device(None);
    for (kind, missing, reverse, lighting) in [
        (0, false, false, Lighting::HEADLIGHT),
        (0, true, true, Lighting::HEADLIGHT),
        (1, false, true, Lighting::UNLIT),
        (2, true, false, Lighting::UNLIT),
        (3, false, false, Lighting::HEADLIGHT),
    ] {
        let cpu = source([69, 39, 35], kind, missing, reverse);
        let source =
            GpuBrickSource::new(&d, &q, cpu, 32, BrickStencil::CentralSecants, || false).unwrap();
        let mut scene = scene(source.clone(), [31, 23]);
        scene.lighting = lighting;
        let expected = dense(&d, &q, &scene);
        for capacity in [1, source.layout().len()] {
            let pool = GpuBrickAtlasPool::new(source.clone(), capacity, || false).unwrap();
            let mut renderer =
                PagedMaterialRenderer::new(pool, wgpu::TextureFormat::Rgba8Unorm, || false)
                    .unwrap();
            let image = complete(&mut renderer, &scene, 7);
            assert!(Arc::ptr_eq(&image.scene().source, &source));
            assert!(Arc::ptr_eq(&image.scene().material, &scene.material));
            assert_eq!(
                read(&d, &q, image.texture()),
                expected,
                "case{kind}/{missing}/{reverse}/capacity{capacity}"
            );
            assert!(image.statistics().fenced_waves > image.statistics().tile_batches);
            let old = read(&d, &q, image.texture());
            let next = complete(&mut renderer, &scene, 16);
            assert_eq!(read(&d, &q, next.texture()), expected);
            assert_eq!(read(&d, &q, image.texture()), old);
            if capacity == source.layout().len() {
                assert_eq!(next.statistics().source_bytes_uploaded, 0);
                assert_eq!(next.statistics().atlas_bytes_copied, 0);
                let indices = (0..source.layout().len()).collect::<Vec<_>>();
                renderer.atlas.load(&indices, &|| false).unwrap();
                let fast = renderer.render_complete(&scene, 0, || false).unwrap();
                assert_eq!(read(&d, &q, fast.texture()), expected);
                assert_eq!(fast.statistics().fenced_waves, 1);
                assert_eq!(fast.statistics().tile_batches, 1);
                assert_eq!(fast.statistics().source_bytes_uploaded, 0);
                assert_eq!(read(&d, &q, image.texture()), old);
            }
        }
    }
}
#[test]
fn paged_rendering_exceeds_the_device_source_side_and_keeps_requested_2048_output() {
    let (d, q) = device(Some(64));
    let cpu = source([129, 9, 7], 1, false, false);
    assert!(GpuVolumeSource::new(&d, &q, cpu.clone(), || false).is_err());
    let source =
        GpuBrickSource::new(&d, &q, cpu, 32, BrickStencil::CentralSecants, || false).unwrap();
    let mut scene = scene(source.clone(), [2048, 3]);
    scene.lighting = Lighting::UNLIT;
    scene.volume.camera.yaw_degrees = 0.;
    scene.volume.camera.pitch_degrees = 0.;
    scene.volume.camera.roll_degrees = 0.;
    let pool = GpuBrickAtlasPool::new(source, 1, || false).unwrap();
    let mut renderer =
        PagedMaterialRenderer::new(pool, wgpu::TextureFormat::Rgba8Unorm, || false).unwrap();
    let image = complete(&mut renderer, &scene, 1);
    assert_eq!(image.texture().width(), 2048);
    assert_eq!(image.texture().height(), 3);
    let (other, other_queue) = device(None);
    let reference = dense(&other, &other_queue, &scene);
    assert_eq!(read(&d, &q, image.texture()), reference);
    assert!(image.statistics().source_pages_uploaded > 1);
}
#[test]
fn homogeneous_physical_attenuation_matches_independent_f64_rays_in_large_offset_clipped_affine_geometry()
 {
    let (d, q) = device(None);
    let cpu = source([69, 39, 35], 3, false, true);
    let source =
        GpuBrickSource::new(&d, &q, cpu, 32, BrickStencil::CentralSecants, || false).unwrap();
    let mut scene = scene(source.clone(), [35, 29]);
    scene.lighting = Lighting::UNLIT;
    scene.material = Arc::new(
        VolumeMaterial::new(&[-1000., 1000.].map(|value| MaterialPoint {
            value,
            luminance: 0.6,
            extinction_per_mm: 0.02,
        }))
        .unwrap(),
    );
    let centre = source.source().center();
    scene.volume.clip = Some(PhysicalClip {
        minimum_mm: std::array::from_fn(|a| centre[a] - 20.),
        maximum_mm: std::array::from_fn(|a| centre[a] + 20.),
    });
    let pool = GpuBrickAtlasPool::new(source.clone(), 1, || false).unwrap();
    let mut renderer =
        PagedMaterialRenderer::new(pool, wgpu::TextureFormat::Rgba8Unorm, || false).unwrap();
    let image = complete(&mut renderer, &scene, 16);
    let pixels = read(&d, &q, image.texture());
    let rays = scene
        .volume
        .camera
        .frame(
            &scene.navigation,
            scene.size[0] as f64 / scene.size[1] as f64,
        )
        .unwrap();
    let mut nonempty = 0;
    for (i, pixel) in pixels.iter().enumerate() {
        let uv = [
            (i % scene.size[0] as usize) as f64 + 0.5,
            (i / scene.size[0] as usize) as f64 + 0.5,
        ];
        let uv = [uv[0] / scene.size[0] as f64, uv[1] / scene.size[1] as f64];
        let (origin, direction) = rays.ray(uv);
        let interval =
            source
                .source()
                .clipped_ray_interval(origin, direction, scene.volume.clip.as_ref());
        let length = interval.map_or(0., |p| (p[1] - p[0]).max(0.));
        nonempty += usize::from(length > 0.);
        let transmission = (-0.02 * length).exp();
        let expected = 0.6 * (1. - transmission) + transmission * 0.0030352698;
        let bits = pixel[0];
        let exp = (bits >> 10) & 31;
        let fraction = bits & 1023;
        let actual = if exp == 0 {
            fraction as f64 * 2f64.powi(-24)
        } else {
            (1. + fraction as f64 / 1024.) * 2f64.powi(exp as i32 - 15)
        };
        assert!(
            (actual - expected).abs() < 0.0006,
            "pixel{i} {actual}/{expected} length{length}"
        );
    }
    assert!(nonempty > 100);
}
#[test]
fn cancelled_unadmitted_stale_and_reordered_work_cannot_publish_or_change_a_completed_front() {
    let (d, q) = device(None);
    let cpu = source([129, 9, 7], 0, false, false);
    let before_gpu = MemoryBudget::gpu().used();
    let before_cpu = MemoryBudget::volumes().used();
    let source = GpuBrickSource::new(
        &d,
        &q,
        cpu.clone(),
        32,
        BrickStencil::CentralSecants,
        || false,
    )
    .unwrap();
    let scene = scene(source.clone(), [31, 13]);
    let pool = GpuBrickAtlasPool::new(source.clone(), 1, || false).unwrap();
    let mut renderer =
        PagedMaterialRenderer::new(pool, wgpu::TextureFormat::Rgba8Unorm, || false).unwrap();
    let image = complete(&mut renderer, &scene, 7);
    let old = read(&d, &q, image.texture());
    assert!(renderer.render_rows(&scene, 0, 1..3, || false).is_err());
    assert!(renderer.render_rows(&scene, 0, 0..17, || false).is_err());
    assert!(renderer.render_rows(&scene, 2, 0..3, || false).is_err());
    assert!(
        renderer
            .render_rows(&scene, 0, 0..3, || false)
            .unwrap()
            .is_none()
    );
    let mut stale = scene.clone();
    stale.material = Arc::new((*scene.material).clone());
    assert!(renderer.render_rows(&stale, 0, 3..6, || false).is_err());
    assert!(renderer.render_rows(&scene, 0, 3..6, || false).is_err());
    drop(stale);
    for cutoff in [0, 4, 100] {
        let calls = Cell::new(0);
        let result = renderer.render_rows(&scene, 0, 0..13, || {
            let n = calls.get();
            calls.set(n + 1);
            n >= cutoff
        });
        assert!(result.is_err(), "cutoff{cutoff}");
        assert_eq!(read(&d, &q, image.texture()), old);
    }
    let recovered = complete(&mut renderer, &scene, 7);
    assert_eq!(read(&d, &q, recovered.texture()), old);
    drop(recovered);
    let gpu = MemoryBudget::gpu();
    fence(&d);
    let limit = gpu.limit();
    gpu.set_limit(gpu.used());
    let result = renderer.render_rows(&scene, 0, 0..3, || false);
    gpu.set_limit(limit);
    assert!(result.is_err());
    let ram = MemoryBudget::volumes();
    let limit = ram.limit();
    ram.set_limit(ram.used());
    let result = renderer.render_rows(&scene, 0, 0..3, || false);
    ram.set_limit(limit);
    assert!(result.is_err());
    assert_eq!(read(&d, &q, image.texture()), old);
    let foreign =
        GpuBrickSource::new(&d, &q, cpu, 32, BrickStencil::CentralSecants, || false).unwrap();
    let mut stale = scene.clone();
    stale.source = foreign;
    assert!(renderer.render_rows(&stale, 0, 0..3, || false).is_err());
    assert!(renderer.cancel(2).is_err());
    renderer.cancel(0).unwrap();
    drop((stale, image, renderer, scene, source));
    fence(&d);
    assert_eq!(gpu.used(), before_gpu);
    assert_eq!(ram.used(), before_cpu);
}

#[test]
fn cache_constrained_complete_images_keep_global_pixels_across_partial_workgroups_and_rows() {
    let (d, q) = device(None);
    let cpu = source([133, 73, 35], 0, true, false);
    let source =
        GpuBrickSource::new(&d, &q, cpu, 32, BrickStencil::CentralSecants, || false).unwrap();
    let mut scene = scene(source.clone(), [193, 41]);
    let expected = dense(&d, &q, &scene);
    let pool = GpuBrickAtlasPool::new(source, 1, || false).unwrap();
    let mut renderer =
        PagedMaterialRenderer::new(pool, wgpu::TextureFormat::Rgba8Unorm, || false).unwrap();
    let old = renderer.render_complete(&scene, 0, || false).unwrap();
    assert_eq!(read(&d, &q, old.texture()), expected);
    assert_eq!(old.statistics().tile_batches, 3);
    scene.volume.camera.yaw_degrees = -33.;
    let expected_next = dense(&d, &q, &scene);
    let next = renderer.render_complete(&scene, 0, || false).unwrap();
    assert_eq!(read(&d, &q, next.texture()), expected_next);
    assert_eq!(read(&d, &q, old.texture()), expected);
}
