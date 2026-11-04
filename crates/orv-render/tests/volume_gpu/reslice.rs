// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use orv_core::{
    Plane,
    oblique::ObliquePlane,
    slab::{Slab, SlabProjection},
    volume::ReformatGrid,
};
use orv_render::{reslice::GpuReslicer, volume_source::GpuVolumeSource};

fn read_scalars(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    mut encoder: wgpu::CommandEncoder,
) -> Vec<[f32; 4]> {
    let size = texture.size();
    let stride = (size.width * 16).div_ceil(256) * 256;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("generated reslice readback"),
        size: u64::from(stride) * u64::from(size.height),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
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
    queue.submit([encoder.finish()]);
    let (tx, rx) = mpsc::sync_channel(1);
    buffer.map_async(wgpu::MapMode::Read, .., move |result| {
        let _ = tx.send(result);
    });
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(Duration::from_secs(10)),
        })
        .unwrap();
    rx.recv_timeout(Duration::from_secs(10)).unwrap().unwrap();
    let result = {
        let bytes = buffer.get_mapped_range(..).unwrap();
        bytes
            .chunks_exact(stride as usize)
            .flat_map(|row| {
                row[..size.width as usize * 16]
                    .as_chunks::<16>()
                    .0
                    .iter()
                    .map(|pixel| {
                        std::array::from_fn(|i| {
                            f32::from_le_bytes(pixel[i * 4..i * 4 + 4].try_into().unwrap())
                        })
                    })
            })
            .collect()
    };
    buffer.unmap();
    result
}

fn compare(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    renderer: &mut GpuReslicer,
    grid: &ReformatGrid,
    cpu: &Frame,
) {
    assert_eq!(grid.dimensions(), [cpu.width, cpu.height]);
    assert_eq!(grid.geometry(), cpu.geometry.as_ref().unwrap());
    assert_eq!(grid.slab(), cpu.slab);
    let mut encoder = device.create_command_encoder(&Default::default());
    renderer.render(&mut encoder, grid, 0).unwrap();
    let values = read_scalars(device, queue, renderer.output_texture(0).unwrap(), encoder);
    for (index, pixel) in values.into_iter().enumerate() {
        assert_eq!(
            pixel[1] == 1.,
            cpu.has_sample(index),
            "validity {index}: {pixel:?}"
        );
        if cpu.has_sample(index) {
            let expected = cpu.pixels[index];
            assert!(
                (pixel[0] - expected).abs() <= 0.0002 * expected.abs().max(1.),
                "scalar {index}: {pixel:?} != {expected}"
            );
        } else {
            assert_eq!(pixel[0], 0.);
        }
        assert_eq!(&pixel[2..], &[0., 1.]);
    }
}

#[test]
fn anatomical_and_oblique_grids_match_cpu_on_an_anisotropic_sheared_source() {
    let (device, queue) = device();
    let volume = volume(None, false);
    let source = GpuVolumeSource::new(&device, &queue, volume.clone(), || false).unwrap();
    let mut renderer = GpuReslicer::new(source).unwrap();
    for plane in [Plane::Axial, Plane::Coronal, Plane::Sagittal] {
        let grid = volume.reformat_grid(plane, volume.center(), 64).unwrap();
        let cpu = volume
            .reformat(plane, volume.center(), 64, || false)
            .unwrap();
        compare(&device, &queue, &mut renderer, &grid, &cpu);
    }
    let plane = ObliquePlane::from_angles(volume.center(), [23., -17., 31.]).unwrap();
    let grid = volume.oblique_grid(&plane, 64, None).unwrap();
    let cpu = volume.reformat_oblique(&plane, 64, || false).unwrap();
    compare(&device, &queue, &mut renderer, &grid, &cpu);
}

