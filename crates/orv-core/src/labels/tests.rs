// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use crate::{WindowLevel, study::Geometry};
fn source() -> Arc<Volume> {
    Arc::new(
        Volume::new(
            (0..3)
                .map(|z| {
                    Arc::new(Frame {
                        slab: None,
                        scalar: None,
                        validity: None,
                        color: None,
                        nifti: None,
                        rgba: None,
                        width: 8,
                        height: 6,
                        pixels: (0..48)
                            .map(|i| (i + z * 100) as f32)
                            .collect::<Vec<_>>()
                            .into(),
                        geometry: Some(Geometry {
                            nifti_transform: None,
                            frame_of_reference: Some("synthetic-label-grid".into()),
                            position: [10. + z as f64 * 0.25, 20., 30. + z as f64 * 2.],
                            orientation: [1., 0., 0., 0., 1., 0.],
                            spacing: [1.5, 0.75],
                        }),
                        instance: z,
                        frame_index: 0,
                        monochrome1: false,
                        default_window: WindowLevel {
                            function: Default::default(),
                            width: 256.,
                            center: 128.,
                        },
                    })
                })
                .collect(),
        )
        .unwrap(),
    )
}
#[test]
fn threshold_brush_erase_preserve_other_labels_and_cancel() {
    let mut labels = Labels::empty(source()).unwrap();
    labels
        .paint(0, &[[2., 2.], [6., 2.]], 1, 7, || false)
        .unwrap();
    assert_eq!(labels.values[2 * 8 + 4], 7);
    assert_eq!(labels.values[3 * 8 + 4], 7);
    assert_eq!(labels.values[4 * 8 + 4], 0);
    labels.threshold(Some(0), [0., 47.], 3, || false).unwrap();
    assert_eq!(labels.values[2 * 8 + 4], 7);
    assert_eq!(labels.values[0], 3);
    assert_eq!(labels.values[48], 0);
    labels.paint(0, &[[4., 2.]], 1, 0, || false).unwrap();
    assert_eq!(labels.values[2 * 8 + 4], 0);
    assert_eq!(
        labels.threshold(None, [100., 120.], 2, || true),
        Err("Cancelled")
    );
    assert!(labels.paint(0, &[[f64::NAN, 1.]], 1, 1, || false).is_err());
    assert!(labels.paint(9, &[[1., 1.]], 1, 1, || false).is_err());
    assert!(labels.threshold(None, [2., 1.], 1, || false).is_err());
    assert_eq!(labels.slice_index(&labels.source.frames()[1]), Some(1));
    let mut frame = (*labels.source.frames()[1]).clone();
    frame.geometry.as_mut().unwrap().position[0] += 0.1;
    assert_eq!(labels.slice_index(&frame), None);
    let rgba = labels.plane_rgba(0).unwrap();
    assert_eq!(rgba.len(), 8 * 6 * 4);
    assert_eq!(rgba[(2 * 8 + 4) * 4 + 3], 0);
}
#[test]
fn exact_nifti_label_export_roundtrip_and_invalid_import_refusal() {
    let original = source();
    let mut labels = Labels::empty(original.clone()).unwrap();
    labels.paint(1, &[[3., 2.]], 1, 19, || false).unwrap();
    let mut bytes = Vec::new();
    labels.write_nifti(&mut bytes, || false).unwrap();
    assert_eq!(&bytes[4..12], b"n+2\0\r\n\x1a\n");
    assert_eq!(
        i32::from_le_bytes(bytes[504..508].try_into().unwrap()),
        1002
    );
    assert_eq!(
        f64::from_le_bytes(bytes[400..408].try_into().unwrap()),
        -0.75
    );
    assert_eq!(
        f64::from_le_bytes(bytes[416..424].try_into().unwrap()),
        -0.25
    );
    assert!(bytes[240..344].iter().all(|v| *v == 0));
    let path =
        std::env::temp_dir().join(format!("orv-synthetic-labels-{}.nii", std::process::id()));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .unwrap();
    std::io::Write::write_all(&mut file, &bytes).unwrap();
    drop(file);
    let study = crate::index::index_paths(std::slice::from_ref(&path), || false, |_| {}).unwrap();
    let volume = Volume::new(study.series[0].volume_frames(|| false).unwrap()).unwrap();
    let imported = Labels::import(original.clone(), &volume, || false).unwrap();
    assert_eq!(imported.values, labels.values);
    std::fs::remove_file(&path).unwrap();
    let mut frames = volume.frames().to_vec();
    let mut bad = (*frames[0]).clone();
    let mut p = bad.pixels.to_vec();
    p[0] = 1.5;
    bad.pixels = p.into();
    frames[0] = Arc::new(bad);
    assert!(Labels::import(original.clone(), &Volume::new(frames).unwrap(), || false).is_err());
    let moved = volume
        .frames()
        .iter()
        .map(|f| {
            let mut f = (**f).clone();
            f.geometry.as_mut().unwrap().position[0] += 1.;
            Arc::new(f)
        })
        .collect();
    assert!(Labels::import(original, &Volume::new(moved).unwrap(), || false).is_err());
    assert!(labels.write_nifti(Vec::new(), || true).is_err());
}
