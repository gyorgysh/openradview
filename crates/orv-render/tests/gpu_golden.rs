// SPDX-License-Identifier: GPL-3.0-or-later
//! Opt-in real GPU test: cargo nextest run -p orv-render --features gpu-tests
use orv_core::{ViewState, WindowLevel, study::Frame};
use orv_render::slice::SliceRenderer;

#[test]
fn scalar_source_validity_excludes_finite_padding_before_window_and_inversion() {
    use orv_core::scene::SceneState;
    use orv_render::snapshot::SnapshotRenderer;
    let mut frame = (*orv_core::study::demo_study().series[0].frames[0]).clone();
    frame.width = 4;
    frame.height = 2;
    frame.geometry = None;
    frame.pixels = vec![60000., 200., 200., 60000., 60000., 200., 200., 60000.].into();
    frame.validity = Some([0, 1, 1, 0, 0, 1, 1, 0].into());
    let mut renderer = pollster::block_on(SnapshotRenderer::new()).expect("Real GPU required");
    for invert in [false, true] {
        let scene = SceneState {
            schema_version: 1,
            source_sha256: "0".repeat(64),
            series_index: 0,
            view: ViewState {
                window: WindowLevel {
                    function: Default::default(),
                    width: 256.,
                    center: 128.,
                },
                invert,
                ..Default::default()
            },
            crosshair_mm: None,
            reformat_side: 2,
            output_size: [4, 2],
            reconstruction: None,
        };
        let result = renderer.capture(&frame, &scene, || false).unwrap();
        let png = image::load_from_memory(&result.png).unwrap().to_rgba8();
        for (i, pixel) in png.pixels().enumerate() {
            let expected = if frame.has_sample(i) {
                if invert { 55u8 } else { 200 }
            } else {
                10
            };
            assert!(pixel.0[..3].iter().all(|v| v.abs_diff(expected) <= 1));
            assert_eq!(pixel.0[3], 255);
        }
    }
}

#[test]
fn production_snapshot_png_matches_pixels_manifest_and_cancellation() {
    use orv_core::scene::{SceneState, hex};
    use orv_render::snapshot::{SnapshotManifest, SnapshotRenderer};
    use sha2::{Digest, Sha256};
    let frame = Frame {
        scalar: None,
        validity: None,
        color: None,
        nifti: None,
        rgba: None,
        width: 3,
        height: 2,
        pixels: vec![0.0, 128.0, 255.0, f32::NAN, 64.0, 192.0].into(),
        geometry: None,
        instance: 1,
        frame_index: 0,
        monochrome1: false,
        default_window: WindowLevel {
            function: Default::default(),
            width: 256.0,
            center: 128.0,
        },
    };
    let scene = SceneState {
        schema_version: 1,
        source_sha256: "0".repeat(64),
        series_index: 0,
        view: ViewState {
            window: frame.default_window,
            ..Default::default()
        },
        reconstruction: None,
        crosshair_mm: None,
        reformat_side: 512,
        output_size: [3, 2],
    };
    let mut renderer = pollster::block_on(SnapshotRenderer::new()).expect("Real GPU required");
    let snapshot = renderer.capture(&frame, &scene, || false).unwrap();
    let image = image::load_from_memory(&snapshot.png).unwrap().to_rgba8();
    assert_eq!(image.dimensions(), (3, 2));
    for (pixel, expected) in image.pixels().zip([0, 128, 255, 10, 64, 192]) {
        for c in &pixel.0[..3] {
            assert!(
                (i16::from(*c) - expected).abs() <= 1,
                "pixel {:?}, expected {expected}; image {:?}",
                pixel.0,
                image.as_raw()
            );
        }
        assert_eq!(pixel.0[3], 255);
    }
    assert_eq!(snapshot.manifest.scene_sha256, scene.hash().unwrap());
    assert_eq!(
        snapshot.manifest.png_sha256,
        hex(&Sha256::digest(&snapshot.png))
    );
    let json = serde_json::to_string(&snapshot.manifest).unwrap();
    assert_eq!(
        serde_json::from_str::<SnapshotManifest>(&json).unwrap(),
        snapshot.manifest
    );
    assert!(!snapshot.manifest.includes_ui_overlays);
    assert!(renderer.capture(&frame, &scene, || true).is_err());
    let repeated = renderer.capture(&frame, &scene, || false).unwrap();
    assert_eq!(snapshot.png, repeated.png);
}

