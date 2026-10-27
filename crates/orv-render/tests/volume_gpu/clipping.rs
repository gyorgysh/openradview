// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use orv_core::scene::{Reconstruction, SceneState};
use orv_render::snapshot::SnapshotRenderer;

fn scene(state: VolumeView, window: WindowLevel) -> SceneState {
    SceneState {
        schema_version: 2,
        source_sha256: "0".repeat(64),
        series_index: 0,
        view: ViewState {
            window,
            ..Default::default()
        },
        crosshair_mm: None,
        reformat_side: 2,
        output_size: [1, 1],
        reconstruction: Some(Reconstruction::Volume(state)),
    }
}
#[test]
fn clipped_compositing_uses_physical_path_length_and_empty_clips_remain_background() {
    let volume = volume(Some(200.), false);
    let mut state = VolumeView::fit(&volume);
    state.projection = Projection::Composite;
    state.transfer = TransferFunction {
        low: 0.,
        high: 200.,
        opacity_per_mm: 0.2,
        shape: TransferShape::Ramp,
    };
    state.clip = Some(PhysicalClip {
        minimum_mm: [-100., 1., -100.],
        maximum_mm: [100., 2., 100.],
    });
    let window = WindowLevel {
        width: 256.,
        center: 128.,
        function: Default::default(),
    };
    let mut renderer = pollster::block_on(SnapshotRenderer::new()).expect("Real GPU required");
    for quality in [Quality::Interactive, Quality::Refined] {
        state.quality = quality;
        let requested = scene(state.clone(), window);
        let result = renderer
            .capture_volume(volume.clone(), &requested, || false)
            .unwrap();
        let png = image::load_from_memory(&result.png).unwrap().to_rgba8();
        let opacity = 1. - (-0.2f64).exp();
        assert_gray(
            &png.get_pixel(0, 0).0,
            200. * opacity + 10. * (1. - opacity),
        );
        assert_eq!(result.manifest.scene, requested);
        let mut outside = state.clone();
        outside.clip = Some(PhysicalClip {
            minimum_mm: [100.; 3],
            maximum_mm: [101.; 3],
        });
        for projection in [Projection::Maximum, Projection::Composite] {
            outside.projection = projection;
            let result = renderer
                .capture_volume(volume.clone(), &scene(outside.clone(), window), || false)
                .unwrap();
            let png = image::load_from_memory(&result.png).unwrap().to_rgba8();
            assert_gray(&png.get_pixel(0, 0).0, 10.);
        }
    }
    let mut no_clip = state.clone();
    no_clip.clip = None;
    assert_ne!(
        scene(state, window).hash().unwrap(),
        scene(no_clip, window).hash().unwrap()
    );
}
#[test]
fn clipped_projection_preserves_scalar_padding_and_refuses_unrepresentable_bounds() {
    let source = volume(Some(200.), false);
    let frames = source
        .frames()
        .iter()
        .map(|frame| {
            let mut frame = (**frame).clone();
            frame.validity = Some(vec![0; frame.pixels.len()].into());
            Arc::new(frame)
        })
        .collect();
    let missing = Arc::new(Volume::new(frames).unwrap());
    let mut state = VolumeView::fit(&missing);
    state.clip = Some(PhysicalClip {
        minimum_mm: [-100., 1., -100.],
        maximum_mm: [100., 2., 100.],
    });
    let window = WindowLevel::SOFT_TISSUE;
    let mut renderer = pollster::block_on(SnapshotRenderer::new()).expect("Real GPU required");
    for projection in [Projection::Maximum, Projection::Composite] {
        state.projection = projection;
        let result = renderer
            .capture_volume(missing.clone(), &scene(state.clone(), window), || false)
            .unwrap();
        let png = image::load_from_memory(&result.png).unwrap().to_rgba8();
        assert_gray(&png.get_pixel(0, 0).0, 10.);
    }
    state.clip = Some(PhysicalClip {
        minimum_mm: [1_000_000.; 3],
        maximum_mm: [1_000_000.001; 3],
    });
    assert!(
        renderer
            .capture_volume(missing, &scene(state, window), || false)
            .is_err()
    );
}