#[test]
fn all_slab_projections_keep_the_declared_samples_and_missing_mask() {
    let (device, queue) = device();
    let mut frames = volume(None, false).frames().to_vec();
    // Missing contributors, valid zero-valued source samples, and whole missing
    // rays are tested independently of display mapping or readback quantization.
    for frame in &mut frames {
        let f = Arc::make_mut(frame);
        f.validity = Some(
            (0..16)
                .map(|i| u8::from(i != 5 && i != 6))
                .collect::<Vec<_>>()
                .into(),
        );
    }
    let volume = Arc::new(Volume::new(frames).unwrap());
    let source = GpuVolumeSource::new(&device, &queue, volume.clone(), || false).unwrap();
    let mut renderer = GpuReslicer::new(source).unwrap();
    let plane = ObliquePlane::anatomical(Plane::Axial, volume.center()).unwrap();
    for projection in [
        SlabProjection::Maximum,
        SlabProjection::Minimum,
        SlabProjection::Mean,
    ] {
        for intervals in [2, 16, 256] {
            let slab = Slab {
                projection,
                thickness_mm: 9.,
                intervals,
            };
            let grid = volume.oblique_grid(&plane, 32, Some(slab)).unwrap();
            let cpu = volume.reformat_slab(&plane, slab, 32, || false).unwrap();
            compare(&device, &queue, &mut renderer, &grid, &cpu);
        }
    }
}

#[test]
fn three_panes_share_one_source_with_immutable_dispatches_and_survive_renderer_drop() {
    let (device, queue) = device();
    let volume = volume(None, false);
    let source = GpuVolumeSource::new(&device, &queue, volume.clone(), || false).unwrap();
    let projection =
        VolumeRenderer::from_source(source.clone(), wgpu::TextureFormat::Rgba8Unorm, || false)
            .unwrap();
    let mut reslice = GpuReslicer::new(source.clone()).unwrap();
    assert_eq!(Arc::strong_count(&source), 3);
    drop(projection);
    assert_eq!(Arc::strong_count(&source), 2);
    let mut encoder = device.create_command_encoder(&Default::default());
    let grids = [Plane::Axial, Plane::Coronal, Plane::Sagittal]
        .map(|p| volume.reformat_grid(p, volume.center(), 32).unwrap());
    for (slot, grid) in grids.iter().enumerate() {
        reslice.render(&mut encoder, grid, slot).unwrap();
    }
    queue.submit([encoder.finish()]);
    for (slot, grid) in grids.iter().enumerate() {
        let actual = read_scalars(
            &device,
            &queue,
            reslice.output_texture(slot).unwrap(),
            device.create_command_encoder(&Default::default()),
        );
        let g = grid.geometry();
        for y in 0..grid.dimensions()[1] {
            for x in 0..grid.dimensions()[0] {
                let world = g.pixel_to_world([f64::from(x), f64::from(y)]).unwrap();
                let expected = volume.sample(world);
                let pixel = actual[(y * grid.dimensions()[0] + x) as usize];
                assert_eq!(pixel[1] > 0., expected.is_some());
                if let Some(expected) = expected {
                    assert!((pixel[0] - expected).abs() < 0.001);
                }
            }
        }
    }
    drop(reslice);
    assert_eq!(Arc::strong_count(&source), 1);
}

#[test]
fn sampling_rebases_large_world_offsets_and_refuses_wrong_references() {
    let (device, queue) = device();
    let original = volume(None, false);
    let frames = original
        .frames()
        .iter()
        .map(|f| {
            let mut f = (**f).clone();
            for value in &mut f.geometry.as_mut().unwrap().position {
                *value += 1_000_000.;
            }
            Arc::new(f)
        })
        .collect();
    let volume = Arc::new(Volume::new(frames).unwrap());
    let source = GpuVolumeSource::new(&device, &queue, volume.clone(), || false).unwrap();
    let mut renderer = GpuReslicer::new(source.clone()).unwrap();
    let grid = volume
        .reformat_grid(Plane::Coronal, volume.center(), 64)
        .unwrap();
    let cpu = volume
        .reformat(Plane::Coronal, volume.center(), 64, || false)
        .unwrap();
    compare(&device, &queue, &mut renderer, &grid, &cpu);
    let mut wrong_frames = original.frames().to_vec();
    for f in &mut wrong_frames {
        Arc::make_mut(f)
            .geometry
            .as_mut()
            .unwrap()
            .frame_of_reference = Some("different".into());
    }
    let wrong = Volume::new(wrong_frames).unwrap();
    let wrong_grid = wrong
        .reformat_grid(Plane::Axial, wrong.center(), 32)
        .unwrap();
    let mut encoder = device.create_command_encoder(&Default::default());
    assert!(renderer.render(&mut encoder, &wrong_grid, 0).is_err());
    assert!(renderer.render(&mut encoder, &grid, 3).is_err());
    assert!(GpuVolumeSource::new(&device, &queue, volume, || true).is_err());
}