#[test]
fn offscreen_scalar_golden_including_inversion() {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: orv_render::configured_backends().expect("Native backend selection"),
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapter = pollster::block_on(instance.request_adapter(&Default::default()))
        .expect("GPU test requires a real adapter");
    let (device, queue) =
        pollster::block_on(adapter.request_device(&orv_render::device_descriptor(&adapter)))
            .unwrap();
    let frame = Frame {
        scalar: None,
        validity: None,
        color: None,
        nifti: None,
        rgba: None,
        width: 4,
        height: 2,
        pixels: vec![-100.0, 0.0, 128.0, 255.0, 260.0, 64.0, 192.0, 100.0].into(),
        geometry: None,
        instance: 1,
        frame_index: 0,
        monochrome1: false,
        default_window: WindowLevel {
            function: Default::default(),
            width: 256.0,
            center: 128.0,
        },
    };
    for format in [
        wgpu::TextureFormat::Rgba8Unorm,
        wgpu::TextureFormat::Rgba8UnormSrgb,
    ] {
        let mut engine = SliceRenderer::new(&device, format);
        for color in [false, true, false] {
            let mut frame = frame.clone();
            if color {
                frame.rgba = Some(
                    [0u8, 0, 128, 255, 255, 64, 192, 100]
                        .into_iter()
                        .flat_map(|r| [r, 255 - r, r / 2, 255])
                        .collect::<Vec<_>>()
                        .into(),
                );
                frame.pixels = Vec::new().into();
            }
            engine.upload(&device, &queue, &frame).unwrap();
            let mut view = ViewState {
                window: frame.default_window,
                ..Default::default()
            };
            for invert in [false, true] {
                view.invert = invert;
                engine.set_view(&queue, &frame, &view, 2.0).unwrap();
                let size = wgpu::Extent3d {
                    width: 4,
                    height: 2,
                    depth_or_array_layers: 1,
                };
                let target = device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("golden target"),
                    size,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                    view_formats: &[],
                });
                let target_view = target.create_view(&Default::default());
                let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("golden readback"),
                    size: 512,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                });
                let mut encoder = device.create_command_encoder(&Default::default());
                {
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("golden pass"),
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
                encoder.copy_texture_to_buffer(
                    wgpu::TexelCopyTextureInfo {
                        texture: &target,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::TexelCopyBufferInfo {
                        buffer: &buffer,
                        layout: wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(256),
                            rows_per_image: Some(2),
                        },
                    },
                    size,
                );
                queue.submit([encoder.finish()]);
                let (tx, rx) = std::sync::mpsc::channel();
                buffer.map_async(wgpu::MapMode::Read, .., move |r| tx.send(r).unwrap());
                device
                    .poll(wgpu::PollType::Wait {
                        submission_index: None,
                        timeout: Some(std::time::Duration::from_secs(10)),
                    })
                    .unwrap();
                rx.recv_timeout(std::time::Duration::from_secs(10))
                    .unwrap()
                    .unwrap();
                let mapped = buffer.get_mapped_range(..).unwrap();
                let golden = [0u8, 0, 128, 255, 255, 64, 192, 100];
                for (i, value) in golden.iter().enumerate() {
                    let offset = (i / 4) * 256 + (i % 4) * 4;
                    for (c, channel) in mapped[offset..offset + 3].iter().enumerate() {
                        let base = if color {
                            [*value, 255 - value, value / 2][c]
                        } else {
                            *value
                        };
                        let expected = if invert { 255 - base } else { base };
                        assert!(
                            (i16::from(*channel) - i16::from(expected)).abs() <= 1,
                            "pixel {i}: {channel} != {expected}"
                        );
                    }
                    assert_eq!(mapped[offset + 3], 255);
                }
                drop(mapped);
                buffer.unmap();
            }
        }
    }
}

