// SPDX-License-Identifier: GPL-3.0-or-later
//! Generated complete known-interface projection timings, without source optics.
use orv_core::{
    ViewState,
    labels::Labels,
    mask::MaskOwner,
    resources::{MemoryBudget, Reservation},
    study::{Geometry, demo_study},
    surface::{CellSurface, SurfaceLimits},
    volume::Volume,
    volume_material::{MaterialPoint, VolumeMaterial},
    volume_view::{Camera, Projection, VolumeView},
};
use orv_render::{
    GpuReservation,
    material_volume::Lighting,
    opaque_surface::{OpaqueSurfaceImage, OpaqueSurfaceRenderer, OpaqueSurfaceScene},
    surface_pick::{GpuSurfacePicker, PickFrame, PickScope, PickState, SurfaceProbeKind},
    surface_projection::{GpuSurfaceProjector, SurfaceImage, SurfaceProjection},
    surface_source::GpuCellSurface,
    volume_source::GpuVolumeSource,
};
use serde_json::{Value, json};
use std::{
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
fn source() -> Result<Arc<Volume>> {
    let template = demo_study().series.remove(0).frames.remove(0);
    let plane: Arc<[f32]> = vec![0.; 512 * 512].into();
    Ok(Arc::new(Volume::new(
        (0..128)
            .map(|z| {
                let mut f = (*template).clone();
                f.width = 512;
                f.height = 512;
                f.pixels = plane.clone();
                f.geometry = Some(Geometry {
                    position: [f64::from(z) * 0.05, 0., f64::from(z) * 1.2],
                    orientation: [1., 0., 0., 0., 1., 0.],
                    spacing: [0.7, 0.7],
                    frame_of_reference: Some("generated cell projection benchmark".into()),
                    nifti_transform: None,
                });
                Arc::new(f)
            })
            .collect(),
    )?))
}
fn fence(device: &wgpu::Device) -> Result<()> {
    device.poll(wgpu::PollType::Wait {
        submission_index: None,
        timeout: Some(Duration::from_secs(30)),
    })?;
    Ok(())
}
fn measure(mut work: impl FnMut() -> Result<()>, samples: usize) -> Result<Value> {
    for _ in 0..4 {
        work()?;
    }
    let mut times = Vec::new();
    for _ in 0..samples {
        let start = Instant::now();
        work()?;
        times.push(start.elapsed().as_secs_f64() * 1000.);
    }
    times.sort_by(f64::total_cmp);
    Ok(
        json!({"samples":samples,"warmups":4,"median_ms":times[samples/2],
        "p95_ms":times[(samples*95).div_ceil(100)-1],"max_ms":times[samples-1]}),
    )
}
struct Readback {
    bytes: Vec<u8>,
    _cpu: Reservation,
}
fn read(gpu: &GpuVolumeSource, texture: &wgpu::Texture, bpp: u32) -> Result<Readback> {
    let size = texture.size();
    let stride = (size.width * bpp).div_ceil(256) * 256;
    let size_bytes = u64::from(stride) * u64::from(size.height);
    let _gpu = GpuReservation::new(size_bytes + 65536, gpu.device(), gpu.queue())?;
    let cpu = MemoryBudget::volumes()
        .reserve(size_bytes * 2)
        .ok_or("Readback exceeds CPU staging admission")?;
    let buffer = gpu.device().create_buffer(&wgpu::BufferDescriptor {
        label: Some("generated projection verification"),
        size: size_bytes,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = gpu.device().create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(stride),
                rows_per_image: Some(size.height),
            },
        },
        size,
    );
    gpu.queue().submit([encoder.finish()]);
    let (tx, rx) = mpsc::sync_channel(1);
    buffer.map_async(wgpu::MapMode::Read, .., move |r| {
        let _ = tx.send(r);
    });
    fence(gpu.device())?;
    rx.recv_timeout(Duration::from_secs(10))??;
    let mapped = buffer.get_mapped_range(..)?;
    let mut bytes = Vec::new();
    bytes.try_reserve_exact((size.width * size.height * bpp) as usize)?;
    for row in mapped.chunks_exact(stride as usize) {
        bytes.extend_from_slice(&row[..(size.width * bpp) as usize]);
    }
    drop(mapped);
    buffer.unmap();
    Ok(Readback { bytes, _cpu: cpu })
}
fn word(bytes: &[u8; 16]) -> [u32; 4] {
    std::array::from_fn(|a| u32::from_le_bytes(std::array::from_fn(|b| bytes[a * 4 + b])))
}
fn half(bits: u16) -> Result<f64> {
    let exponent = (bits >> 10) & 31;
    if exponent == 31 {
        return Err("Non-finite surface normal".into());
    }
    let sign = if bits & 32768 == 0 { 1. } else { -1. };
    let fraction = f64::from(bits & 1023);
    Ok(sign
        * if exponent == 0 {
            fraction * 2f64.powi(-24)
        } else {
            (1. + fraction / 1024.) * 2f64.powi(i32::from(exponent) - 15)
        })
}
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a.into_iter().zip(b).map(|(a, b)| a * b).sum()
}
fn reference(
    req: &SurfaceProjection,
    xy: [u32; 2],
    plane: Option<usize>,
) -> Option<([u32; 4], f64, f64)> {
    let source = req.surface.source().volume();
    let frame = req.camera.frame(&req.display, 1.).ok()?;
    let (world, direction) = frame.ray(xy.map(|v| (f64::from(v) + 0.5) / f64::from(req.size[0])));
    let o = source.world_to_voxel(world);
    let d = source.world_vector_to_voxel(direction);
    let mut low = [-0.5; 3];
    let mut high = [511.5, 511.5, 127.5];
    if let Some(z) = plane {
        low[2] = z as f64 - 0.5;
        high[2] = z as f64 + 0.5;
    }
    let mut entry = f64::NEG_INFINITY;
    let mut exit = f64::INFINITY;
    let mut side = 0;
    for a in 0..3 {
        if d[a] == 0. {
            if o[a] < low[a] || o[a] > high[a] {
                return None;
            }
            continue;
        }
        let t0 = (low[a] - o[a]) / d[a];
        let t1 = (high[a] - o[a]) / d[a];
        if t0.min(t1) > entry {
            entry = t0.min(t1);
            side = a * 2 + usize::from(d[a] < 0.);
        }
        exit = exit.min(t0.max(t1));
    }
    if entry >= exit {
        return None;
    }
    let p: [f64; 3] = std::array::from_fn(|a| o[a] + d[a] * entry);
    let mut cell = p.map(|v| (v + 0.5).floor().max(0.) as u32);
    let axis = side / 2;
    cell[axis] = if side % 2 == 0 {
        (low[axis] + 0.5) as u32
    } else {
        (high[axis] - 0.5) as u32
    };
    let grid = axis != 2 || plane.is_none();
    let word = [
        cell[0],
        cell[1],
        cell[2],
        1 | ((side as u32) << 16) | (u32::from(grid) << 24),
    ];
    let position = source.voxel_to_world(p);
    let depth = dot(
        std::array::from_fn(|a| position[a] - source.center()[a]),
        direction,
    );
    let edge = (0..3)
        .filter(|a| *a != axis)
        .map(|a| (0.5 - ((p[a] + 0.5).fract() - 0.5).abs()).abs())
        .fold(f64::INFINITY, f64::min);
    Some((word, depth, edge))
}
fn verify(image: &SurfaceImage, kind: &str, plane: Option<usize>) -> Result<Value> {
    let req = image.request();
    let gpu = req.surface.source();
    let cells = read(gpu, image.cells(), 16)?;
    let depths = read(gpu, image.depth(), 4)?;
    let normals = read(gpu, image.normals(), 8)?;
    let mut hits = 0;
    let mut expected_normals = [[0.; 3]; 6];
    for (side, expected) in expected_normals.iter_mut().enumerate() {
        let raw = std::array::from_fn(|a| {
            let mut v = [0.; 3];
            v[a] = 1.;
            gpu.volume().world_vector_to_voxel(v)[side / 2]
                * if side.is_multiple_of(2) { -1. } else { 1. }
        });
        *expected =
            orv_core::volume_material::gradient_normal(raw).ok_or("Invalid normal reference")?;
    }
    for ((cell, depth), normal) in cells
        .bytes
        .as_chunks::<16>()
        .0
        .iter()
        .zip(depths.bytes.as_chunks::<4>().0)
        .zip(normals.bytes.as_chunks::<8>().0)
    {
        let cell = word(cell);
        let depth = f32::from_le_bytes(*depth);
        let mut components = [0.; 4];
        for (a, v) in normal.as_chunks::<2>().0.iter().enumerate() {
            components[a] = half(u16::from_le_bytes(*v))?;
        }
        let normal = components;
        if !depth.is_finite() {
            return Err("Non-finite surface depth".into());
        }
        if cell[3] == 0 {
            if cell != [0; 4] || depth != 0. || normal != [0.; 4] {
                return Err("Uncleared surface background".into());
            }
            continue;
        }
        hits += 1;
        let side = ((cell[3] >> 16) & 7) as usize;
        let axis = side / 2;
        if kind == "empty"
            || side > 5
            || cell[0] >= 512
            || cell[1] >= 512
            || cell[2] >= 128
            || (cell[3] & 65535) != 1
            || cell[3] & !0x0107ffff != 0
        {
            return Err("Invalid surface owner".into());
        }
        let boundary = if axis == 2 {
            plane.map_or(if side.is_multiple_of(2) { 0 } else { 127 }, |z| z as u32)
        } else if side.is_multiple_of(2) {
            0
        } else {
            511
        };
        let grid = axis != 2 || plane.is_none();
        if cell[axis] != boundary || (cell[3] & (1 << 24) != 0) != grid {
            return Err("Ineligible reported interface".into());
        }
        let expected = expected_normals[side];
        if normal[3] != 1.
            || expected
                .into_iter()
                .zip(&normal)
                .any(|(a, b)| (a - b).abs() > 0.0006)
        {
            return Err("Incorrect surface normal".into());
        }
    }
    let mut compared = 0;
    for i in 0..256u32 {
        let side = req.size[0];
        let xy = [(i * 73 + 17) % side, (i * 151 + 31) % side];
        let index = (xy[1] * side + xy[0]) as usize;
        let actual = word(cells.bytes[index * 16..index * 16 + 16].try_into()?);
        let expected = (kind != "empty")
            .then(|| reference(req, xy, plane))
            .flatten();
        if let Some((word, depth, edge)) = expected {
            let actual_depth =
                f32::from_le_bytes(depths.bytes[index * 4..index * 4 + 4].try_into()?);
            if actual[3] == 0
                || (f64::from(actual_depth) - depth).abs() > 0.002
                || (edge > 0.0001 && actual != word)
            {
                return Err(
                    format!("Analytic ray differs at {xy:?}: {actual:?} versus {word:?}").into(),
                );
            }
        } else if actual != [0; 4] {
            return Err("Unexpected analytic ray hit".into());
        }
        compared += 1;
    }
    if kind != "empty" && hits == 0 {
        return Err("Generated surface is invisible".into());
    }
    Ok(
        json!({"verified_pixels":cells.bytes.len()/16,"eligible_hits":hits,"independent_f64_box_rays":compared,"sampled_depth_tolerance_mm":0.002}),
    )
}