#[test]
fn full_detail_thin_grid_preserves_stripes_and_ignores_zero_weight_missing_neighbors() {
    let (device, queue) = device();
    let frames = volume(None, false)
        .frames()
        .iter()
        .enumerate()
        .map(|(z, f)| {
            let mut f = (**f).clone();
            f.width = 257;
            f.height = 3;
            f.pixels = (0..3)
                .flat_map(|y| (0..257).map(move |x| (x % 2 * 4000 + y * 100 + z * 1000) as f32))
                .collect::<Vec<_>>()
                .into();
            f.validity = Some(
                (0..771)
                    .map(|i| u8::from(z != 1 || i != 385))
                    .collect::<Vec<_>>()
                    .into(),
            );
            let g = f.geometry.as_mut().unwrap();
            g.position = [0., 0., z as f64 * 2.];
            g.spacing = [1., 1.];
            Arc::new(f)
        })
        .collect();
    let volume = Arc::new(Volume::new(frames).unwrap());
    let source = GpuVolumeSource::new(&device, &queue, volume.clone(), || false).unwrap();
    let mut renderer = GpuReslicer::new(source).unwrap();
    let grid = volume
        .reformat_grid(Plane::Axial, [128., 1., 2.], 512)
        .unwrap();
    assert_eq!(grid.dimensions(), [257, 3]);
    let mut encoder = device.create_command_encoder(&Default::default());
    renderer.render(&mut encoder, &grid, 0).unwrap();
    let values = read_scalars(
        &device,
        &queue,
        renderer.output_texture(0).unwrap(),
        encoder,
    );
    assert_eq!(values[385], [0., 0., 0., 1.]);
    assert_eq!(values[384], [5100., 1., 0., 1.]);
    assert_eq!(values[386], [5100., 1., 0., 1.]);
    for (x, pixel) in values[..257].iter().enumerate() {
        assert_eq!(*pixel, [(x % 2 * 4000 + 1000) as f32, 1., 0., 1.]);
    }
}

#[test]
fn bounded_tiles_replace_the_previous_plane_and_refuse_invalid_regions() {
    let (device, queue) = device();
    let volume = volume(None, false);
    let source = GpuVolumeSource::new(&device, &queue, volume.clone(), || false).unwrap();
    let mut renderer = GpuReslicer::new(source).unwrap();
    let mut point = volume.center();
    point[2] = 2.;
    let before = volume.reformat_grid(Plane::Axial, point, 32).unwrap();
    point[2] = 4.;
    let after = volume.reformat_grid(Plane::Axial, point, 32).unwrap();
    let cpu = volume.reformat(Plane::Axial, point, 32, || false).unwrap();
    let [w, h] = after.dimensions();
    let mut encoder = device.create_command_encoder(&Default::default());
    renderer.render(&mut encoder, &before, 0).unwrap();
    for y in 0..h {
        renderer
            .render_region(&mut encoder, &after, 0, [[0, y], [w, 1]])
            .unwrap();
    }
    for region in [[[0, 0], [0, h]], [[1, 0], [w, h]], [[u32::MAX, 0], [2, h]]] {
        assert!(
            renderer
                .render_region(&mut encoder, &after, 0, region)
                .is_err()
        );
    }
    let values = read_scalars(
        &device,
        &queue,
        renderer.output_texture(0).unwrap(),
        encoder,
    );
    for (i, pixel) in values.into_iter().enumerate() {
        assert_eq!(pixel[1] > 0., cpu.has_sample(i));
        if cpu.has_sample(i) {
            assert!((pixel[0] - cpu.pixels[i]).abs() < 0.001);
        }
    }
}