#[test]
fn color_snapshot_png_manifest_and_pan_match_the_same_renderer() {
    use orv_core::scene::SceneState;
    use orv_render::snapshot::SnapshotRenderer;
    let pixels = vec![
        255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 128, 64, 32, 255,
    ];
    let frame = Frame {
        scalar: None,
        validity: None,
        color: None,
        nifti: None,
        width: 2,
        height: 2,
        pixels: Vec::new().into(),
        rgba: Some(pixels.clone().into()),
        geometry: None,
        instance: 0,
        frame_index: 0,
        monochrome1: false,
        default_window: WindowLevel {
            function: Default::default(),
            width: 256.,
            center: 128.,
        },
    };
    let mut scene = SceneState {
        schema_version: 1,
        source_sha256: "0".repeat(64),
        series_index: 0,
        view: ViewState {
            window: frame.default_window,
            ..Default::default()
        },
        reconstruction: None,
        crosshair_mm: None,
        reformat_side: 512,
        output_size: [2, 2],
    };
    let mut renderer = pollster::block_on(SnapshotRenderer::new()).expect("Real Metal required");
    let snapshot = renderer.capture(&frame, &scene, || false).unwrap();
    assert_eq!(
        image::load_from_memory(&snapshot.png)
            .unwrap()
            .to_rgba8()
            .as_raw(),
        &pixels
    );
    assert_eq!(
        snapshot.manifest.image.unwrap().color_space.as_deref(),
        Some("sRGB assumed; no ICC calibration")
    );
    scene.view.pan = [0.5, 0.];
    let moved = renderer.capture(&frame, &scene, || false).unwrap();
    assert_eq!(
        image::load_from_memory(&moved.png)
            .unwrap()
            .to_rgba8()
            .as_raw(),
        &vec![
            10, 10, 10, 255, 255, 0, 0, 255, 10, 10, 10, 255, 0, 0, 255, 255
        ]
    );
}

#[test]
fn color_interpolation_is_linear_light_and_missing_pixels_keep_the_background() {
    use orv_core::scene::SceneState;
    use orv_render::snapshot::SnapshotRenderer;
    let mut frame = Frame {
        scalar: None,
        validity: None,
        color: None,
        nifti: None,
        width: 2,
        height: 2,
        pixels: Vec::new().into(),
        rgba: Some(
            vec![
                0, 0, 0, 255, 255, 255, 255, 255, 0, 0, 0, 255, 255, 255, 255, 255,
            ]
            .into(),
        ),
        geometry: None,
        instance: 0,
        frame_index: 0,
        monochrome1: false,
        default_window: WindowLevel {
            function: Default::default(),
            width: 256.,
            center: 128.,
        },
    };
    let scene = SceneState {
        schema_version: 1,
        source_sha256: "0".repeat(64),
        series_index: 0,
        view: ViewState {
            window: frame.default_window,
            ..Default::default()
        },
        crosshair_mm: None,
        reformat_side: 32,
        output_size: [3, 3],
        reconstruction: None,
    };
    let mut renderer = pollster::block_on(SnapshotRenderer::new()).expect("Real GPU required");
    let snapshot = renderer.capture(&frame, &scene, || false).unwrap();
    let png = image::load_from_memory(&snapshot.png).unwrap().to_rgba8();
    for value in &png.get_pixel(1, 1).0[..3] {
        assert!(value.abs_diff(188) <= 1);
    }
    assert_eq!(snapshot.manifest.renderer_version, 6);
    frame.rgba = Some(
        vec![
            0, 0, 0, 0, 255, 255, 255, 255, 0, 0, 0, 255, 255, 255, 255, 255,
        ]
        .into(),
    );
    let snapshot = renderer.capture(&frame, &scene, || false).unwrap();
    let png = image::load_from_memory(&snapshot.png).unwrap().to_rgba8();
    assert_eq!(png.get_pixel(1, 1).0, [10, 10, 10, 255]);
}

