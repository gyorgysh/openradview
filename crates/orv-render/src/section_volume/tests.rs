// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use crate::{
    brick_source::tests::{device, source},
    reslice::GpuReslicer,
};
use orv_core::{
    Plane, VoiFunction,
    volume::Volume,
    volume_material::MaterialPoint,
    volume_view::{PhysicalClip, Quality},
};
use std::{cell::Cell, time::Duration};

fn fence(d: &wgpu::Device) {
    d.poll(wgpu::PollType::Wait {
        submission_index: None,
        timeout: Some(Duration::from_secs(30)),
    })
    .unwrap();
}
fn pixels(d: &wgpu::Device, q: &wgpu::Queue, t: &wgpu::Texture) -> Vec<[u16; 4]> {
    let size = t.size();
    let stride = (size.width * 8).next_multiple_of(256);
    let bytes = u64::from(stride) * u64::from(size.height);
    let _gpu = allocation::reserve_bytes(bytes, d, q).unwrap();
    let _cpu = MemoryBudget::volumes().reserve(bytes * 2).unwrap();
    let b = allocation::buffer(
        d,
        &wgpu::BufferDescriptor {
            label: Some("generated section verification"),
            size: bytes,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        },
    )
    .unwrap();
    let mut e = d.create_command_encoder(&Default::default());
    e.copy_texture_to_buffer(
        t.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &b,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(stride),
                rows_per_image: Some(size.height),
            },
        },
        size,
    );
    q.submit([e.finish()]);
    let (tx, rx) = mpsc::sync_channel(1);
    b.map_async(wgpu::MapMode::Read, .., move |r| {
        let _ = tx.send(r);
    });
    fence(d);
    rx.recv_timeout(Duration::from_secs(10)).unwrap().unwrap();
    let data = b.get_mapped_range(..).unwrap();
    let result = data
        .chunks_exact(stride as usize)
        .flat_map(|r| {
            r[..size.width as usize * 8]
                .as_chunks::<8>()
                .0
                .iter()
                .map(|p| std::array::from_fn(|a| u16::from_le_bytes([p[a * 2], p[a * 2 + 1]])))
        })
        .collect();
    drop(data);
    b.unmap();
    result
}
fn half(bits: u16) -> f64 {
    assert_eq!(bits & 32768, 0);
    let e = (bits >> 10) & 31;
    assert_ne!(e, 31);
    let f = f64::from(bits & 1023);
    if e == 0 {
        f * 2f64.powi(-24)
    } else {
        (1. + f / 1024.) * 2f64.powi(i32::from(e) - 15)
    }
}
fn material(extinction: f32) -> Arc<VolumeMaterial> {
    Arc::new(
        VolumeMaterial::new(&[-1000., 1000.].map(|value| MaterialPoint {
            value,
            luminance: 0.3,
            extinction_per_mm: extinction,
        }))
        .unwrap(),
    )
}
fn scene(gpu: Arc<GpuVolumeSource>) -> SectionScene {
    let v = gpu.volume();
    let grids = [Plane::Axial, Plane::Coronal, Plane::Sagittal]
        .map(|p| v.reformat_grid(p, v.center(), 32).unwrap());
    let mut reslice = GpuReslicer::new(gpu.clone()).unwrap();
    let images = reslice
        .render_frames(grids.each_ref().map(Some), || false)
        .unwrap();
    SectionScene {
        planes: images.map(|image| {
            image.map(|image| SectionPlane {
                image,
                window: WindowLevel {
                    width: 80.,
                    center: 20.,
                    function: VoiFunction::LinearExact,
                },
                invert: false,
            })
        }),
        navigation: ViewState {
            pan: [0.017, -0.023],
            zoom: 1.13,
            ..Default::default()
        },
        volume: VolumeView {
            projection: orv_core::volume_view::Projection::Composite,
            ..VolumeView::fit(v)
        },
        material: material(0.07),
        lighting: Lighting::UNLIT,
        size: [37, 29],
    }
}
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a.into_iter().zip(b).map(|(a, b)| a * b).sum()
}
fn sample(
    plane: &SectionPlane,
    origin: [f64; 3],
    direction: [f64; 3],
    clip: Option<&PhysicalClip>,
) -> Option<(f64, f64)> {
    let g = plane.image.grid().geometry();
    let n = g.normal();
    let denominator = dot(direction, n);
    if denominator == 0. {
        return None;
    }
    let depth = dot(std::array::from_fn(|a| g.position[a] - origin[a]), n) / denominator;
    let world = std::array::from_fn(|a| origin[a] + direction[a] * depth);
    if clip
        .is_some_and(|c| (0..3).any(|a| world[a] < c.minimum_mm[a] || world[a] > c.maximum_mm[a]))
    {
        return None;
    }
    let xy = g.world_to_pixel(world).unwrap();
    let f = plane.image.frame();
    if xy[0] < -0.5
        || xy[1] < -0.5
        || xy[0] >= f64::from(f.width) - 0.5
        || xy[1] >= f64::from(f.height) - 0.5
    {
        return None;
    }
    let xy = xy.map(|v| {
        if (v - v.round()).abs() < 0.00001 {
            v.round()
        } else {
            v
        }
    });
    let base = xy.map(f64::floor);
    let weight = [xy[0] - base[0], xy[1] - base[1]];
    let mut value = 0.;
    for y in 0..2 {
        for x in 0..2 {
            let w = if x == 0 { 1. - weight[0] } else { weight[0] }
                * if y == 0 { 1. - weight[1] } else { weight[1] };
            if w <= 0. {
                continue;
            }
            let x = (base[0] + x as f64).clamp(0., f64::from(f.width - 1)) as usize;
            let y = (base[1] + y as f64).clamp(0., f64::from(f.height - 1)) as usize;
            let i = y * f.width as usize + x;
            if !f.has_sample(i) {
                return None;
            }
            value += f64::from(f.pixels[i]) * w;
        }
    }
    let mut gray = f64::from(f.normalize(plane.window, value as f32).unwrap());
    if plane.invert ^ f.monochrome1 {
        gray = 1. - gray;
    }
    let linear = if gray <= 0.04045 {
        gray / 12.92
    } else {
        ((gray + 0.055) / 1.055).powf(2.4)
    };
    Some((depth, linear))
}
fn valid_length(v: &Volume, origin: [f64; 3], direction: [f64; 3], interval: [f64; 2]) -> f64 {
    if interval[1] <= interval[0] {
        return 0.;
    }
    let index = v.world_to_voxel(origin);
    let delta = v.world_vector_to_voxel(direction);
    let mut times = vec![interval[0], interval[1]];
    for a in 0..3 {
        if delta[a] != 0. {
            for n in 0..v.dimensions()[a] {
                let t = (n as f64 - index[a]) / delta[a];
                if t > interval[0] && t < interval[1] {
                    times.push(t);
                }
            }
        }
    }
    times.sort_by(f64::total_cmp);
    times.dedup();
    times
        .windows(2)
        .filter_map(|t| {
            let midpoint = std::array::from_fn(|a| origin[a] + direction[a] * (t[0] + t[1]) * 0.5);
            v.sample(midpoint).map(|_| t[1] - t[0])
        })
        .sum()
}
fn verify(image: &SectionImage, actual: &[[u16; 4]]) -> (usize, usize) {
    let scene = image.scene();
    let volume = image.source().volume();
    let camera = scene
        .volume
        .camera
        .frame(
            &scene.navigation,
            f64::from(scene.size[0]) / f64::from(scene.size[1]),
        )
        .unwrap();
    let mut hits = 0;
    let mut gaps = 0;
    for y in 0..scene.size[1] {
        for x in 0..scene.size[0] {
            let uv = [
                (f64::from(x) + 0.5) / f64::from(scene.size[0]),
                (f64::from(y) + 0.5) / f64::from(scene.size[1]),
            ];
            let (origin, direction) = camera.ray(uv);
            let mut nearest = None;
            for plane in scene.planes.iter().flatten() {
                if let Some(hit) = sample(plane, origin, direction, scene.volume.clip.as_ref())
                    && nearest.is_none_or(|(depth, _)| hit.0 < depth)
                {
                    nearest = Some(hit);
                }
            }
            let background = nearest.map_or(0.0030352698, |(_, l)| l);
            if nearest.is_some() {
                hits += 1;
            }
            let length = volume
                .clipped_ray_interval(origin, direction, scene.volume.clip.as_ref())
                .map_or(0., |mut interval| {
                    if let Some((depth, _)) = nearest {
                        interval[1] = interval[1].min(depth);
                    }
                    let valid = valid_length(volume, origin, direction, interval);
                    if valid + 1e-6 < (interval[1] - interval[0]).max(0.) {
                        gaps += 1;
                    }
                    valid
                });
            let transmission =
                (-f64::from(scene.material.points()[0].extinction_per_mm) * length).exp();
            let expected = f64::from(scene.material.points()[0].luminance) * (1. - transmission)
                + transmission * background;
            let pixel = actual[(y * scene.size[0] + x) as usize];
            assert_eq!(pixel[0], pixel[1]);
            assert_eq!(pixel[0], pixel[2]);
            assert_eq!(pixel[3], 0x3c00);
            assert!(
                (half(pixel[0]) - expected).abs() < 0.001,
                "{x},{y}: {} versus {expected}, hit {nearest:?}, length {length}",
                half(pixel[0])
            );
        }
    }
    (hits, gaps)
}
#[test]
fn ordered_completed_planes_match_physical_rays_with_missing_data_clips_and_tiled_completion() {
    let (d, q) = device(None);
    for (missing, reverse) in [(false, false), (false, true), (true, true)] {
        let cpu = source([17, 15, 9], 1, false, reverse);
        let cpu = if missing {
            Arc::new(
                Volume::new(
                    cpu.frames()
                        .iter()
                        .enumerate()
                        .map(|(z, frame)| {
                            let mut frame = (**frame).clone();
                            frame.validity = Some(
                                (0..frame.pixels.len())
                                    .map(|i| u8::from(z != 3 && i % 17 != 8))
                                    .collect::<Vec<_>>()
                                    .into(),
                            );
                            Arc::new(frame)
                        })
                        .collect(),
                )
                .unwrap(),
            )
        } else {
            cpu
        };
        let gpu = GpuVolumeSource::new(&d, &q, cpu.clone(), || false).unwrap();
        let mut renderer =
            SectionRenderer::new(gpu.clone(), wgpu::TextureFormat::Rgba8UnormSrgb).unwrap();
        let mut scene = scene(gpu.clone());
        if reverse {
            let plane =
                orv_core::oblique::ObliquePlane::from_angles(cpu.center(), [17., -23., 11.])
                    .unwrap();
            let grid = cpu.oblique_grid(&plane, 32, None).unwrap();
            let mut reslice = GpuReslicer::new(gpu).unwrap();
            scene.planes[2].as_mut().unwrap().image = reslice
                .render_frames([Some(&grid), None, None], || false)
                .unwrap()[0]
                .take()
                .unwrap();
        }
        scene.volume.quality = Quality::Refined;
        let center = cpu.center();
        for (yaw, pitch, roll, clip) in [
            (23., -17., 31., false),
            (90., -90., 180., false),
            (32., -21., -13., true),
        ] {
            scene.volume.camera.yaw_degrees = yaw;
            scene.volume.camera.pitch_degrees = pitch;
            scene.volume.camera.roll_degrees = roll;
            scene.planes[1].as_mut().unwrap().invert = true;
            scene.planes[2].as_mut().unwrap().window.function = VoiFunction::Sigmoid;
            scene.volume.clip = clip.then_some(PhysicalClip {
                minimum_mm: [center[0] - 100., center[1] - 100., center[2] - 2.3],
                maximum_mm: [center[0] + 3.1, center[1] + 100., center[2] + 100.],
            });
            let full = renderer
                .render_rows(&scene, 0, 0..scene.size[1], || false)
                .unwrap()
                .unwrap();
            let expected = pixels(&d, &q, full.texture());
            let (hits, gaps) = verify(&full, &expected);
            assert!(
                hits > 10,
                "Plane verification must include actual plane pixels"
            );
            if missing {
                assert!(gaps > 10, "Missing source intervals must be exercised");
            }
            assert!(
                renderer
                    .render_rows(&scene, 1, 0..13, || false)
                    .unwrap()
                    .is_none()
            );
            let tiled = renderer
                .render_rows(&scene, 1, 13..scene.size[1], || false)
                .unwrap()
                .unwrap();
            assert_eq!(expected, pixels(&d, &q, tiled.texture()));
        }
    }
}
#[test]
fn section_completion_is_immutable_and_refuses_changed_owners_rows_slabs_or_cancelled_work() {
    let (d, q) = device(None);
    let before = [MemoryBudget::gpu().used(), MemoryBudget::volumes().used()];
    {
        let cpu = source([17, 15, 9], 1, false, false);
        let gpu = GpuVolumeSource::new(&d, &q, cpu.clone(), || false).unwrap();
        let mut renderer =
            SectionRenderer::new(gpu.clone(), wgpu::TextureFormat::Rgba8UnormSrgb).unwrap();
        let mut scene = scene(gpu.clone());
        let old = renderer
            .render_rows(&scene, 0, 0..scene.size[1], || false)
            .unwrap()
            .unwrap();
        let retained = pixels(&d, &q, old.texture());
        let pitch = scene.volume.camera.pitch_degrees;
        scene.volume.camera.pitch_degrees = 1e-40;
        assert!(
            renderer
                .render_rows(&scene, 0, 0..scene.size[1], || false)
                .is_err()
        );
        scene.volume.camera.pitch_degrees = pitch;
        fence(&d);
        let ram = MemoryBudget::volumes();
        let vram = MemoryBudget::gpu();
        let limit = ram.limit();
        ram.set_limit(ram.used());
        let denied = renderer.render_rows(&scene, 0, 0..scene.size[1], || false);
        ram.set_limit(limit);
        assert!(denied.is_err());
        assert!(renderer.pending[0].is_none());
        fence(&d);
        let limit = vram.limit();
        vram.set_limit(vram.used());
        let denied = renderer.render_rows(&scene, 0, 0..scene.size[1], || false);
        vram.set_limit(limit);
        assert!(denied.is_err());
        assert!(renderer.pending[0].is_none());
        assert!(
            renderer
                .render_rows(&scene, 0, 0..7, || false)
                .unwrap()
                .is_none()
        );
        scene.planes[0].as_mut().unwrap().invert = true;
        assert!(
            renderer
                .render_rows(&scene, 0, 7..scene.size[1], || false)
                .is_err()
        );
        renderer.cancel(0).unwrap();
        assert!(
            renderer
                .render_rows(&scene, 0, 7..scene.size[1], || false)
                .is_err()
        );
        let another = GpuVolumeSource::new(&d, &q, cpu.clone(), || false).unwrap();
        assert!(
            SectionRenderer::new(another, wgpu::TextureFormat::Rgba8UnormSrgb)
                .unwrap()
                .render_rows(&scene, 0, 0..scene.size[1], || false)
                .is_err()
        );
        let plane = orv_core::oblique::ObliquePlane::from_angles(cpu.center(), [0.; 3]).unwrap();
        let grid = cpu
            .oblique_grid(
                &plane,
                32,
                Some(orv_core::slab::Slab {
                    thickness_mm: 3.,
                    projection: orv_core::slab::SlabProjection::Mean,
                    intervals: 2,
                }),
            )
            .unwrap();
        let mut reslice = GpuReslicer::new(gpu).unwrap();
        let slab = reslice
            .render_frames([Some(&grid), None, None], || false)
            .unwrap()[0]
            .take()
            .unwrap();
        let saved = scene.planes[0].as_ref().unwrap().image.clone();
        scene.planes[0].as_mut().unwrap().image = slab;
        assert!(
            renderer
                .render_rows(&scene, 0, 0..scene.size[1], || false)
                .is_err()
        );
        scene.planes[0].as_mut().unwrap().image = saved;
        assert!(
            renderer
                .render_rows(&scene, 0, 0..7, || false)
                .unwrap()
                .is_none()
        );
        assert!(
            renderer
                .render_rows(&scene, 0, 7..scene.size[1], || true)
                .is_err()
        );
        assert!(renderer.pending[0].is_none());
        let calls = Cell::new(0);
        assert!(
            renderer
                .render_rows(&scene, 0, 0..scene.size[1], || {
                    calls.set(calls.get() + 1);
                    calls.get() > 2
                })
                .is_err()
        );
        assert!(renderer.pending[0].is_none());
        let new = renderer
            .render_rows(&scene, 0, 0..scene.size[1], || false)
            .unwrap()
            .unwrap();
        verify(&new, &pixels(&d, &q, new.texture()));
        assert!(!old.scene().planes[0].as_ref().unwrap().invert);
        assert!(new.scene().planes[0].as_ref().unwrap().invert);
        drop(renderer);
        assert_eq!(retained, pixels(&d, &q, old.texture()));
    }
    fence(&d);
    assert_eq!(
        before,
        [MemoryBudget::gpu().used(), MemoryBudget::volumes().used()]
    );
}

