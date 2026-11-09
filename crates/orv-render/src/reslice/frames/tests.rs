// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use crate::brick_source::tests::{device, source};
use orv_core::Plane;
use orv_core::{
    oblique::ObliquePlane,
    resources::MemoryBudget,
    slab::{Slab, SlabProjection},
    volume::Volume,
};
use std::sync::mpsc;
use std::{cell::Cell, time::Duration};

fn texture_bytes(d: &wgpu::Device, q: &wgpu::Queue, t: &wgpu::Texture, bpp: u32) -> Vec<u8> {
    let size = t.size();
    let stride = (size.width * bpp).next_multiple_of(256);
    let bytes = u64::from(stride) * u64::from(size.height);
    let _gpu = allocation::reserve_bytes(bytes, d, q).unwrap();
    let _cpu = MemoryBudget::volumes().reserve(bytes * 2).unwrap();
    let buffer = d.create_buffer(&wgpu::BufferDescriptor {
        label: Some("generated scalar display verification"),
        size: bytes,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut e = d.create_command_encoder(&Default::default());
    e.copy_texture_to_buffer(
        t.as_image_copy(),
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
    q.submit([e.finish()]);
    let (tx, rx) = mpsc::sync_channel(1);
    buffer.map_async(wgpu::MapMode::Read, .., move |r| {
        let _ = tx.send(r);
    });
    fence(d);
    rx.recv_timeout(Duration::from_secs(10)).unwrap().unwrap();
    let mapped = buffer.get_mapped_range(..).unwrap();
    let mut result = Vec::new();
    for row in mapped.chunks(stride as usize) {
        result.extend_from_slice(&row[..(size.width * bpp) as usize]);
    }
    drop(mapped);
    buffer.unmap();
    result
}

#[test]
fn immutable_gpu_planes_match_their_corrected_cpu_frames_and_the_existing_display() {
    use crate::slice::SliceRenderer;
    use orv_core::{ViewState, WindowLevel};
    let (d, q) = device(None);
    let cpu = source([71, 37, 19], 0, true, true);
    let gpu = GpuVolumeSource::new(&d, &q, cpu.clone(), || false).unwrap();
    let mut renderer = GpuReslicer::new(gpu).unwrap();
    let grid = cpu.reformat_grid(Plane::Axial, cpu.center(), 64).unwrap();
    let image = renderer
        .render_frames([Some(&grid), None, None], || false)
        .unwrap()[0]
        .take()
        .unwrap();
    let bytes = texture_bytes(&d, &q, image.texture(), 16);
    for (i, p) in bytes.as_chunks::<16>().0.iter().enumerate() {
        let values: [f32; 4] = std::array::from_fn(|a| f32::from_le_bytes(p.as_chunks::<4>().0[a]));
        let expected = if image.frame().has_sample(i) {
            [image.frame().pixels[i], 1., 0., 1.]
        } else {
            [0., 0., 0., 1.]
        };
        assert_eq!(values.map(f32::to_bits), expected.map(f32::to_bits));
    }
    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let mut original = SliceRenderer::new(&d, format);
    let direct = SliceRenderer::from_reslice(image.clone(), format).unwrap();
    original.upload(&d, &q, image.frame()).unwrap();
    let copy = (**image.frame()).clone();
    assert!(
        direct
            .set_view(&q, &copy, &ViewState::default(), 1.)
            .is_err()
    );
    let target = d.create_texture(&wgpu::TextureDescriptor {
        label: Some("generated scalar plane presentation"),
        size: wgpu::Extent3d {
            width: 73,
            height: 59,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let target_view = target.create_view(&Default::default());
    for turn in 0..4 {
        let mut state = ViewState {
            window: WindowLevel {
                width: 700.,
                center: 150.,
                function: orv_core::VoiFunction::Linear,
            },
            ..Default::default()
        };
        state.transform.quarter_turns = turn;
        state.transform.flip_horizontal = turn % 2 == 1;
        state.invert = turn % 2 == 0;
        state.zoom = 1.15;
        state.pan = [0.02, -0.03];
        let mut pictures = Vec::new();
        for engine in [&original, &direct] {
            engine
                .set_view(&q, image.frame(), &state, 73. / 59.)
                .unwrap();
            let mut e = d.create_command_encoder(&Default::default());
            {
                let mut pass = e.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: None,
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &target_view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                engine.paint(&mut pass);
            }
            q.submit([e.finish()]);
            pictures.push(texture_bytes(&d, &q, &target, 4));
        }
        assert_eq!(pictures[0], pictures[1]);
    }
    let old = texture_bytes(&d, &q, image.texture(), 16);
    let point = cpu.voxel_to_world([22., 17., 5.]);
    let next = cpu.reformat_grid(Plane::Axial, point, 64).unwrap();
    drop(
        renderer
            .render_frames([Some(&next), None, None], || false)
            .unwrap(),
    );
    drop(renderer);
    fence(&d);
    assert_eq!(texture_bytes(&d, &q, image.texture(), 16), old);
    let (foreign, foreign_q) = device(None);
    let mut foreign_display = SliceRenderer::new(&foreign, format);
    assert!(foreign_display.upload_reslice(image.clone()).is_err());
    let foreign_source = GpuVolumeSource::new(&foreign, &foreign_q, cpu.clone(), || false).unwrap();
    let mut foreign_reslicer = GpuReslicer::new(foreign_source).unwrap();
    let foreign_image = foreign_reslicer
        .render_frames([Some(&grid), None, None], || false)
        .unwrap()[0]
        .take()
        .unwrap();
    let mut bound_foreign = SliceRenderer::from_reslice(foreign_image, format).unwrap();
    assert!(bound_foreign.upload_reslice(image).is_err());
}

fn fence(d: &wgpu::Device) {
    d.poll(wgpu::PollType::Wait {
        submission_index: None,
        timeout: Some(Duration::from_secs(30)),
    })
    .unwrap();
}
fn assert_pixels(frame: &Frame, expected: &Frame) {
    assert_eq!(frame.geometry, expected.geometry);
    assert_eq!(frame.slab, expected.slab);
    assert_eq!(frame.pixels.len(), expected.pixels.len());
    for (i, (&a, &b)) in frame.pixels.iter().zip(expected.pixels.iter()).enumerate() {
        assert_eq!(a.is_finite(), b.is_finite(), "validity {i}: {a}/{b}");
        if b.is_finite() {
            assert!((a - b).abs() <= 2e-4 * b.abs(), "value {i}: {a}/{b}");
        }
    }
}

#[test]
fn complete_slab_frames_keep_every_requested_sample_missing_rule_and_scalar_scale() {
    let (d, q) = device(None);
    for kind in [0, 2, 3] {
        let cpu = source([71, 37, 19], kind, true, true);
        let source = GpuVolumeSource::new(&d, &q, cpu.clone(), || false).unwrap();
        let mut renderer = GpuReslicer::new(source).unwrap();
        for projection in [
            SlabProjection::Minimum,
            SlabProjection::Maximum,
            SlabProjection::Mean,
        ] {
            for intervals in [2, 16, 256] {
                let plane = ObliquePlane::from_angles(cpu.center(), [23., -17., 11.]).unwrap();
                let slab = Slab {
                    projection,
                    thickness_mm: 9.,
                    intervals,
                };
                let grid = cpu.oblique_grid(&plane, 48, Some(slab)).unwrap();
                let actual = renderer
                    .read_frames([Some(&grid), None, None], || false)
                    .unwrap()[0]
                    .take()
                    .unwrap();
                let expected = cpu.reformat_slab(&plane, slab, 48, || false).unwrap();
                assert_pixels(&actual, &expected);
                assert_eq!(actual.slab, Some(slab));
            }
        }
    }
}

#[test]
fn completed_frames_retain_admission_and_pixels_after_renderer_reuse_cancellation_and_drop() {
    let (d, q) = device(None);
    let cpu = source([69, 39, 19], 0, false, false);
    let gpu = GpuVolumeSource::new(&d, &q, cpu.clone(), || false).unwrap();
    let ram = MemoryBudget::volumes();
    let vram = MemoryBudget::gpu();
    fence(&d);
    let base = (ram.used(), vram.used());
    let mut renderer = GpuReslicer::new(gpu).unwrap();
    let grid = cpu.reformat_grid(Plane::Axial, cpu.center(), 64).unwrap();
    let mut previous = renderer
        .read_frames([Some(&grid), None, None], || false)
        .unwrap();
    let image = previous[0].take().unwrap();
    let expected = image.pixels.iter().map(|v| v.to_bits()).collect::<Vec<_>>();
    let calls = Cell::new(0);
    drop(
        renderer
            .read_frames([Some(&grid), None, None], || {
                calls.set(calls.get() + 1);
                false
            })
            .unwrap(),
    );
    for cutoff in [0, 1, 4, calls.get() / 2, calls.get() - 1] {
        let n = Cell::new(0);
        assert!(
            renderer
                .read_frames([Some(&grid), None, None], || {
                    let call = n.get();
                    n.set(call + 1);
                    call >= cutoff
                })
                .is_err()
        );
    }
    fence(&d);
    let retained = ram.used();
    let limit = ram.limit();
    ram.set_limit(retained);
    let denied = renderer.read_frames([Some(&grid), None, None], || false);
    ram.set_limit(limit);
    assert!(denied.is_err());
    assert_eq!(ram.used(), retained);
    let gpu_retained = vram.used();
    let gpu_limit = vram.limit();
    vram.set_limit(gpu_retained);
    let gpu_denied = renderer.read_frames([Some(&grid), None, None], || false);
    vram.set_limit(gpu_limit);
    assert!(gpu_denied.is_err());
    fence(&d);
    assert_eq!(ram.used(), retained);
    assert!(vram.used() <= gpu_retained);
    let foreign = Arc::new(Volume::new(cpu.frames().to_vec()).unwrap());
    let foreign_grid = foreign
        .reformat_grid(Plane::Axial, foreign.center(), 64)
        .unwrap();
    assert!(
        renderer
            .read_frames([Some(&foreign_grid), None, None], || false)
            .is_err()
    );
    let recovered = renderer
        .read_frames([Some(&grid), None, None], || false)
        .unwrap()[0]
        .take()
        .unwrap();
    assert_pixels(&recovered, &image);
    assert_eq!(
        image.pixels.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
        expected
    );
    drop((renderer, recovered));
    fence(&d);
    assert!(ram.used() > base.0);
    assert_eq!(vram.used(), 0);
    let clone = image.clone();
    drop(image);
    assert!(ram.used() > 0);
    drop(clone);
    fence(&d);
    assert_eq!(ram.used(), base.0);
}
#[test]
fn empty_slots_allocate_no_frames_and_subnormal_sources_preserve_the_cpu_path() {
    let (d, q) = device(None);
    let base = source([7, 5, 3], 0, false, false);
    let frames = base
        .frames()
        .iter()
        .map(|frame| {
            let mut f = (**frame).clone();
            f.pixels = vec![f32::MIN_POSITIVE / 2.; f.pixels.len()].into();
            Arc::new(f)
        })
        .collect();
    let cpu = Arc::new(Volume::new(frames).unwrap());
    let gpu = GpuVolumeSource::new(&d, &q, cpu.clone(), || false).unwrap();
    let mut renderer = GpuReslicer::new(gpu).unwrap();
    fence(&d);
    let before = (MemoryBudget::volumes().used(), MemoryBudget::gpu().used());
    assert!(
        renderer
            .render_frames([None, None, None], || false)
            .unwrap()
            .iter()
            .all(Option::is_none)
    );
    assert_eq!(renderer.frame_statistics().planes, 0);
    assert_eq!(
        before,
        (MemoryBudget::volumes().used(), MemoryBudget::gpu().used())
    );
    let grid = cpu.reformat_grid(Plane::Axial, cpu.center(), 64).unwrap();
    assert!(
        renderer
            .render_frames([None, Some(&grid), None], || false)
            .is_err()
    );
    assert_eq!(
        before,
        (MemoryBudget::volumes().used(), MemoryBudget::gpu().used())
    );
    let reference = cpu.resample_grid(&grid, || false).unwrap();
    assert!(
        reference
            .pixels
            .iter()
            .filter(|v| v.is_finite())
            .all(|v| v.is_subnormal())
    );
    assert!(reference.pixels.iter().any(|v| v.is_subnormal()));
}
#[test]
fn completed_gpu_frames_preserve_cpu_validity_values_and_geometry_on_large_affine_grids() {
    let (d, q) = device(None);
    for (kind, missing, reverse) in [(0, false, false), (0, true, true), (2, false, false)] {
        let cpu = source([513, 263, 19], kind, missing, reverse);
        let source = GpuVolumeSource::new(&d, &q, cpu.clone(), || false).unwrap();
        let mut reslicer = GpuReslicer::new(source).unwrap();
        let point = cpu.voxel_to_world([256.5, 131.5, 8.5]);
        let grids = [Plane::Axial, Plane::Coronal, Plane::Sagittal]
            .map(|p| cpu.reformat_grid(p, point, 512).unwrap());
        let actual = reslicer
            .read_frames(grids.each_ref().map(Some), || false)
            .unwrap();
        for (slot, plane) in [Plane::Axial, Plane::Coronal, Plane::Sagittal]
            .into_iter()
            .enumerate()
        {
            let expected = cpu.reformat(plane, point, 512, || false).unwrap();
            let frame = actual[slot].as_ref().unwrap();
            assert_eq!(frame.geometry, expected.geometry);
            assert_eq!(
                [frame.width, frame.height],
                [expected.width, expected.height]
            );
            assert_eq!(frame.slab, expected.slab);
            assert_eq!(frame.default_window, expected.default_window);
            assert!(frame.memory.is_some());
            for (i, (&value, &reference)) in
                frame.pixels.iter().zip(expected.pixels.iter()).enumerate()
            {
                assert_eq!(
                    frame.has_sample(i),
                    expected.has_sample(i),
                    "kind{kind}/missing{missing}/slot{slot}/pixel{i} {value}/{reference}"
                );
                if expected.has_sample(i) {
                    assert!(
                        (value - reference).abs() <= 2e-4 * reference.abs(),
                        "kind{kind}/missing{missing}/slot{slot}/pixel{i} {value}/{reference}"
                    );
                }
            }
        }
    }
}
