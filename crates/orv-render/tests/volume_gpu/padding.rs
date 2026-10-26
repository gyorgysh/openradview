// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use orv_core::scene::{Reconstruction, SceneState};
use orv_render::snapshot::SnapshotRenderer;

fn padded(all_missing: bool, mixed: bool) -> Arc<Volume> {
    let source = volume(Some(200.), false);
    let frames = source
        .frames()
        .iter()
        .enumerate()
        .map(|(z, original)| {
            let mut f = (**original).clone();
            let g = f.geometry.as_mut().unwrap();
            g.position = [0., 0., z as f64];
            g.spacing = [1.; 2];
            let mask: Vec<u8> = (0..16)
                .map(|i| u8::from(!all_missing && matches!(i % 4, 1 | 2) && matches!(i / 4, 1 | 2)))
                .collect();
            if mixed && z == 3 {
                f.validity = None;
            } else {
                f.pixels = mask
                    .iter()
                    .map(|v| if *v == 0 { 60000. } else { 200. })
                    .collect::<Vec<_>>()
                    .into();
                f.validity = Some(mask.into());
            }
            Arc::new(f)
        })
        .collect();
    Arc::new(Volume::new(frames).unwrap())
}
fn scene(volume: &Volume, projection: Projection, quality: Quality, invert: bool) -> SceneState {
    let mut state = VolumeView::fit(volume);
    state.projection = projection;
    state.quality = quality;
    state.camera.pitch_degrees = 90.;
    state.camera.yaw_degrees = 0.;
    state.camera.vertical_span_mm = 3.;
    state.transfer = TransferFunction {
        low: 0.,
        high: 200.,
        opacity_per_mm: 0.2,
        shape: TransferShape::Ramp,
    };
    SceneState {
        schema_version: 2,
        source_sha256: "0".repeat(64),
        series_index: 0,
        view: ViewState {
            window: WindowLevel {
                width: 256.,
                center: 128.,
            },
            invert,
            ..Default::default()
        },
        crosshair_mm: None,
        reformat_side: 2,
        output_size: [3, 3],
        reconstruction: Some(Reconstruction::Volume(state)),
    }
}

#[test]
fn scalar_padding_stays_out_of_mip_and_compositing_at_both_sampling_qualities() {
    let mut renderer = pollster::block_on(SnapshotRenderer::new()).expect("Real GPU required");
    for mixed in [false, true] {
        let volume = padded(false, mixed);
        assert!(volume.has_missing_scalar());
        for projection in [Projection::Maximum, Projection::Composite] {
            for quality in [Quality::Interactive, Quality::Refined] {
                let scene = scene(&volume, projection, quality, false);
                let result = renderer
                    .capture_volume(volume.clone(), &scene, || false)
                    .unwrap();
                let png = image::load_from_memory(&result.png).unwrap().to_rgba8();
                for (i, pixel) in png.pixels().enumerate() {
                    let expected = if i == 4 {
                        if projection == Projection::Maximum {
                            200.
                        } else {
                            let alpha = 1. - (-0.2f64 * 3.).exp();
                            200. * alpha + 10. * (1. - alpha)
                        }
                    } else if mixed && projection == Projection::Maximum {
                        200.
                    } else {
                        10.
                    };
                    assert_gray(&pixel.0, expected);
                }
                assert_eq!(result.manifest.renderer_version, 5);
            }
        }
    }
}

#[test]
fn entirely_padded_volume_rays_keep_the_background_even_when_inverted() {
    let volume = padded(true, false);
    let mut renderer = pollster::block_on(SnapshotRenderer::new()).expect("Real GPU required");
    for projection in [Projection::Maximum, Projection::Composite] {
        for quality in [Quality::Interactive, Quality::Refined] {
            let scene = scene(&volume, projection, quality, true);
            let result = renderer
                .capture_volume(volume.clone(), &scene, || false)
                .unwrap();
            let png = image::load_from_memory(&result.png).unwrap().to_rgba8();
            for pixel in png.pixels() {
                assert_gray(&pixel.0, 10.);
            }
        }
    }
}
