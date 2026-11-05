// SPDX-License-Identifier: GPL-3.0-or-later
//! Wide logical source with a shared constant plane; not an RSS/unique-data benchmark.
use super::*;
use orv_core::{
    study::{Geometry, demo_study},
    volume::Volume,
};
pub(super) fn run(d: &wgpu::Device, q: &wgpu::Queue) -> Result<Value> {
    let before_gpu = MemoryBudget::gpu().used();
    let before_cpu = MemoryBudget::volumes().used();
    let template = demo_study().series.remove(0).frames.remove(0);
    let plane: Arc<[f32]> = vec![0.; 4096 * 512].into();
    let frames = (0..256)
        .map(|z| {
            let mut f = (*template).clone();
            f.width = 4096;
            f.height = 512;
            f.pixels = plane.clone();
            f.geometry = Some(Geometry {
                position: [0., 0., z as f64 * 1.2],
                orientation: [1., 0., 0., 0., 1., 0.],
                spacing: [0.7, 0.7],
                frame_of_reference: Some("generated wide paging source".into()),
                nifti_transform: None,
            });
            Arc::new(f)
        })
        .collect();
    let cpu = Volume::prepare(frames, || false)?;
    let refused = 4096 > d.limits().max_texture_dimension_3d;
    if refused && GpuVolumeSource::new(d, q, cpu.clone(), || false).is_ok() {
        return Err("Dense wide source unexpectedly bypassed its texture side limit".into());
    }
    let source = GpuBrickSource::new(d, q, cpu.clone(), 64, BrickStencil::CentralSecants, || {
        false
    })?;
    let mut volume = VolumeView::fit(&cpu);
    volume.projection = Projection::Composite;
    volume.quality = Quality::Refined;
    let material = Arc::new(VolumeMaterial::new(&[-1000., 1000.].map(|value| {
        MaterialPoint {
            value,
            luminance: 0.6,
            extinction_per_mm: 0.001,
        }
    }))?);
    let scene = PagedMaterialScene {
        source: source.clone(),
        navigation: ViewState::default(),
        volume,
        material,
        lighting: Lighting::UNLIT,
        size: [128, 128],
    };
    let pool = GpuBrickAtlasPool::new(source.clone(), 128, || false)?;
    let atlas_bytes = pool.admission_bytes();
    let mut renderer = PagedMaterialRenderer::new(pool, wgpu::TextureFormat::Rgba8Unorm, || false)?;
    let start = Instant::now();
    let image = complete(&mut renderer, &scene)?;
    let elapsed = start.elapsed().as_secs_f64() * 1000.;
    let stats = stats(image.statistics());
    let observed_gpu = MemoryBudget::gpu().used();
    let observed_cpu = MemoryBudget::volumes().used();
    let (pixels, pixel_cpu) = read(d, q, image.texture())?;
    let rays = scene.volume.camera.frame(&scene.navigation, 1.)?;
    let mut nonempty = 0;
    for (i, pixel) in pixels.as_chunks::<8>().0.iter().enumerate() {
        let uv = [
            (i % 128) as f64 / 128. + 0.5 / 128.,
            (i / 128) as f64 / 128. + 0.5 / 128.,
        ];
        let (origin, direction) = rays.ray(uv);
        let length = cpu
            .clipped_ray_interval(origin, direction, None)
            .map_or(0., |p| (p[1] - p[0]).max(0.));
        nonempty += usize::from(length > 0.);
        let t = (-0.001 * length).exp();
        let expected = 0.6 * (1. - t) + t * 0.0030352698;
        let bits = u16::from_le_bytes([pixel[0], pixel[1]]);
        let exponent = (bits >> 10) & 31;
        let fraction = bits & 1023;
        let value = if exponent == 0 {
            fraction as f64 * 2f64.powi(-24)
        } else {
            (1. + fraction as f64 / 1024.) * 2f64.powi(exponent as i32 - 15)
        };
        if (value - expected).abs() > 0.0006 {
            return Err("Wide paged image differs from independent physical rays".into());
        }
    }
    if nonempty < 100 {
        return Err("Wide benchmark did not intersect enough source rays".into());
    }
    let count = source.layout().len();
    drop((
        pixels, pixel_cpu, image, renderer, scene, source, cpu, plane,
    ));
    fence(d)?;
    if MemoryBudget::gpu().used() != before_gpu || MemoryBudget::volumes().used() != before_cpu {
        return Err("Wide paging workload did not restore admission".into());
    }
    Ok(
        json!({"source_dimensions":[4096,512,256],"logical_scalar_bytes":2147483648u64,"shared_cpu_plane_bytes":8388608,"source_storage":"all256 frames share one constant plane; logical admission is not actual RSS or unique-data throughput","source_layout_pages":count,"core_cell_side":64,"atlas_slots":128,"atlas_admission_bytes":atlas_bytes,"device_max_texture_dimension_3d":d.limits().max_texture_dimension_3d,"dense_refused_by_texture_side":refused,"output_size":[128,128],"samples":1,"warmups":0,"complete_worker_ms":elapsed,"statistics":stats,"verified_pixels_against_f64_physical_rays":16384,"intersecting_source_pixels":nonempty,"observed_after_complete_image_gpu_admission_bytes":observed_gpu,"observed_after_complete_image_cpu_admission_bytes":observed_cpu,"before":{"gpu":before_gpu,"cpu":before_cpu},"after":{"gpu":MemoryBudget::gpu().used(),"cpu":MemoryBudget::volumes().used()},"scope":"complete worker rendering after CPU source and pipeline/cache preparation, with upload/status/retirement work; verification and native presentation excluded"}),
    )
}
