// SPDX-License-Identifier: GPL-3.0-or-later
mod common;
use orv_core::{Plane, scene::ImageMetadata};
use orv_mcp::consent::Operation;

#[test]
fn physical_scene_resolves_reproducibly_without_silent_position_changes() {
    let (session, mut scene) = common::session();
    let source = session.authorize(&scene, Operation::Snapshot).unwrap();
    scene.view.plane = Plane::Coronal;
    scene.crosshair_mm = Some([127.5, 127.5, 2.0]);
    let restored = serde_json::from_str(&serde_json::to_string(&scene).unwrap()).unwrap();
    let first = source.resolve(&scene, || false).unwrap();
    let second = source.resolve(&restored, || false).unwrap();
    assert_eq!(
        ImageMetadata::from_frame(&first),
        ImageMetadata::from_frame(&second)
    );
    assert_eq!(first.pixels.len(), second.pixels.len());
    for (a, b) in first.pixels.iter().zip(second.pixels.iter()) {
        assert_eq!(a.to_bits(), b.to_bits());
    }
    let g = first.geometry.as_ref().unwrap();
    assert_eq!(g.position[1], 127.5);
    assert!(first.width <= scene.reformat_side && first.height <= scene.reformat_side);
    assert!(source.resolve(&scene, || true).is_err());
    scene.crosshair_mm = Some([127.5, 127.5, 4000.0]);
    assert!(
        source.resolve(&scene, || false).is_err(),
        "Manifest positions must never be silently clamped"
    );
}

#[tokio::test]
async fn new_views_resolve_by_type_and_projections_cannot_masquerade_as_measurement_planes() {
    use orv_core::{oblique::ObliquePlane, scene::Reconstruction, volume_view::VolumeView};
    use orv_mcp::{
        catalog::Resolved,
        tools::{MeasureRequest, MeasurementKind, RoiRequest, ViewerTools},
    };
    use rmcp::handler::server::wrapper::Parameters;
    use std::sync::Arc;
    let (session, mut scene) = common::session();
    let source = session.authorize(&scene, Operation::Snapshot).unwrap();
    // Ask for a volume first, so its centre can define an explicit oblique plane.
    let mut study = orv_core::study::demo_study();
    study.series[0].frames.truncate(3);
    let volume = orv_core::volume::Volume::new(study.series.remove(0).frames).unwrap();
    scene.schema_version = 2;
    scene.reformat_side = 2;
    scene.reconstruction = Some(Reconstruction::Volume(VolumeView::fit(&volume)));
    assert!(source.resolve(&scene, || false).is_err());
    let Resolved::Volume(actual) = source.resolve_scene(&scene, || false).unwrap() else {
        panic!("Projection resolved as a source image")
    };
    assert_eq!(actual.dimensions(), [256, 256, 3]);
    assert!(source.resolve_scene(&scene, || true).is_err());
    assert!(session.authorize(&scene, Operation::Snapshot).is_err());
    session.approve_preview(&scene).unwrap();
    let tools = ViewerTools::new(session.clone(), Arc::new(|| {}));
    let distance = tools
        .measure(Parameters(MeasureRequest {
            scene: Some(scene.clone()),
            kind: MeasurementKind::Distance,
            points: vec![[0., 0.], [1., 1.]],
        }))
        .await;
    assert_eq!(distance.is_error, Some(true));
    assert!(
        serde_json::to_string(&distance)
            .unwrap()
            .contains("no physical image plane")
    );
    let roi = tools
        .roi_stats(Parameters(RoiRequest {
            scene: Some(scene.clone()),
            corners: [[0., 0.], [1., 1.]],
        }))
        .await;
    assert_eq!(roi.is_error, Some(true));
    scene.reformat_side = 64;
    let plane = ObliquePlane::from_angles(actual.center(), [25., -18., 11.]).unwrap();
    scene.reconstruction = Some(Reconstruction::Oblique(plane.clone()));
    let Resolved::Image(image) = source.resolve_scene(&scene, || false).unwrap() else {
        panic!("Oblique plane resolved as a projection")
    };
    assert_eq!(
        image.geometry.as_ref().unwrap().orientation[..3],
        plane.column_direction
    );
    assert!(image.width <= 64 && image.height <= 64);
    assert!(session.authorize(&scene, Operation::Snapshot).is_err());
    session.approve_preview(&scene).unwrap();
    let measurement = tools
        .measure(Parameters(MeasureRequest {
            scene: Some(scene.clone()),
            kind: MeasurementKind::Distance,
            points: vec![[0., 0.], [1., 1.]],
        }))
        .await;
    assert_ne!(measurement.is_error, Some(true));
    if let Some(Reconstruction::Oblique(plane)) = &mut scene.reconstruction {
        plane.center_mm[2] += 0.01;
    }
    assert!(session.authorize(&scene, Operation::Snapshot).is_err());
    session.revoke();
}

