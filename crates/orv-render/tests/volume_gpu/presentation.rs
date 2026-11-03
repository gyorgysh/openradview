// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use orv_core::{
    VoiFunction,
    scalar::{ScalarData, ScalarInfo, Storage, Unit, ValueType, VoiChoices, VoiLut},
};

#[test]
fn fractional_transfer_range_preserves_physical_composite_opacity() {
    let (device, queue) = device();
    let source = volume(Some(0.25), false);
    let mut engine = VolumeRenderer::new(
        &device,
        &queue,
        source.clone(),
        wgpu::TextureFormat::Rgba8Unorm,
        || false,
    )
    .unwrap();
    let display = ViewState {
        window: WindowLevel {
            width: 0.5,
            center: 0.25,
            function: VoiFunction::LinearExact,
        },
        ..Default::default()
    };
    let mut state = VolumeView::fit(&source);
    state.projection = Projection::Composite;
    state.camera.vertical_span_mm = 7.;
    state.transfer = TransferFunction {
        low: 0.,
        high: 0.5,
        opacity_per_mm: 0.2,
        shape: TransferShape::Ramp,
    };
    let size = [7, 5];
    for quality in [Quality::Interactive, Quality::Refined] {
        state.quality = quality;
        let mut encoder = device.create_command_encoder(&Default::default());
        engine
            .render(&device, &mut encoder, &display, &state, size)
            .unwrap();
        let pixels = read(
            &device,
            &queue,
            engine.output_texture().unwrap(),
            encoder,
            size,
        );
        for y in 0..size[1] {
            for x in 0..size[0] {
                let expected = rays(&source, &display, &state, size, x, y).map_or(
                    10.,
                    |(_, _, [near, far])| {
                        let alpha = 1. - (-0.2 * 0.5 * (far - near)).exp();
                        127.5 * alpha + 10. * (1. - alpha)
                    },
                );
                let offset = ((y * size[0] + x) * 4) as usize;
                assert_gray(&pixels[offset..offset + 4], expected);
            }
        }
    }
}
#[test]
fn volume_functions_and_last_lookup_layer_match_constant_analytic_values() {
    let (device, queue) = device();
    let source = volume(Some(1.5), false);
    let tables = (0..16)
        .map(|index| {
            VoiLut::new(
                0,
                16,
                Arc::from(if index == 15 {
                    [0, 16384, 49152, 65535]
                } else {
                    [0, 8192, 32768, 65535]
                }),
            )
            .unwrap()
        })
        .collect();
    let scalar = ScalarData::new(
        ScalarInfo {
            modality_lut: None,
            value_type: ValueType::F32,
            storage: Storage::Float,
            stored_bits: 32,
            unit: Unit::Unknown,
            rescale: None,
            resampled: true,
        },
        VoiChoices::new(Vec::new(), tables).unwrap(),
    )
    .unwrap();
    let frames = source
        .frames()
        .iter()
        .map(|frame| {
            let mut frame = (**frame).clone();
            frame.scalar = Some(scalar.clone());
            Arc::new(frame)
        })
        .collect();
    let source = Arc::new(Volume::new(frames).unwrap());
    let size = [7, 5];
    let mut state = VolumeView::fit(&source);
    state.camera.vertical_span_mm = 7.;
    for format in [
        wgpu::TextureFormat::Rgba8Unorm,
        wgpu::TextureFormat::Rgba8UnormSrgb,
    ] {
        let mut engine =
            VolumeRenderer::new(&device, &queue, source.clone(), format, || false).unwrap();
        for (window, expected) in [
            (
                WindowLevel {
                    width: 3.,
                    center: 1.5,
                    function: VoiFunction::LinearExact,
                },
                127.5,
            ),
            (
                WindowLevel {
                    width: 3.,
                    center: 0.,
                    function: VoiFunction::Sigmoid,
                },
                224.603254,
            ),
            (WindowLevel::lookup(15), 16384. / 65535. * 255.),
        ] {
            let display = ViewState {
                window,
                ..Default::default()
            };
            let mut encoder = device.create_command_encoder(&Default::default());
            engine
                .render(&device, &mut encoder, &display, &state, size)
                .unwrap();
            let target = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("generated presentation target"),
                size: wgpu::Extent3d {
                    width: size[0],
                    height: size[1],
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let view = target.create_view(&Default::default());
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: None,
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
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
            let pixels = read(&device, &queue, &target, encoder, size);
            let mut visible = 0;
            for y in 0..size[1] {
                for x in 0..size[0] {
                    let inside = rays(&source, &display, &state, size, x, y).is_some();
                    if inside {
                        visible += 1;
                    }
                    let offset = ((y * size[0] + x) * 4) as usize;
                    assert_gray(
                        &pixels[offset..offset + 4],
                        if inside { expected } else { 10. },
                    );
                }
            }
            assert!(visible > 0);
        }
    }
}
#[test]
fn mapped_values_and_uniform_table_identity_survive_mpr_and_gpu_volume_snapshots() {
    use orv_core::scalar::ModalityLut;
    use orv_core::scene::{Reconstruction, SceneState};
    use orv_render::snapshot::SnapshotRenderer;
    let lut = ModalityLut::new(-2, 8, Arc::from([0, 51, 204, 255])).unwrap();
    let scalar = ScalarData::with_modality_lut(
        ScalarInfo {
            modality_lut: None,
            value_type: ValueType::F32,
            storage: Storage::SignedInteger,
            stored_bits: 12,
            unit: Unit::Hounsfield,
            rescale: None,
            resampled: false,
        },
        VoiChoices::new(vec![], vec![]).unwrap(),
        lut.clone(),
    )
    .unwrap();
    let base = volume(Some(51.), false);
    let frames = base
        .frames()
        .iter()
        .map(|frame| {
            let mut frame = (**frame).clone();
            frame.scalar = Some(scalar.clone());
            Arc::new(frame)
        })
        .collect();
    let source = Arc::new(Volume::new(frames).unwrap());
    let window = WindowLevel {
        width: 255.,
        center: 127.5,
        function: VoiFunction::LinearExact,
    };
    let mut state = VolumeView::fit(&source);
    state.projection = Projection::Maximum;
    state.camera.vertical_span_mm = 7.;
    let mut scene = SceneState {
        schema_version: 2,
        source_sha256: "0".repeat(64),
        series_index: 0,
        view: ViewState {
            window,
            ..Default::default()
        },
        crosshair_mm: None,
        reformat_side: 2,
        output_size: [7, 5],
        reconstruction: Some(Reconstruction::Volume(state.clone())),
    };
    let mut renderer = pollster::block_on(SnapshotRenderer::new()).expect("Real GPU required");
    for quality in [Quality::Interactive, Quality::Refined] {
        state.quality = quality;
        scene.reconstruction = Some(Reconstruction::Volume(state.clone()));
        let result = renderer
            .capture_volume(source.clone(), &scene, || false)
            .unwrap();
        let png = image::load_from_memory(&result.png).unwrap().to_rgba8();
        let mut visible = 0;
        for y in 0..5 {
            for x in 0..7 {
                let inside = rays(&source, &scene.view, &state, scene.output_size, x, y).is_some();
                visible += usize::from(inside);
                assert_gray(&png.get_pixel(x, y).0, if inside { 51. } else { 10. });
            }
        }
        assert!(visible > 0);
        let info = result.manifest.volume.unwrap().source.scalar.unwrap();
        assert!(info.resampled && info.rescale.is_none());
        assert_eq!(info.modality_lut, Some(lut.info()));
    }
    let point = source.center();
    let plane = source
        .reformat(orv_core::Plane::Axial, point, 37, || false)
        .unwrap();
    let valid = plane
        .pixels
        .iter()
        .enumerate()
        .filter(|(index, _)| plane.has_sample(*index))
        .collect::<Vec<_>>();
    assert!(!valid.is_empty() && valid.len() < plane.pixels.len());
    assert!(valid.iter().all(|(_, value)| **value == 51.));
    let scene = SceneState {
        schema_version: 1,
        view: ViewState {
            plane: orv_core::Plane::Axial,
            window,
            ..Default::default()
        },
        crosshair_mm: Some(point),
        reformat_side: 37,
        reconstruction: None,
        output_size: [37, 37],
        ..scene
    };
    let result = renderer.capture(&plane, &scene, || false).unwrap();
    let png = image::load_from_memory(&result.png).unwrap().to_rgba8();
    assert_gray(&png.get_pixel(18, 18).0, 51.);
    assert_eq!(
        result.manifest.image.unwrap().scalar.unwrap().modality_lut,
        Some(lut.info())
    );
}
