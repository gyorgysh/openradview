// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use crate::{
    opaque_surface::{OpaqueSurfaceRenderer, OpaqueSurfaceScene},
    surface_projection::{GpuSurfaceProjector, SurfaceProjection},
    surface_source::GpuCellSurface,
};
use orv_core::{
    labels::Labels,
    mask::MaskOwner,
    study::{NiftiMethod, NiftiTransform},
    surface::{CellSurface, SurfaceLimits},
};

pub(super) fn geometry(gpu: Arc<GpuVolumeSource>) -> Arc<GpuCellSurface> {
    let volume = gpu.volume();
    let frames = volume
        .frames()
        .iter()
        .enumerate()
        .map(|(z, f)| {
            let mut f = (**f).clone();
            f.validity = None;
            f.geometry.as_mut().unwrap().nifti_transform = Some(NiftiTransform {
                method: NiftiMethod::Sform,
                code: 1,
            });
            f.pixels = (0..f.pixels.len())
                .map(|i| {
                    f32::from(
                        (4..13).contains(&(i % 17))
                            && (3..12).contains(&(i / 17))
                            && (2..7).contains(&z),
                    )
                })
                .collect::<Vec<_>>()
                .into();
            Arc::new(f)
        })
        .collect();
    let map =
        Arc::new(Labels::import(volume.clone(), &Volume::new(frames).unwrap(), || false).unwrap());
    let surface = CellSurface::build(
        MaskOwner::Manual(map),
        1,
        None,
        SurfaceLimits::default(),
        || false,
    )
    .unwrap()
    .0;
    GpuCellSurface::upload(gpu, surface, None, || false)
        .unwrap()
        .0
}
pub(super) fn project(
    projector: &GpuSurfaceProjector,
    geometry: Arc<GpuCellSurface>,
    scene: &SectionScene,
) -> Arc<SurfaceImage> {
    projector
        .render(
            SurfaceProjection {
                surface: geometry,
                camera: scene.volume.camera.clone(),
                display: scene.navigation.clone(),
                clip: scene.volume.clip.clone(),
                size: scene.size,
            },
            || false,
        )
        .unwrap()
}
pub(super) fn boundary(
    surface: &SectionSurface,
    origin: [f64; 3],
    direction: [f64; 3],
) -> Option<(f64, f64)> {
    let request = surface.image.request();
    let volume = request.surface.source().volume();
    let voxel = volume.world_to_voxel(origin);
    let vector = volume.world_vector_to_voxel(direction);
    let mut nearest = None;
    for face in request
        .surface
        .surface()
        .chunks()
        .flat_map(|chunk| chunk.faces())
    {
        let side = face.side() as usize;
        let axis = side / 2;
        if vector[axis] == 0. {
            continue;
        }
        let center = face.cell().map(f64::from);
        let sign = if side.is_multiple_of(2) { -1. } else { 1. };
        let depth = (center[axis] + sign * 0.5 - voxel[axis]) / vector[axis];
        let hit: [f64; 3] = std::array::from_fn(|a| voxel[a] + vector[a] * depth);
        if (0..3).any(|a| a != axis && (hit[a] - center[a]).abs() > 0.5 + 1e-9) {
            continue;
        }
        let world = volume.voxel_to_world(hit);
        if request.clip.as_ref().is_some_and(|clip| {
            (0..3).any(|a| world[a] < clip.minimum_mm[a] || world[a] > clip.maximum_mm[a])
        }) {
            continue;
        }
        let normal = std::array::from_fn(|a| {
            volume.world_vector_to_voxel(std::array::from_fn(|i| if i == a { 1. } else { 0. }))
                [axis]
                * sign
        });
        let length = dot(normal, normal).sqrt();
        let normal = normal.map(|v| v / length);
        let light = f64::from(surface.lighting.ambient)
            + f64::from(surface.lighting.diffuse) * dot(normal, direction).abs();
        let luminance = f64::from(surface.luminance) * light;
        if nearest.is_none_or(|(t, _)| depth < t) {
            nearest = Some((depth, luminance));
        }
    }
    nearest
}
fn verify_scene(image: &SectionImage) -> (usize, usize) {
    let scene = image.scene();
    let volume = image.source().volume();
    let actual = pixels(
        image.source().device(),
        image.source().queue(),
        image.texture(),
    );
    let camera = scene
        .volume
        .camera
        .frame(
            &scene.navigation,
            f64::from(scene.size[0]) / f64::from(scene.size[1]),
        )
        .unwrap();
    let mut surfaces = 0;
    let mut planes = 0;
    for y in 0..scene.size[1] {
        for x in 0..scene.size[0] {
            let (origin, direction) = camera.ray([
                (f64::from(x) + 0.5) / f64::from(scene.size[0]),
                (f64::from(y) + 0.5) / f64::from(scene.size[1]),
            ]);
            let mut nearest = scene
                .surface
                .as_ref()
                .and_then(|s| boundary(s, origin, direction));
            let mut is_surface = nearest.is_some();
            for plane in scene.planes.iter().flatten() {
                if let Some(hit) = sample(plane, origin, direction, scene.volume.clip.as_ref())
                    && nearest.is_none_or(|(t, _)| hit.0 < t - 1e-8)
                {
                    nearest = Some(hit);
                    is_surface = false;
                }
            }
            if nearest.is_some() {
                if is_surface {
                    surfaces += 1;
                } else {
                    planes += 1;
                }
            }
            let background = nearest.map_or(0.0030352698, |(_, l)| l);
            let length = volume
                .clipped_ray_interval(origin, direction, scene.volume.clip.as_ref())
                .map_or(0., |mut range| {
                    if let Some((depth, _)) = nearest {
                        range[1] = range[1].min(depth);
                    }
                    valid_length(volume, origin, direction, range)
                });
            let transmission =
                (-f64::from(scene.material.points()[0].extinction_per_mm) * length).exp();
            let expected = f64::from(scene.material.points()[0].luminance) * (1. - transmission)
                + transmission * background;
            let pixel = actual[(y * scene.size[0] + x) as usize];
            assert_eq!(pixel[0], pixel[1]);
            assert_eq!(pixel[1], pixel[2]);
            assert_eq!(pixel[3], 0x3c00);
            assert!(
                (half(pixel[0]) - expected).abs() < 0.001,
                "mixed {x},{y}: {} versus {expected}; hit{nearest:?}",
                half(pixel[0])
            );
        }
    }
    (surfaces, planes)
}
#[test]
fn known_interfaces_and_planes_share_physical_depth_after_large_along_ray_target_shifts() {
    let (d, q) = device(None);
    for (reverse, missing) in [(false, false), (true, false), (true, true)] {
        let cpu = source([17, 15, 9], 1, false, reverse);
        let cpu = if missing {
            Arc::new(
                Volume::new(
                    cpu.frames()
                        .iter()
                        .enumerate()
                        .map(|(z, f)| {
                            let mut f = (**f).clone();
                            f.validity = Some(
                                (0..f.pixels.len())
                                    .map(|i| u8::from(z != 3 && i % 17 != 8))
                                    .collect::<Vec<_>>()
                                    .into(),
                            );
                            Arc::new(f)
                        })
                        .collect(),
                )
                .unwrap(),
            )
        } else {
            cpu
        };
        let gpu = GpuVolumeSource::new(&d, &q, cpu.clone(), || false).unwrap();
        let geometry = geometry(gpu.clone());
        let projector = GpuSurfaceProjector::new(gpu.clone()).unwrap();
        let mut renderer =
            SectionRenderer::new(gpu.clone(), wgpu::TextureFormat::Rgba8UnormSrgb).unwrap();
        let mut scene = scene(gpu);
        scene.volume.camera.yaw_degrees = 23.;
        scene.volume.camera.pitch_degrees = -17.;
        scene.volume.camera.roll_degrees = 31.;
        scene.volume.quality = Quality::Refined;
        scene.planes[1].as_mut().unwrap().invert = true;
        for (shift, clipped) in [(0., false), (500000., false), (-500000., true)] {
            let mut target = cpu.center();
            let frame = scene
                .volume
                .camera
                .frame(&scene.navigation, 37. / 29.)
                .unwrap();
            for (a, value) in target.iter_mut().enumerate() {
                *value += frame.direction_unit[a] * shift;
            }
            scene.volume.camera.target_mm = target;
            let center = cpu.center();
            scene.volume.clip = clipped.then_some(PhysicalClip {
                minimum_mm: [center[0] - 2.1, center[1] - 100., center[2] - 100.],
                maximum_mm: [center[0] + 100., center[1] + 100., center[2] + 100.],
            });
            scene.surface = Some(SectionSurface {
                image: project(&projector, geometry.clone(), &scene),
                luminance: 0.8,
                lighting: Lighting::HEADLIGHT,
            });
            let image = renderer
                .render_rows(&scene, 0, 0..scene.size[1], || false)
                .unwrap()
                .unwrap();
            let counts = verify_scene(&image);
            assert!(
                counts.0 > 10 && counts.1 > 10,
                "Exercise foreground surfaces and planes: {counts:?}"
            );
            assert!(
                renderer
                    .render_rows(&scene, 1, 0..11, || false)
                    .unwrap()
                    .is_none()
            );
            let tiled = renderer
                .render_rows(&scene, 1, 11..scene.size[1], || false)
                .unwrap()
                .unwrap();
            assert_eq!(
                pixels(&d, &q, image.texture()),
                pixels(&d, &q, tiled.texture())
            );
            assert!(Arc::ptr_eq(
                &geometry,
                &image
                    .scene()
                    .surface
                    .as_ref()
                    .unwrap()
                    .image
                    .request()
                    .surface
            ));
        }
    }
}
#[test]
fn surface_only_section_composition_matches_the_existing_surface_renderer_and_rejects_stale_views()
{
    let (d, q) = device(None);
    let before = [MemoryBudget::volumes().used(), MemoryBudget::gpu().used()];
    {
        let gpu =
            GpuVolumeSource::new(&d, &q, source([17, 15, 9], 1, false, true), || false).unwrap();
        let geometry = geometry(gpu.clone());
        let projector = GpuSurfaceProjector::new(gpu.clone()).unwrap();
        let mut scene = scene(gpu.clone());
        scene.volume.camera.yaw_degrees = 23.;
        scene.volume.camera.pitch_degrees = -17.;
        scene.planes = [None, None, None];
        let surface = project(&projector, geometry.clone(), &scene);
        scene.surface = Some(SectionSurface {
            image: surface.clone(),
            luminance: 0.8,
            lighting: Lighting::HEADLIGHT,
        });
        let mut old_renderer =
            OpaqueSurfaceRenderer::new(gpu.clone(), wgpu::TextureFormat::Rgba8UnormSrgb).unwrap();
        let old = old_renderer
            .render_rows(
                &OpaqueSurfaceScene {
                    surface: surface.clone(),
                    navigation: scene.navigation.clone(),
                    volume: scene.volume.clone(),
                    material: scene.material.clone(),
                    lighting: scene.lighting,
                    surface_lighting: Lighting::HEADLIGHT,
                    surface_luminance: 0.8,
                },
                0,
                0..scene.size[1],
                || false,
            )
            .unwrap()
            .unwrap();
        let mut renderer = SectionRenderer::new(gpu, wgpu::TextureFormat::Rgba8UnormSrgb).unwrap();
        let image = renderer
            .render_rows(&scene, 0, 0..scene.size[1], || false)
            .unwrap()
            .unwrap();
        assert!(verify_scene(&image).0 > 20);
        assert_eq!(
            pixels(&d, &q, image.texture()),
            pixels(&d, &q, old.texture())
        );
        assert!(
            renderer
                .render_rows(&scene, 0, 0..11, || false)
                .unwrap()
                .is_none()
        );
        scene.surface.as_mut().unwrap().image = project(&projector, geometry, &scene);
        assert!(
            renderer
                .render_rows(&scene, 0, 11..scene.size[1], || false)
                .is_err()
        );
        renderer.cancel(0).unwrap();
        scene.surface.as_mut().unwrap().image = surface;
        scene.volume.camera.yaw_degrees += 1.;
        assert!(
            renderer
                .render_rows(&scene, 0, 0..scene.size[1], || false)
                .is_err()
        );
        scene.volume.camera.yaw_degrees -= 1.;
        scene.size[0] += 1;
        assert!(
            renderer
                .render_rows(&scene, 0, 0..scene.size[1], || false)
                .is_err()
        );
        scene.size[0] -= 1;
        scene.surface.as_mut().unwrap().luminance = f32::NAN;
        assert!(
            renderer
                .render_rows(&scene, 0, 0..scene.size[1], || false)
                .is_err()
        );
        drop(renderer);
        assert!(verify_scene(&image).0 > 20);
    }
    fence(&d);
    assert_eq!(
        before,
        [MemoryBudget::volumes().used(), MemoryBudget::gpu().used()]
    );
}