#[tokio::test]
async fn color_series_keeps_consent_scope_and_refuses_scalar_operations() {
    use orv_core::{
        ViewState, WindowLevel,
        scene::SceneState,
        study::{Frame, demo_study},
    };
    use orv_mcp::{
        catalog::PreparedSeries,
        consent::{Consent, DataScope, Session, ToolScopes},
        tools::{RoiRequest, SceneRequest, ViewRequest, ViewerTools},
    };
    use rmcp::handler::server::wrapper::Parameters;
    use std::sync::Arc;
    let mut series = demo_study().series.remove(0);
    let window = WindowLevel {
        function: Default::default(),
        width: 256.,
        center: 128.,
    };
    series.frames = vec![Arc::new(Frame {
        slab: None,
        scalar: None,
        validity: None,
        color: None,
        nifti: None,
        width: 2,
        height: 2,
        pixels: Vec::new().into(),
        rgba: Some(vec![255; 16].into()),
        geometry: None,
        instance: 0,
        frame_index: 0,
        monochrome1: false,
        default_window: window,
    })];
    let source = Arc::new(PreparedSeries::new(0, Arc::new(series), || false).unwrap());
    let scene = SceneState {
        schema_version: 1,
        source_sha256: source.description().source_sha256.clone(),
        series_index: 0,
        view: ViewState {
            window,
            ..Default::default()
        },
        reconstruction: None,
        crosshair_mm: None,
        reformat_side: 64,
        output_size: [2, 2],
    };
    let session = Session::grant(
        source,
        Consent {
            recipient: "Synthetic local color reader".into(),
            data: DataScope::SelectedSeries,
            tools: ToolScopes {
                view_control: true,
                snapshots: true,
                metadata: true,
                measurements: true,
            },
            preview_reviewed: true,
            burned_in_risk_accepted: true,
        },
        scene.clone(),
    )
    .unwrap();
    let tools = ViewerTools::new(session.clone(), Arc::new(|| {}));
    let tags = tools
        .get_tags(Parameters(SceneRequest { scene: None }))
        .await;
    assert!(
        tags.structured_content.unwrap()["image"]["color_space"]
            .as_str()
            .unwrap()
            .contains("sRGB")
    );
    let roi = tools
        .roi_stats(Parameters(RoiRequest {
            scene: None,
            corners: [[0., 0.], [1., 1.]],
        }))
        .await;
    assert_eq!(roi.is_error, Some(true));
    let mut contrast = scene.clone();
    contrast.view.window.width = 400.;
    assert_eq!(
        tools
            .set_view(Parameters(ViewRequest { scene: contrast }))
            .await
            .is_error,
        Some(true)
    );
    let mut pan = scene.clone();
    pan.view.pan = [0.1, 0.];
    assert_ne!(
        tools
            .set_view(Parameters(ViewRequest { scene: pan.clone() }))
            .await
            .is_error,
        Some(true)
    );
    pan.series_index = 1;
    assert_eq!(
        tools
            .set_view(Parameters(ViewRequest { scene: pan }))
            .await
            .is_error,
        Some(true)
    );
    session.revoke();
    assert_eq!(
        tools
            .get_tags(Parameters(SceneRequest { scene: None }))
            .await
            .is_error,
        Some(true)
    );
}

#[tokio::test]
async fn slab_sharing_requires_exact_approval_and_refuses_physical_measurement() {
    use orv_core::{oblique::ObliquePlane, scene::Reconstruction, slab::*};
    use orv_mcp::tools::{MeasureRequest, MeasurementKind, ViewerTools};
    use rmcp::handler::server::wrapper::Parameters;
    use std::sync::Arc;
    let (session, mut scene) = common::session();
    let source = session.authorize(&scene, Operation::Snapshot).unwrap();
    scene.schema_version = 2;
    scene.reformat_side = 32;
    scene.reconstruction = Some(Reconstruction::Slab(SlabPlane {
        plane: ObliquePlane::from_angles([127.5, 127.5, 2.], [0.; 3]).unwrap(),
        slab: Slab {
            projection: SlabProjection::Minimum,
            thickness_mm: 4.,
            intervals: 8,
        },
    }));
    assert!(session.authorize(&scene, Operation::Snapshot).is_err());
    session.approve_preview(&scene).unwrap();
    let image = source.resolve(&scene, || false).unwrap();
    let metadata = ImageMetadata::from_frame(&image);
    assert!(metadata.geometry.is_none());
    assert!(metadata.slab.is_some());
    assert!(
        source
            .resolve_scene(&scene, || false)
            .unwrap()
            .image()
            .is_err()
    );
    let tools = ViewerTools::new(session.clone(), Arc::new(|| {}));
    let reply = tools
        .measure(Parameters(MeasureRequest {
            scene: Some(scene.clone()),
            kind: MeasurementKind::Distance,
            points: vec![[0., 0.], [1., 1.]],
        }))
        .await;
    assert_eq!(reply.is_error, Some(true));
    if let Some(Reconstruction::Slab(state)) = &mut scene.reconstruction {
        state.slab.thickness_mm += 1.;
    }
    assert!(session.authorize(&scene, Operation::Snapshot).is_err());
    session.revoke();
    assert!(session.authorize(&scene, Operation::Metadata).is_err());
}
