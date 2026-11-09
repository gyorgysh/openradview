// SPDX-License-Identifier: GPL-3.0-or-later
//! Sparse-source comparisons with a retained surface front and local probes.
use super::*;
use orv_core::volume_ranges::VolumeRanges;
use orv_render::material_ranges::GpuMaterialRanges;
#[path = "../fixtures/optical.rs"]
mod optical;

fn compare(
    mut run: impl FnMut(usize) -> Result<()>,
    samples: usize,
    warmups: usize,
) -> Result<[Value; 2]> {
    for cycle in 0..warmups {
        for mode in 0..2 {
            run((mode + cycle) % 2)?;
        }
    }
    let mut times: [Vec<f64>; 2] = std::array::from_fn(|_| Vec::with_capacity(samples));
    for cycle in 0..samples {
        for order in 0..2 {
            let mode = (order + cycle) % 2;
            let start = Instant::now();
            run(mode)?;
            times[mode].push(start.elapsed().as_secs_f64() * 1000.);
        }
    }
    Ok(times.map(|mut ms|{ms.sort_by(f64::total_cmp);json!({"samples":samples,"warmups":warmups,
        "median_ms":ms[samples/2],"p95_ms":ms[(samples*95).div_ceil(100)-1],"max_ms":ms[samples-1],"order":"rotating interleaved dense/accelerated"})}))
}
struct Reference {
    radiance: f64,
    transmission: f64,
    hit: Option<[u32; 4]>,
}
fn optical_reference(
    scene: &OpaqueSurfaceScene,
    pixel: [u32; 2],
    kind: &str,
    plane: Option<usize>,
) -> Result<Reference> {
    let req = scene.surface.request();
    let source = req.surface.source().volume();
    let frame = req.camera.frame(&req.display, 1.)?;
    let (origin, direction) =
        frame.ray(pixel.map(|p| (f64::from(p) + 0.5) / f64::from(req.size[0])));
    let hit = if kind == "empty" {
        None
    } else {
        reference(req, pixel, plane)
    };
    let mut background = 0.0030352698;
    let mut ray = source.ray_interval(origin, direction).unwrap_or([1., 0.]);
    if let Some((word, depth, _)) = hit {
        let axis = ((word[3] >> 16) & 7) as usize / 2;
        let normal = std::array::from_fn(|a| {
            let mut u = [0.; 3];
            u[a] = 1.;
            source.world_vector_to_voxel(u)[axis]
        });
        let normal = orv_core::volume_material::gradient_normal(normal)
            .ok_or("Invalid independent surface normal")?;
        background = f64::from(scene.surface_luminance)
            * (f64::from(scene.surface_lighting.ambient)
                + f64::from(scene.surface_lighting.diffuse) * dot(normal, direction).abs());
        let offset = dot(
            std::array::from_fn(|a| origin[a] - source.center()[a]),
            direction,
        );
        ray[1] = ray[1].min(depth - offset);
    }
    let mut transmission = 1.;
    let mut radiance = 0.;
    if ray[1] > ray[0] {
        let speed = source
            .world_vector_to_voxel(direction)
            .into_iter()
            .map(f64::abs)
            .fold(0., f64::max);
        let count = ((ray[1] - ray[0]) * speed / scene.volume.quality.step_factor())
            .ceil()
            .clamp(1., f64::from(scene.volume.quality.max_steps())) as u32;
        let step = (ray[1] - ray[0]) / f64::from(count);
        for i in 0..count {
            let t = ray[0] + (f64::from(i) + 0.5) * step;
            let point = std::array::from_fn(|a| origin[a] + direction[a] * t);
            if let Some(value) = source.sample(point) {
                let sample = scene
                    .material
                    .sample(value)
                    .ok_or("Invalid reference material")?;
                let alpha = sample
                    .opacity(step)
                    .ok_or("Invalid reference optical depth")?;
                radiance += transmission * alpha * sample.luminance;
                transmission *= 1. - alpha;
                if transmission < 0.0001 {
                    break;
                }
            }
        }
    }
    Ok(Reference {
        radiance: radiance + transmission * background,
        transmission: if hit.is_some() { transmission } else { 0. },
        hit: hit.map(|h| h.0),
    })
}
fn verify_pair(
    dense: &OpaqueSurfaceImage,
    fast: &OpaqueSurfaceImage,
    kind: &str,
    plane: Option<usize>,
) -> Result<Value> {
    let req = fast.scene().surface.request();
    let a = read(req.surface.source(), dense.texture(), 8)?;
    let b = read(req.surface.source(), fast.texture(), 8)?;
    if a.bytes != b.bytes {
        return Err("Accelerated surface radiance changed a pixel".into());
    }
    for word in b.bytes.as_chunks::<8>().0 {
        let mut rgba = [0.; 4];
        for (a, channel) in rgba.iter_mut().enumerate() {
            *channel = half(u16::from_le_bytes(word[a * 2..a * 2 + 2].try_into()?))?;
        }
        if rgba[0] < 0. || rgba[0] > 1. || rgba[0] != rgba[1] || rgba[1] != rgba[2] || rgba[3] != 1.
        {
            return Err("Invalid composed color/alpha".into());
        }
    }
    let mut error = 0f64;
    for i in 0..256u32 {
        let side = req.size[0];
        let pixel = [(i * 73 + 17) % side, (i * 151 + 31) % side];
        let index = (pixel[0] + pixel[1] * side) as usize * 8;
        let actual = half(u16::from_le_bytes(b.bytes[index..index + 2].try_into()?))?;
        error = error
            .max((actual - optical_reference(fast.scene(), pixel, kind, plane)?.radiance).abs());
    }
    if error > 0.001 {
        return Err(format!("Independent source/surface optics error {error} for {kind}").into());
    }
    Ok(
        json!({"all_pixels_identical":true,"finite_grayscale_alpha_pixels":b.bytes.len()/8,"independent_f64_optical_rays":256,"max_linear_error":error}),
    )
}
fn verify_probe(
    probe: &orv_render::surface_pick::SurfaceProbe,
    expected: &Reference,
    current: &PickState,
    pixel: [u32; 2],
) -> Result<()> {
    let expected_kind = if expected.hit.is_none() {
        SurfaceProbeKind::Empty
    } else if expected.transmission < 0.05 {
        SurfaceProbeKind::Obscured
    } else {
        SurfaceProbeKind::Hit
    };
    if probe.kind() != expected_kind || !probe.is_current(current, pixel) {
        return Err("Accelerated probe changed visibility/current state".into());
    }
    if (f64::from(probe.foreground_transmittance()) - expected.transmission).abs() > 0.001 {
        return Err("Independent probe foreground attenuation differs".into());
    }
    if let Some(hit) = probe.hit() {
        let face = hit.face;
        let cell = face.cell();
        let word = [
            cell[0],
            cell[1],
            cell[2],
            u32::from(hit.key.number)
                | ((face.side() as u32) << 16)
                | (u32::from(face.boundary() == orv_core::surface::FaceBoundary::Grid) << 24),
        ];
        if Some(word) != expected.hit {
            return Err("Independent probe cell/face membership differs".into());
        }
    }
    Ok(())
}
pub(super) fn run() -> Result<()> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        flags: orv_render::instance_flags(),
        backends: orv_render::configured_backends()?,
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapter = pollster::block_on(instance.request_adapter(&Default::default()))?;
    let info = adapter.get_info();
    let (device, queue) =
        pollster::block_on(adapter.request_device(&orv_render::device_descriptor(&adapter)))?;
    let before_gpu = MemoryBudget::gpu().used();
    let before_cpu = MemoryBudget::volumes().used();
    let cpu = optical::source("sparse")?;
    let gpu = GpuVolumeSource::new(&device, &queue, cpu.clone(), || false)?;
    let material = Arc::new(VolumeMaterial::new(&[
        MaterialPoint {
            value: -1200.,
            luminance: 0.2,
            extinction_per_mm: 0.,
        },
        MaterialPoint {
            value: -800.,
            luminance: 0.2,
            extinction_per_mm: 0.,
        },
        MaterialPoint {
            value: -799.,
            luminance: 0.2,
            extinction_per_mm: 0.005,
        },
        MaterialPoint {
            value: 2000.,
            luminance: 0.2,
            extinction_per_mm: 0.01,
        },
    ])?);
    let started = Instant::now();
    let cpu_ranges = VolumeRanges::build(cpu.clone(), 32, || false)?;
    let ranges = GpuMaterialRanges::new(gpu.clone(), cpu_ranges, material.clone(), || false)?;
    let classification_ms = started.elapsed().as_secs_f64() * 1000.;
    let projector = GpuSurfaceProjector::new(gpu.clone())?;
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let mut dense = OpaqueSurfaceRenderer::new(gpu.clone(), format)?;
    let mut fast = OpaqueSurfaceRenderer::new_accelerated(ranges.clone(), format)?;
    let old_picker = GpuSurfacePicker::new(gpu.clone())?;
    let new_picker = GpuSurfacePicker::new_accelerated(ranges.clone())?;
    let mut cases = Vec::new();
    for (kind, plane) in [
        ("empty", None),
        ("filled", None),
        ("one_cell_plane", Some(64)),
    ] {
        let mut map = Labels::empty(cpu.clone())?;
        if kind != "empty" {
            map.threshold(plane, [-1e20, 1e20], 1, || false)?;
        }
        let mesh = CellSurface::build(
            MaskOwner::Manual(Arc::new(map)),
            1,
            None,
            SurfaceLimits::default(),
            || false,
        )?
        .0;
        let (surface, _) = GpuCellSurface::upload(gpu.clone(), mesh, None, || false)?;
        for side in [512, 2048] {
            let mut camera = Camera::fit(&cpu);
            camera.yaw_degrees = 31.;
            camera.pitch_degrees = -23.;
            camera.roll_degrees = 7.;
            let req = SurfaceProjection {
                surface: surface.clone(),
                camera,
                display: ViewState {
                    pan: [0.013, -0.017],
                    zoom: 1.05,
                    ..Default::default()
                },
                clip: None,
                size: [side; 2],
            };
            let projected = projector.render(req.clone(), || false)?;
            let mut volume = VolumeView::fit(&cpu);
            volume.camera = req.camera.clone();
            volume.projection = Projection::Composite;
            let mut scene = OpaqueSurfaceScene {
                surface: projected,
                navigation: req.display.clone(),
                volume,
                material: material.clone(),
                lighting: Lighting::UNLIT,
                surface_lighting: Lighting::HEADLIGHT,
                surface_luminance: 0.85,
            };
            let a = compose(&mut dense, &scene, side)?;
            let b = compose(&mut fast, &scene, side)?;
            let verified = verify_pair(&a, &b, kind, plane)?;
            if !Arc::ptr_eq(
                b.material_ranges().ok_or("No retained classification")?,
                &ranges,
            ) {
                return Err("Completed image lost its classification owner".into());
            }
            let scope = PickScope::new()?;
            let old_frame = PickFrame::new(a, scope.clone());
            let new_frame = PickFrame::new(b, scope);
            let old_current = PickState::from_frame(&old_frame);
            let new_current = PickState::from_frame(&new_frame);
            let pixel = [side / 2; 2];
            let expected = optical_reference(&scene, pixel, kind, plane)?;
            let a = old_picker.probe(&old_frame, &old_current, pixel, || false)?;
            let b = new_picker.probe(&new_frame, &new_current, pixel, || false)?;
            verify_probe(&a, &expected, &old_current, pixel)?;
            verify_probe(&b, &expected, &new_current, pixel)?;
            if a.foreground_transmittance().to_bits() != b.foreground_transmittance().to_bits() {
                return Err("Probe transmission changed".into());
            }
            drop(a);
            drop(b);
            fence(&device)?;
            let baseline_cpu = MemoryBudget::volumes().used();
            let baseline_gpu = MemoryBudget::gpu().used();
            let timing = compare(
                |mode| {
                    let image =
                        compose(if mode == 0 { &mut dense } else { &mut fast }, &scene, side)?;
                    drop(image);
                    fence(&device)
                },
                if side == 512 { 12 } else { 4 },
                if side == 512 { 3 } else { 1 },
            )?;
            let rows = compare(
                |mode| {
                    let image =
                        compose(if mode == 0 { &mut dense } else { &mut fast }, &scene, 16)?;
                    drop(image);
                    fence(&device)
                },
                if side == 512 { 4 } else { 2 },
                1,
            )?;
            let row_image = compose(&mut fast, &scene, 16)?;
            let verified_rows = verify_pair(old_frame.image(), &row_image, kind, plane)?;
            drop(row_image);
            fence(&device)?;
            let probes = compare(
                |mode| {
                    let result = if mode == 0 {
                        old_picker.probe(&old_frame, &old_current, pixel, || false)?
                    } else {
                        new_picker.probe(&new_frame, &new_current, pixel, || false)?
                    };
                    drop(result);
                    fence(&device)
                },
                if side == 512 { 60 } else { 20 },
                3,
            )?;
            let mut front = Some(new_frame.image().clone());
            let combined = measure(
                || {
                    scene.surface = projector.render(req.clone(), || false)?;
                    let output = compose(&mut fast, &scene, side)?;
                    front = Some(output);
                    fence(&device)
                },
                if side == 512 { 8 } else { 4 },
            )?;
            let verified_combined = verify_pair(
                old_frame.image(),
                front.as_ref().ok_or("No retained front")?,
                kind,
                plane,
            )?;
            drop(front);
            fence(&device)?;
            // The combined replacement retains only the new projected surface;
            // restore the cached reference before comparing active ownership.
            scene.surface = old_frame.image().scene().surface.clone();
            fence(&device)?;
            if MemoryBudget::volumes().used() != baseline_cpu
                || MemoryBudget::gpu().used() != baseline_gpu
            {
                return Err("Cached surface/probe reservations were not restored".into());
            }
            cases.push(json!({"kind":kind,"size":[side,side],"faces":surface.surface().face_count(),"state":scene.volume,
                "cached_composition_dense":timing[0],"cached_composition_accelerated":timing[1],"fenced_16_row_sequence_dense":rows[0],"fenced_16_row_sequence_accelerated":rows[1],
                "local_probe_dense":probes[0],"local_probe_accelerated":probes[1],"complete_projection_plus_accelerated_composition":combined,
                "verification":verified,"verification_rows":verified_rows,"verification_combined":verified_combined,
                "probe_pixel":pixel,"independent_probe_transmission":expected.transmission,"independent_probe_membership_match":true}));
            eprintln!("Verified accelerated {kind} {side}");
        }
    }
    let transparent_tiles = ranges.transparent_tiles();
    drop(projector);
    drop(dense);
    drop(fast);
    drop(old_picker);
    drop(new_picker);
    drop(ranges);
    drop(gpu);
    drop(cpu);
    fence(&device)?;
    if MemoryBudget::gpu().used() != before_gpu || MemoryBudget::volumes().used() != before_cpu {
        return Err("Sparse benchmark reservations were not restored".into());
    }
    println!(
        "{}",
        serde_json::to_string_pretty(
            &json!({"schema":4,"adapter":info.name,"backend":format!("{:?}",info.backend),
        "source_dimensions":[512,512,128],"unique_scalar_bytes":134217728,"range_side":32,"transparent_tiles":transparent_tiles,"classification_prepare_ms":classification_ms,
        "source_lighting":"unlit","surface_lighting":"two-sided headlight","source_luminance":0.2,"material":material.points().iter().map(|p|json!([p.value,p.luminance,p.extinction_per_mm])).collect::<Vec<_>>(),
        "scope":"Complete checked immutable source/surface images, fresh allocation, uniforms, status readback, drop and retirement fences; cached projection for paired comparisons. Original midpoint phase/resolution retained. Fully valid generated sparse source; source/geometry/classification/pipeline preparation, verification readback, native input/display and RSS excluded. Complete projection+composition retains old front separately. Probe uses original centre pixel, shared classification, 56B readback and f64 membership/visibility checks.",
        "cases":cases,"reservations_restored":true})
        )?
    );
    Ok(())
}