#[test]
fn nearest_valid_plane_precedes_slot_order_and_missing_stencils_expose_farther_planes() {
    let (d, q) = device(None);
    let original = source([17, 15, 9], 1, false, false);
    let cpu = Arc::new(
        Volume::new(
            original
                .frames()
                .iter()
                .enumerate()
                .map(|(z, f)| {
                    let mut f = (**f).clone();
                    f.validity = Some(
                        (0..f.pixels.len())
                            .map(|i| u8::from(z != 6 || i % 17 != 8))
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
    let mut renderer =
        SectionRenderer::new(gpu.clone(), wgpu::TextureFormat::Rgba8UnormSrgb).unwrap();
    let mut scene = scene(gpu.clone());
    let mut reslice = GpuReslicer::new(gpu).unwrap();
    let grids = [2., 6.].map(|z| {
        cpu.reformat_grid(Plane::Axial, cpu.voxel_to_world([8., 7., z]), 32)
            .unwrap()
    });
    let mut images = reslice
        .render_frames([Some(&grids[0]), Some(&grids[1]), None], || false)
        .unwrap();
    let far = SectionPlane {
        image: images[0].take().unwrap(),
        window: WindowLevel {
            width: 80.,
            center: 20.,
            function: VoiFunction::LinearExact,
        },
        invert: false,
    };
    let near = SectionPlane {
        image: images[1].take().unwrap(),
        invert: true,
        ..far.clone()
    };
    scene.planes = [Some(far.clone()), Some(near.clone()), None];
    scene.material = material(0.);
    scene.volume.camera.yaw_degrees = 0.;
    scene.volume.camera.pitch_degrees = 90.;
    scene.volume.camera.roll_degrees = 0.;
    scene.size = [63, 47];
    let image = renderer
        .render_rows(&scene, 0, 0..scene.size[1], || false)
        .unwrap()
        .unwrap();
    verify(&image, &pixels(&d, &q, image.texture()));
    let camera = scene
        .volume
        .camera
        .frame(&scene.navigation, 63. / 47.)
        .unwrap();
    let mut exposed = 0;
    let mut front = 0;
    for y in 0..47 {
        for x in 0..63 {
            let (o, d) = camera.ray([(f64::from(x) + 0.5) / 63., (f64::from(y) + 0.5) / 47.]);
            match (sample(&far, o, d, None), sample(&near, o, d, None)) {
                (Some(_), None) => exposed += 1,
                (_, Some(_)) => front += 1,
                _ => (),
            }
        }
    }
    assert!(
        exposed > 5 && front > 20,
        "Exercise both foreground and transparent plane pixels: {exposed}/{front}"
    );
    scene.planes = [
        Some(far.clone()),
        Some(SectionPlane {
            invert: true,
            ..far.clone()
        }),
        None,
    ];
    let first = renderer
        .render_rows(&scene, 0, 0..scene.size[1], || false)
        .unwrap()
        .unwrap();
    let a = pixels(&d, &q, first.texture());
    verify(&first, &a);
    scene.planes.swap(0, 1);
    let second = renderer
        .render_rows(&scene, 0, 0..scene.size[1], || false)
        .unwrap()
        .unwrap();
    let b = pixels(&d, &q, second.texture());
    verify(&second, &b);
    assert!(
        a.iter().zip(&b).filter(|(a, b)| a != b).count() > 20,
        "Coincident planes must keep stable slot precedence"
    );
    scene.planes = [None, None, None];
    let empty = renderer
        .render_rows(&scene, 0, 0..scene.size[1], || false)
        .unwrap()
        .unwrap();
    assert_eq!(verify(&empty, &pixels(&d, &q, empty.texture())).0, 0);
}

#[test]
fn source_sections_preserve_lookup_tables_and_monochrome_polarity() {
    use orv_core::scalar::{ScalarData, ScalarInfo, Storage, Unit, ValueType, VoiChoices, VoiLut};
    let (d, q) = device(None);
    let original = source([17, 15, 9], 1, false, false);
    let scalar = ScalarData::new(
        ScalarInfo {
            modality_lut: None,
            real_world: None,
            value_type: ValueType::F32,
            storage: Storage::SignedInteger,
            stored_bits: 16,
            unit: Unit::Unknown,
            rescale: None,
            resampled: false,
        },
        VoiChoices::new(
            Vec::new(),
            vec![
                VoiLut::new(
                    0,
                    16,
                    Arc::from([0, 65535, 8192, 32768, 16384, 49152, 1024, 60000]),
                )
                .unwrap(),
            ],
        )
        .unwrap(),
    )
    .unwrap();
    for monochrome1 in [false, true] {
        let cpu = Arc::new(
            Volume::new(
                original
                    .frames()
                    .iter()
                    .map(|f| {
                        let mut f = (**f).clone();
                        f.pixels = vec![2.25; f.pixels.len()].into();
                        f.scalar = Some(scalar.clone());
                        f.monochrome1 = monochrome1;
                        f.default_window = WindowLevel::lookup(0);
                        Arc::new(f)
                    })
                    .collect(),
            )
            .unwrap(),
        );
        let gpu = GpuVolumeSource::new(&d, &q, cpu, || false).unwrap();
        let mut renderer =
            SectionRenderer::new(gpu.clone(), wgpu::TextureFormat::Rgba8UnormSrgb).unwrap();
        let mut scene = scene(gpu);
        scene.planes[1] = None;
        scene.planes[2] = None;
        scene.material = material(0.);
        scene.planes[0].as_mut().unwrap().window = WindowLevel::lookup(0);
        scene.volume.camera.pitch_degrees = 90.;
        let mut outputs = Vec::new();
        for invert in [false, true] {
            scene.planes[0].as_mut().unwrap().invert = invert;
            let image = renderer
                .render_rows(&scene, 0, 0..scene.size[1], || false)
                .unwrap()
                .unwrap();
            let data = pixels(&d, &q, image.texture());
            assert!(verify(&image, &data).0 > 20);
            outputs.push(data);
        }
        assert!(
            outputs[0]
                .iter()
                .zip(&outputs[1])
                .filter(|(a, b)| a != b)
                .count()
                > 20
        );
    }
}
