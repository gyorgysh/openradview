// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
#[test]
fn illustration_bytes_hash_and_retained_admission_have_explicit_limits() {
    assert_eq!(
        hash(b"abc", &|| false).unwrap(),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert!(hash(b"abc", &|| true).is_err());
    let required = 64 * 32 * 4 * 4 + 3 * MIB + 16 * 1024;
    let budget = MemoryBudget::new(required - 1);
    assert!(artifact_admission([64, 32], &budget).is_err());
    assert_eq!(budget.used(), 0);
    budget.set_limit(required);
    let admitted = artifact_admission([64, 32], &budget).unwrap();
    assert_eq!(budget.used(), required);
    drop(admitted);
    assert_eq!(budget.used(), 0);
    for size in [[0, 1], [1025, 1], [1, 1025], [u32::MAX, u32::MAX]] {
        assert!(artifact_admission(size, &budget).is_err());
    }
    let mut bytes = LimitedBytes { bytes: Vec::new() };
    bytes.write_all(&vec![0; MANIFEST_LIMIT]).unwrap();
    assert!(bytes.write_all(&[1]).is_err());
    assert_eq!(bytes.bytes.len(), MANIFEST_LIMIT);
}
#[test]
fn composition_admission_covers_buffers_textures_and_upload_staging_before_gpu_growth() {
    let required = 97 * MIB;
    let budget = MemoryBudget::new(required - 1);
    assert!(composition_admission(&budget, MESH_LIMIT, 32 * 1024 * 1024).is_err());
    assert_eq!(budget.used(), 0);
    budget.set_limit(required);
    let memory = composition_admission(&budget, MESH_LIMIT, 32 * 1024 * 1024).unwrap();
    assert_eq!(budget.used(), required);
    drop(memory);
    assert_eq!(budget.used(), 0);
    assert!(composition_admission(&budget, MESH_LIMIT + 1, 0).is_err());
    assert!(composition_admission(&budget, 0, 32 * 1024 * 1024 + 1).is_err());
    assert_eq!(budget.used(), 0);
}
#[cfg(feature = "gpu-tests")]
pub(in crate::export::illustration) fn device() -> (wgpu::Device, wgpu::Queue) {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        flags: orv_render::instance_flags(),
        backends: orv_render::configured_backends().unwrap(),
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapter = pollster::block_on(instance.request_adapter(&Default::default()))
        .expect("Real GPU required");
    pollster::block_on(adapter.request_device(&orv_render::device_descriptor(&adapter))).unwrap()
}
#[cfg(feature = "gpu-tests")]
pub(in crate::export::illustration) fn input(
    device: wgpu::Device,
    queue: wgpu::Queue,
) -> FrozenInput {
    use crate::annotations::{Annotations, SavedMark, SavedTool};
    let mut series = orv_core::study::demo_study().series.remove(0);
    series.frames = crate::mpr_worker::tests::frames();
    for (index, frame) in series.frames.iter_mut().enumerate() {
        let frame = Arc::make_mut(frame);
        assert_eq!([frame.width, frame.height], [4, 4]);
        frame.geometry.as_mut().unwrap().spacing = [1., 1.];
        frame.scalar = None;
        frame.rgba = None;
        frame.color = None;
        frame.pixels = vec![
            0., 0., 0., 0., 0., 100., 100., 0., 0., 100., 100., 0., 0., 0., 0., 0.,
        ]
        .into();
        frame.validity = if index == 1 {
            let mut mask = vec![1; 16];
            mask[10] = 0;
            Some(mask.into())
        } else {
            None
        };
    }
    let source = Arc::new(series);
    let volume = Arc::new(orv_core::volume::Volume::new(source.frames.clone()).unwrap());
    let mut map = orv_core::labels::Labels::empty(volume).unwrap();
    map.threshold(None, [100., 100.], 1, || false).unwrap();
    let mut labels = crate::labels::LabelUi::default();
    labels.current = Some(Arc::new(map));
    let masks = labels.illustration_masks(0).unwrap();
    let scene = SceneState {
        schema_version: 1,
        source_sha256: "0".repeat(64),
        series_index: 0,
        view: orv_core::ViewState {
            slice: 1,
            window: orv_core::WindowLevel {
                width: 256.,
                center: 128.,
                function: Default::default(),
            },
            ..Default::default()
        },
        crosshair_mm: None,
        reformat_side: 2,
        // Odd magnification puts source centres on fragment centres, so validity
        // interpolation does not include a positive-weight missing neighbour.
        output_size: [68, 68],
        reconstruction: None,
    };
    let annotations = Annotations::restore(
        &[SavedMark {
            frame: 1,
            tool: SavedTool::Roi,
            points: vec![[0.75, 0.75], [2.75, 2.75]],
        }],
        &source,
        || false,
    )
    .unwrap();
    let outlines = annotations
        .outlines(&source.frames[1], &scene.view)
        .unwrap();
    FrozenInput {
        base: Input {
            series: source.clone(),
            scene,
            pixels: Pixels::Image(source.frames[1].clone()),
            device,
            queue,
        },
        outlines,
        masks: masks.map(Arc::new),
        mask_note: None,
        sheet: None,
    }
}
#[cfg(feature = "gpu-tests")]
#[test]
fn real_gpu_illustrations_keep_image_only_bytes_and_match_known_mask_and_outline_pixels() {
    let (device, queue) = device();
    let input = Arc::new(input(device, queue));
    let base = prepare(
        Request {
            input: input.clone(),
            choice: Choice::default(),
        },
        &|| false,
    )
    .unwrap();
    let mut scene = input.base.scene.clone();
    scene.source_sha256 = base.source_sha256.clone();
    let mut renderer =
        SnapshotRenderer::with_device(input.base.device.clone(), input.base.queue.clone());
    let ordinary = renderer
        .capture(input.image().unwrap(), &scene, || false)
        .unwrap();
    assert_eq!(base.png, ordinary.png);
    assert!(!ordinary.manifest.includes_ui_overlays);
    let masked = prepare(
        Request {
            input: input.clone(),
            choice: Choice {
                marks: false,
                masks: true,
                ..Default::default()
            },
        },
        &|| false,
    )
    .unwrap();
    let pixels = image::load_from_memory(&masked.png).unwrap().to_rgba8();
    assert!(
        pixels.get_pixel(25, 25).0[..3]
            .iter()
            .all(|v| v.abs_diff(139) <= 1),
        "masked pixel {:?}, base {:?}",
        pixels.get_pixel(25, 25).0,
        image::load_from_memory(&base.png)
            .unwrap()
            .to_rgba8()
            .get_pixel(25, 25)
            .0
    );
    assert_eq!(pixels.get_pixel(8, 8).0, [0, 0, 0, 255]);
    let original = image::load_from_memory(&base.png).unwrap().to_rgba8();
    assert_eq!(
        pixels.get_pixel(42, 42),
        original.get_pixel(42, 42),
        "Missing source cells receive no mask"
    );
    let json: serde_json::Value = serde_json::from_slice(&masked.manifest).unwrap();
    let expected: Vec<u8> = (0..16)
        .flat_map(|i| {
            if [5, 6, 9].contains(&i) {
                [197, 197, 197, 255]
            } else {
                [0, 0, 0, 0]
            }
        })
        .collect();
    assert_eq!(
        json["masks"]["raster_sha256"],
        hash(&expected, &|| false).unwrap()
    );
    assert!(json["marks"].is_null());
    assert_eq!(json["format"], "openradview_illustration");
    assert_eq!(json["png_sha256"], hash(&masked.png, &|| false).unwrap());
    assert!(
        !String::from_utf8(masked.manifest.clone())
            .unwrap()
            .contains("synthetic-worker")
    );
    let marked = prepare(
        Request {
            input: input.clone(),
            choice: Choice {
                marks: true,
                masks: false,
                ..Default::default()
            },
        },
        &|| false,
    )
    .unwrap();
    let pixels = image::load_from_memory(&marked.png).unwrap().to_rgba8();
    assert!(pixels.get_pixel(21, 34).0[0] > 190);
    let json: serde_json::Value = serde_json::from_slice(&marked.manifest).unwrap();
    assert_eq!(json["marks"][0]["number"], 1);
    assert_eq!(marked.preview.size, [68, 68]);
    assert!(
        marked
            .preview
            .pixels
            .iter()
            .zip(pixels.pixels())
            .all(|(a, b)| a.to_array() == b.0)
    );
    assert!(
        prepare(
            Request {
                input,
                choice: Choice::default()
            },
            &|| true
        )
        .is_err()
    );
}
#[cfg(feature = "gpu-tests")]
#[test]
fn illustration_source_verification_refuses_changed_pixels_and_private_reference_contexts() {
    let (device, queue) = device();
    for reference in [false, true] {
        let mut input = input(device.clone(), queue.clone());
        let mut changed = (*input.base.series).clone();
        let frame = Arc::make_mut(&mut changed.frames[1]);
        if reference {
            frame.geometry.as_mut().unwrap().frame_of_reference =
                Some("changed generated reference".into());
        } else {
            frame.pixels = vec![99.; 16].into();
        }
        input.base.series = Arc::new(changed);
        let error = prepare(
            Request {
                input: Arc::new(input),
                choice: Choice {
                    marks: true,
                    masks: true,
                    ..Default::default()
                },
            },
            &|| false,
        )
        .err()
        .unwrap();
        assert!(error.contains("differs"));
    }
}

#[cfg(feature = "gpu-tests")]
#[test]
fn categorical_gpu_composition_tracks_every_rotation_flip_and_zoom_pan() {
    let (device, queue) = device();
    let original = prepare(
        Request {
            input: Arc::new(input(device.clone(), queue.clone())),
            choice: Choice {
                marks: false,
                masks: true,
                ..Default::default()
            },
        },
        &|| false,
    )
    .unwrap();
    let pixels = image::load_from_memory(&original.png).unwrap().to_rgba8();
    for quarter_turns in 0..4 {
        for flip_horizontal in [false, true] {
            for flip_vertical in [false, true] {
                let mut captured = input(device.clone(), queue.clone());
                captured.base.scene.view.transform = orv_core::view::DisplayTransform {
                    quarter_turns,
                    flip_horizontal,
                    flip_vertical,
                };
                let rendered = prepare(
                    Request {
                        input: Arc::new(captured),
                        choice: original.choice,
                    },
                    &|| false,
                )
                .unwrap();
                let mut expected = match quarter_turns {
                    0 => pixels.clone(),
                    1 => image::imageops::rotate90(&pixels),
                    2 => image::imageops::rotate180(&pixels),
                    _ => image::imageops::rotate270(&pixels),
                };
                if flip_horizontal {
                    expected = image::imageops::flip_horizontal(&expected);
                }
                if flip_vertical {
                    expected = image::imageops::flip_vertical(&expected);
                }
                let actual = image::load_from_memory(&rendered.png).unwrap().to_rgba8();
                assert!(
                    expected
                        .as_raw()
                        .iter()
                        .zip(actual.as_raw())
                        .all(|(a, b)| a.abs_diff(*b) <= 1),
                    "rotation {quarter_turns}, flips {flip_horizontal}/{flip_vertical}"
                );
            }
        }
    }
    let mut captured = input(device, queue);
    captured.base.scene.view.zoom = 2.;
    captured.base.scene.view.pan = [0.125, -0.125];
    captured.base.scene.view.invert = true;
    let rendered = prepare(
        Request {
            input: Arc::new(captured),
            choice: original.choice,
        },
        &|| false,
    )
    .unwrap();
    let actual = image::load_from_memory(&rendered.png).unwrap().to_rgba8();
    assert!(
        actual.get_pixel(25, 8).0[..3]
            .iter()
            .all(|v| v.abs_diff(172) <= 1)
    );
    assert_eq!(actual.get_pixel(59, 42).0, [10, 10, 10, 255]);
}
