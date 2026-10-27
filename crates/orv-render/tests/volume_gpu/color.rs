// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use orv_core::{
    color::{ColorData, ColorEncoding, ColorInfo},
    scene::{Reconstruction, SceneState},
};
use orv_render::snapshot::SnapshotRenderer;
fn color_volume(colors: [[u8; 3]; 4]) -> Arc<Volume> {
    let scalar = volume(Some(0.), false);
    Arc::new(
        Volume::new(
            scalar
                .frames()
                .iter()
                .enumerate()
                .map(|(z, original)| {
                    let mut frame = (**original).clone();
                    let rgb = colors[z];
                    frame.pixels = Arc::from([]);
                    frame.rgba = Some(
                        (0..16)
                            .flat_map(|_| [rgb[0], rgb[1], rgb[2], 255])
                            .collect::<Vec<u8>>()
                            .into(),
                    );
                    frame.color = Some(Arc::new(ColorData {
                        info: ColorInfo {
                            bits: 8,
                            source_encoding: ColorEncoding::Rgb,
                            icc_sha256: None,
                        },
                        rgb: (0..16)
                            .flat_map(|_| rgb.map(u16::from))
                            .collect::<Vec<u16>>()
                            .into(),
                    }));
                    Arc::new(frame)
                })
                .collect(),
        )
        .unwrap(),
    )
}
fn scene(volume: &Volume, state: VolumeView, invert: bool) -> SceneState {
    SceneState {
        schema_version: 2,
        source_sha256: "0".repeat(64),
        series_index: 0,
        view: ViewState {
            window: volume.frames()[0].default_window,
            invert,
            ..Default::default()
        },
        crosshair_mm: None,
        reformat_side: 2,
        output_size: [1, 1],
        reconstruction: Some(Reconstruction::Volume(state)),
    }
}
fn linear(v: u8) -> f64 {
    let v = v as f64 / 255.;
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}
fn encoded(v: f64) -> f64 {
    let v = v.clamp(0., 1.);
    if v <= 0.0031308 {
        v * 12.92 * 255.
    } else {
        (1.055 * v.powf(1. / 2.4) - 0.055) * 255.
    }
}
#[test]
fn color_mip_takes_separate_channel_maxima_and_records_color_provenance() {
    let volume = color_volume([[255, 0, 64], [0, 255, 128], [64, 64, 255], [128, 128, 0]]);
    let mut renderer = pollster::block_on(SnapshotRenderer::new()).expect("Real GPU required");
    for invert in [false, true] {
        let mut state = VolumeView::fit(&volume);
        state.camera.pitch_degrees = 90.; // look along source Z, crossing every color slice
        let scene = scene(&volume, state, invert);
        let result = renderer
            .capture_volume(volume.clone(), &scene, || false)
            .unwrap();
        let png = image::load_from_memory(&result.png).unwrap().to_rgba8();
        for value in &png.get_pixel(0, 0).0[..3] {
            assert!(value.abs_diff(if invert { 0 } else { 255 }) <= 1, "{png:?}");
        }
        assert_eq!(result.manifest.renderer_version, 8);
        let metadata = &result.manifest.volume.unwrap().source;
        assert!(metadata.color.is_some());
        assert!(
            metadata
                .color_rendering
                .as_ref()
                .unwrap()
                .contains("RGB channel maxima")
        );
    }
}
#[test]
fn color_compositing_matches_physical_opacity_in_linear_light_for_both_qualities() {
    let source = [200, 100, 50];
    let volume = color_volume([source; 4]);
    let mut renderer = pollster::block_on(SnapshotRenderer::new()).expect("Real GPU required");
    for invert in [false, true] {
        for quality in [Quality::Interactive, Quality::Refined] {
            let mut state = VolumeView::fit(&volume);
            state.projection = Projection::Composite;
            state.quality = quality;
            state.transfer.opacity_per_mm = 0.2;
            let scene = scene(&volume, state.clone(), invert);
            let (_, _, [near, far]) = rays(&volume, &scene.view, &state, [1, 1], 0, 0).unwrap();
            let brightness = 0.2126 * linear(source[0])
                + 0.7152 * linear(source[1])
                + 0.0722 * linear(source[2]);
            let alpha = 1. - (-0.2 * brightness * (far - near)).exp();
            let expected = source.map(|v| {
                encoded(
                    alpha * linear(if invert { 255 - v } else { v }) + (1. - alpha) * linear(10),
                )
            });
            let result = renderer
                .capture_volume(volume.clone(), &scene, || false)
                .unwrap();
            let png = image::load_from_memory(&result.png).unwrap().to_rgba8();
            for (actual, expected) in png.get_pixel(0, 0).0[..3].iter().zip(expected) {
                assert!(
                    (*actual as f64 - expected).abs() <= 2.,
                    "actual {actual}, expected {expected}"
                );
            }
        }
    }
}

#[test]
fn native_volume_paint_preserves_rgb_channels_on_linear_and_srgb_targets() {
    let volume = color_volume([[200, 100, 50]; 4]);
    let (device, queue) = device();
    let state = VolumeView::fit(&volume);
    for format in [
        wgpu::TextureFormat::Rgba8Unorm,
        wgpu::TextureFormat::Rgba8UnormSrgb,
    ] {
        let mut renderer =
            VolumeRenderer::new(&device, &queue, volume.clone(), format, || false).unwrap();
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("native color display regression"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        renderer
            .render_region(
                &device,
                &mut encoder,
                &ViewState {
                    window: volume.frames()[0].default_window,
                    ..Default::default()
                },
                &state,
                [1, 1],
                [[0, 0], [1, 1]],
            )
            .unwrap();
        let view = target.create_view(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            renderer.paint(&mut pass);
        }
        let rgba = read(&device, &queue, &target, encoder, [1, 1]);
        for (actual, expected) in rgba[..3].iter().zip([200u8, 100, 50]) {
            assert!(actual.abs_diff(expected) <= 1, "{format:?} {rgba:?}");
        }
    }
}