fn composition_reference(
    scene: &OpaqueSurfaceScene,
    pixel: [u32; 2],
    kind: &str,
    plane: Option<usize>,
) -> Result<f64> {
    let req = scene.surface.request();
    let source = req.surface.source().volume();
    let frame = req.camera.frame(&req.display, 1.)?;
    let (world, direction) =
        frame.ray(pixel.map(|v| (f64::from(v) + 0.5) / f64::from(req.size[0])));
    let origin = source.world_to_voxel(world);
    let vector = source.world_vector_to_voxel(direction);
    let mut near = f64::NEG_INFINITY;
    let mut far = f64::INFINITY;
    for a in 0..3 {
        let maximum = (source.dimensions()[a] - 1) as f64;
        if vector[a] == 0. {
            if origin[a] < 0. || origin[a] > maximum {
                near = 1.;
                far = 0.;
                break;
            }
        } else {
            let t0 = -origin[a] / vector[a];
            let t1 = (maximum - origin[a]) / vector[a];
            near = near.max(t0.min(t1));
            far = far.min(t0.max(t1));
        }
    }
    let hit = (kind != "empty")
        .then(|| reference(req, pixel, plane))
        .flatten();
    let mut background = 0.0030352698;
    if let Some((word, depth, _)) = hit {
        let axis = ((word[3] >> 16) & 7) as usize / 2;
        let normal = std::array::from_fn(|a| {
            let mut u = [0.; 3];
            u[a] = 1.;
            source.world_vector_to_voxel(u)[axis]
        });
        let normal =
            orv_core::volume_material::gradient_normal(normal).ok_or("Invalid face normal")?;
        background = f64::from(scene.surface_luminance)
            * (f64::from(scene.surface_lighting.ambient)
                + f64::from(scene.surface_lighting.diffuse) * dot(normal, direction).abs());
        let offset = dot(
            std::array::from_fn(|a| world[a] - source.center()[a]),
            direction,
        );
        far = far.min(depth - offset);
    }
    let optics = scene
        .material
        .sample(0.)
        .ok_or("Invalid generated optics")?;
    let transmission = (-optics.extinction_per_mm * (far - near).max(0.)).exp();
    Ok(optics.luminance * (1. - transmission) + transmission * background)
}
fn verify_composition(
    image: &OpaqueSurfaceImage,
    kind: &str,
    plane: Option<usize>,
) -> Result<Value> {
    let scene = image.scene();
    let req = scene.surface.request();
    let data = read(req.surface.source(), image.texture(), 8)?;
    for pixel in data.bytes.as_chunks::<8>().0 {
        let mut values = [0.; 4];
        for (a, v) in pixel.as_chunks::<2>().0.iter().enumerate() {
            values[a] = half(u16::from_le_bytes(*v))?;
        }
        if values[0] < 0.
            || values[0] > 1.
            || values[0] != values[1]
            || values[0] != values[2]
            || values[3] != 1.
        {
            return Err("Invalid composed linear output".into());
        }
    }
    let mut max_error = 0f64;
    for i in 0..256u32 {
        let side = req.size[0];
        let xy = [(i * 73 + 17) % side, (i * 151 + 31) % side];
        let index = (xy[1] * side + xy[0]) as usize;
        let bytes = &data.bytes[index * 8..index * 8 + 2];
        let actual = half(u16::from_le_bytes([bytes[0], bytes[1]]))?;
        let expected = composition_reference(scene, xy, kind, plane)?;
        let error = (actual - expected).abs();
        if error > 0.001 {
            return Err(
                format!("Composed optics differs at {xy:?}: {actual} vs {expected}").into(),
            );
        }
        max_error = max_error.max(error);
    }
    Ok(
        json!({"verified_finite_grayscale_alpha_pixels":data.bytes.len()/8,"independent_f64_optical_rays":256,"max_sampled_linear_error":max_error,"linear_tolerance":0.001}),
    )
}
fn compose(
    renderer: &mut OpaqueSurfaceRenderer,
    scene: &OpaqueSurfaceScene,
    rows: u32,
) -> Result<Arc<OpaqueSurfaceImage>> {
    let height = scene.surface.request().size[1];
    for start in (0..height).step_by(rows as usize) {
        if let Some(image) =
            renderer.render_rows(scene, 0, start..(start + rows).min(height), || false)?
        {
            return Ok(image);
        }
        fence(scene.surface.request().surface.source().device())?;
    }
    Err("Incomplete generated composition".into())
}
#[path = "surface_projection_latency/acceleration.rs"]
mod acceleration;
fn main() -> Result<()> {
    match std::env::args().nth(1).as_deref() {
        Some("--accelerated") => return acceleration::run(),
        None => (),
        _ => return Err("Use no argument or --accelerated".into()),
    }
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
    let cpu = source()?;
    let gpu = GpuVolumeSource::new(&device, &queue, cpu.clone(), || false)?;
    let renderer = GpuSurfaceProjector::new(gpu.clone())?;
    let mut compositor = OpaqueSurfaceRenderer::new(gpu.clone(), wgpu::TextureFormat::Rgba8Unorm)?;
    let picker = GpuSurfacePicker::new(gpu.clone())?;
    let material = Arc::new(VolumeMaterial::new(&[-1., 1.].map(|value| {
        MaterialPoint {
            value,
            luminance: 0.2,
            extinction_per_mm: 0.005,
        }
    }))?);
    let source_admission = MemoryBudget::gpu().used() - before_gpu;
    let mut cases = Vec::new();
    for (kind, plane) in [
        ("empty", None),
        ("filled", None),
        ("one_cell_plane", Some(64)),
    ] {
        let mut labels = Labels::empty(cpu.clone())?;
        if kind != "empty" {
            labels.threshold(plane, [-1., 1.], 1, || false)?;
        }
        let surface = CellSurface::build(
            MaskOwner::Manual(Arc::new(labels)),
            1,
            None,
            SurfaceLimits::default(),
            || false,
        )?
        .0;
        let faces = surface.face_count();
        let (surface, _) = GpuCellSurface::upload(gpu.clone(), surface, None, || false)?;
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
            // Upload staging/previous-case retirement can still be queued when
            // the immutable geometry returns. Compare equivalent idle owners.
            fence(&device)?;
            let baseline = MemoryBudget::gpu().used();
            let image = renderer.render(req.clone(), || false)?;
            let verification = verify(&image, kind, plane)?;
            let stats = image.statistics();
            drop(image);
            fence(&device)?;
            let mut peak_gpu = baseline;
            let timing = measure(
                || {
                    let image = renderer.render(req.clone(), || false)?;
                    if image.request().size != [side; 2] || image.statistics() != stats {
                        return Err("Projection changed size/batching".into());
                    }
                    peak_gpu = peak_gpu.max(MemoryBudget::gpu().used());
                    drop(image);
                    fence(&device)
                },
                if side == 512 { 30 } else { 4 },
            )?;
            if MemoryBudget::gpu().used() != baseline {
                return Err("Projection reservations not restored".into());
            }
            let surface = renderer.render(req.clone(), || false)?;
            let mut volume = VolumeView::fit(&cpu);
            volume.camera = req.camera.clone();
            volume.projection = Projection::Composite;
            let mut scene = OpaqueSurfaceScene {
                surface,
                navigation: req.display.clone(),
                volume,
                material: material.clone(),
                lighting: Lighting::HEADLIGHT,
                surface_lighting: Lighting::HEADLIGHT,
                surface_luminance: 0.85,
            };
            let output = compose(&mut compositor, &scene, side)?;
            let verified_optics = verify_composition(&output, kind, plane)?;
            drop(output);
            fence(&device)?;
            let output = compose(&mut compositor, &scene, 16)?;
            let verified_rows = verify_composition(&output, kind, plane)?;
            drop(output);
            fence(&device)?;
            let composed_baseline = MemoryBudget::gpu().used();
            let cached = measure(
                || {
                    let output = compose(&mut compositor, &scene, side)?;
                    drop(output);
                    fence(&device)
                },
                if side == 512 { 30 } else { 4 },
            )?;
            let row_sequence = measure(
                || {
                    let output = compose(&mut compositor, &scene, 16)?;
                    drop(output);
                    fence(&device)
                },
                if side == 512 { 12 } else { 4 },
            )?;
            if MemoryBudget::gpu().used() != composed_baseline {
                return Err("Cached composition reservations not restored".into());
            }
            let mut front = Some(compose(&mut compositor, &scene, side)?);
            let mut combined_peak_gpu = MemoryBudget::gpu().used();
            let combined = measure(
                || {
                    // Keep two completed generations, matching a front image while
                    // the replacement is prepared. Geometry/source inputs are shared.
                    scene.surface = renderer.render(req.clone(), || false)?;
                    combined_peak_gpu = combined_peak_gpu.max(MemoryBudget::gpu().used());
                    let output = compose(&mut compositor, &scene, side)?;
                    combined_peak_gpu = combined_peak_gpu.max(MemoryBudget::gpu().used());
                    front = Some(output);
                    fence(&device)
                },
                if side == 512 { 12 } else { 4 },
            )?;
            let verified_combined = verify_composition(
                front.as_ref().ok_or("No completed front image")?,
                kind,
                plane,
            )?;
            let pick_frame = PickFrame::new(
                front.as_ref().ok_or("No completed pick frame")?.clone(),
                PickScope::new()?,
            );
            let current = PickState::from_frame(&pick_frame);
            let pixel = [side / 2; 2];
            let expected_hit = (kind != "empty")
                .then(|| reference(&req, pixel, plane))
                .flatten();
            let probe = picker.probe(&pick_frame, &current, pixel, || false)?;
            let expected_kind = if expected_hit.is_some() {
                SurfaceProbeKind::Hit
            } else {
                SurfaceProbeKind::Empty
            };
            if probe.kind() != expected_kind || !probe.is_current(&current, pixel) {
                return Err("Generated pick visibility/identity differs".into());
            }
            let mut transmission_error = 0f64;
            if let Some((word, _, _)) = expected_hit {
                let hit = probe.hit().ok_or("No verified interface hit")?;
                let face = hit.face;
                let actual = [
                    face.cell()[0],
                    face.cell()[1],
                    face.cell()[2],
                    u32::from(hit.key.number)
                        | ((face.side() as u32) << 16)
                        | (u32::from(face.boundary() == orv_core::surface::FaceBoundary::Grid)
                            << 24),
                ];
                if actual != word {
                    return Err("Generated physical pick differs from independent box ray".into());
                }
                let direction = req.camera.frame(&req.display, 1.)?.direction_unit;
                let axis = ((word[3] >> 16) & 7) as usize / 2;
                let normal = std::array::from_fn(|a| {
                    let mut u = [0.; 3];
                    u[a] = 1.;
                    cpu.world_vector_to_voxel(u)[axis]
                });
                let normal = orv_core::volume_material::gradient_normal(normal)
                    .ok_or("Invalid probe normal")?;
                let background = f64::from(scene.surface_luminance)
                    * (f64::from(scene.surface_lighting.ambient)
                        + f64::from(scene.surface_lighting.diffuse) * dot(normal, direction).abs());
                let expected =
                    (composition_reference(&scene, pixel, kind, plane)? - 0.2) / (background - 0.2);
                transmission_error = (f64::from(probe.foreground_transmittance()) - expected).abs();
                if transmission_error > 0.001 {
                    return Err("Generated pick foreground transmission differs".into());
                }
            }
            drop(probe);
            fence(&device)?;
            let pick_cpu = MemoryBudget::volumes().used();
            let pick_gpu = MemoryBudget::gpu().used();
            let probe_timing = measure(
                || {
                    let probe = picker.probe(&pick_frame, &current, pixel, || false)?;
                    if probe.kind() != expected_kind || !probe.is_current(&current, pixel) {
                        return Err("Pick identity changed during timing".into());
                    }
                    drop(probe);
                    fence(&device)
                },
                if side == 512 { 120 } else { 30 },
            )?;
            if MemoryBudget::volumes().used() != pick_cpu || MemoryBudget::gpu().used() != pick_gpu
            {
                return Err("Probe reservations not restored".into());
            }
            let probe_verification = json!({"pixel":pixel,"kind":format!("{expected_kind:?}"),"fixed_readback_bytes":56,"f64_box_membership_match":true,"foreground_transmission_error":transmission_error});
            drop(current);
            drop(pick_frame);
            drop(front);
            drop(scene);
            fence(&device)?;
            if MemoryBudget::gpu().used() != baseline {
                return Err("Combined surface/source reservations not restored".into());
            }
            cases.push(json!({"kind":kind,"size":[side,side],"faces":faces,"draws":stats.draws,"fenced_batches":stats.fenced_batches,
                "complete_projection_allocation_fence_drop_retirement":timing,"untimed_readback":verification,
                "cached_surface_source_composition":cached,"cached_surface_16_row_fenced_sequence":row_sequence,"complete_projection_plus_source_composition":combined,
                "untimed_optics_full":verified_optics,"untimed_optics_rows":verified_rows,"untimed_optics_combined":verified_combined,
                "complete_local_pixel_probe":probe_timing,"untimed_probe":probe_verification,
                "gpu_admission_baseline_bytes":baseline,"projection_max_observed_gpu_admission_bytes":peak_gpu,"combined_max_observed_gpu_admission_bytes":combined_peak_gpu}));
        }
    }
    drop(renderer);
    drop(compositor);
    drop(picker);
    drop(gpu);
    drop(cpu);
    fence(&device)?;
    if MemoryBudget::gpu().used() != before_gpu || MemoryBudget::volumes().used() != before_cpu {
        return Err("Benchmark owners/admission not restored".into());
    }
    println!(
        "{}",
        serde_json::to_string_pretty(
            &json!({"schema":3,"adapter":info.name,"backend":format!("{:?}",info.backend),
        "source_dimensions":[512,512,128],"logical_source_bytes":512*512*128*4u64,"shared_cpu_plane_bytes":512*512*4,
        "source_and_pipeline_gpu_admission_bytes":source_admission,"scope":"Separate known-interface projection, cached ordered source composition, fenced 16-row sequences and complete projection plus composition. Timings include fresh output allocation, submitted work/checks, drop and retirement. Source/geometry extraction/upload, readback of pixels, native input/display and RSS excluded. Homogeneous fully valid source, constant linear luminance0.2/extinction0.005 per mm, two-sided source/surface headlight and surface base0.85; missing-stencil performance is not measured. Admission is not driver VRAM.",
        "probe_scope":"Separate complete one-pixel worker probe: optional pipeline is prepared once; per-call uniforms/target/readback allocation, shared optical evaluation, 56B copy/map/status/color check, independent f64 cell traversal, identity/visibility checks, result drop and retirement fence are timed. Original full viewport coordinates; current centre pixel only. No native input/display or per-frame ID copy. Probe samples120 at512 and30 at2048.",
        "cases":cases,"reservations_restored":true})
        )?
    );
    Ok(())
}