#[test]
fn explicit_grayscale_functions_and_tables_match_known_samples_without_baking_values() {
    use orv_core::{
        VoiFunction,
        scalar::{ScalarData, ScalarInfo, Storage, Unit, VoiChoices, VoiLut},
        scene::SceneState,
    };
    use orv_render::snapshot::SnapshotRenderer;
    let mut frame = (*orv_core::study::demo_study().series[0].frames[0]).clone();
    frame.width = 5;
    frame.height = 1;
    frame.geometry = None;
    frame.pixels = [0., 0.25, 0.5, 0.75, 1.].into();
    let mut renderer = pollster::block_on(SnapshotRenderer::new()).expect("Real GPU required");
    for (window, expected) in [
        (
            WindowLevel {
                width: 1.,
                center: 0.5,
                function: VoiFunction::LinearExact,
            },
            [0u8, 64, 128, 191, 255],
        ),
        (
            WindowLevel {
                width: 1.,
                center: 0.5,
                function: VoiFunction::Sigmoid,
            },
            [30, 69, 128, 186, 225],
        ),
    ] {
        for invert in [false, true] {
            let scene = SceneState {
                schema_version: 1,
                source_sha256: "0".repeat(64),
                series_index: 0,
                view: ViewState {
                    window,
                    invert,
                    ..Default::default()
                },
                crosshair_mm: None,
                reformat_side: 2,
                output_size: [5, 1],
                reconstruction: None,
            };
            let result = renderer.capture(&frame, &scene, || false).unwrap();
            let image = image::load_from_memory(&result.png).unwrap().to_rgba8();
            for (pixel, base) in image.pixels().zip(expected) {
                let expected = if invert { 255 - base } else { base };
                assert!(
                    pixel.0[..3]
                        .iter()
                        .all(|value| value.abs_diff(expected) <= 1),
                    "{pixel:?} vs {expected}"
                );
            }
        }
    }
    frame.pixels = [-2., -1., -0.5, 1., 2.].into();
    frame.scalar = Some(
        ScalarData::new(
            ScalarInfo {
                value_type: orv_core::scalar::ValueType::F32,
                storage: Storage::SignedInteger,
                stored_bits: 16,
                unit: Unit::Unknown,
                rescale: None,
                resampled: false,
            },
            VoiChoices::new(
                Vec::new(),
                vec![VoiLut::new(-2, 16, std::sync::Arc::from([0, 8192, 32768, 65535])).unwrap()],
            )
            .unwrap(),
        )
        .unwrap(),
    );
    frame.default_window = WindowLevel::lookup(0);
    for monochrome1 in [false, true] {
        frame.monochrome1 = monochrome1;
        let scene = SceneState {
            schema_version: 1,
            source_sha256: "0".repeat(64),
            series_index: 0,
            view: ViewState {
                window: frame.default_window,
                ..Default::default()
            },
            crosshair_mm: None,
            reformat_side: 2,
            output_size: [5, 1],
            reconstruction: None,
        };
        let result = renderer.capture(&frame, &scene, || false).unwrap();
        let image = image::load_from_memory(&result.png).unwrap().to_rgba8();
        for (pixel, base) in image.pixels().zip([0u8, 32, 32, 255, 255]) {
            let expected = if monochrome1 { 255 - base } else { base };
            assert!(
                pixel.0[..3]
                    .iter()
                    .all(|value| value.abs_diff(expected) <= 1),
                "{pixel:?} vs {expected}"
            );
        }
        assert_eq!(
            result.manifest.image.unwrap().scalar,
            Some(frame.scalar.as_ref().unwrap().info())
        );
    }
    let mut missing = frame.clone();
    missing.scalar = None;
    let scene = SceneState {
        schema_version: 1,
        source_sha256: "0".repeat(64),
        series_index: 0,
        view: ViewState {
            window: WindowLevel::lookup(0),
            ..Default::default()
        },
        crosshair_mm: None,
        reformat_side: 2,
        output_size: [5, 1],
        reconstruction: None,
    };
    assert!(renderer.capture(&missing, &scene, || false).is_err());
    let mut tiny = scene;
    tiny.view.window = WindowLevel {
        width: 1e-30,
        center: 0.,
        function: VoiFunction::LinearExact,
    };
    assert!(tiny.validate().is_ok());
    assert!(renderer.capture(&frame, &tiny, || false).is_err());
    tiny.view.window.width = 1e-20;
    assert!(renderer.capture(&frame, &tiny, || false).is_ok());
}
