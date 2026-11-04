// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use std::cell::Cell;

fn id(x: usize, y: usize, original_z: usize) -> u8 {
    (1 + x + y * 8 + original_z * 53) as u8
}
fn asymmetric(source: Arc<Volume>) -> Labels {
    let mut labels = Labels::empty(source).unwrap();
    for (z, frame) in labels.source_arc().frames().iter().enumerate() {
        for y in 0..6 {
            for x in 0..8 {
                labels
                    .set_value((z * 6 + y) * 8 + x, id(x, y, frame.instance as usize))
                    .unwrap();
            }
        }
    }
    labels
}
fn plane(
    labels: &Labels,
    size: [u32; 2],
    position: [f64; 3],
    orientation: [f64; 6],
    spacing: [f64; 2],
) -> Frame {
    let mut image = (*labels.source.frames()[0]).clone();
    image.width = size[0];
    image.height = size[1];
    image.pixels = vec![1.; size[0] as usize * size[1] as usize].into();
    image.validity = None;
    image.rgba = None;
    image.color = None;
    let geometry = image.geometry.as_mut().unwrap();
    geometry.position = position;
    geometry.orientation = orientation;
    geometry.spacing = spacing;
    image
}

#[test]
fn categorical_planes_match_independent_axial_coronal_sagittal_and_oblique_locations_on_shear() {
    let labels = asymmetric(tests::source());
    let acquired = labels
        .sample_plane(&labels.source.frames()[1], || false)
        .unwrap();
    assert_eq!(acquired.dimensions(), [8, 6]);
    assert_eq!(
        acquired.ids(),
        (0..6)
            .flat_map(|y| (0..8).map(move |x| id(x, y, 1)))
            .collect::<Vec<_>>()
    );
    assert!(acquired.eligibility().iter().all(|v| *v == 1));
    let coronal = plane(
        &labels,
        [8, 3],
        [10., 23., 30.],
        [1., 0., 0., 0., 0., 1.],
        [2., 0.75],
    );
    let sampled = labels.sample_plane(&coronal, || false).unwrap();
    // At the third physical Z, the regular 0.25 mm shear moves nearest X back one.
    let expected: Vec<_> = (0..3)
        .flat_map(|z| {
            (0..8).map(move |x| {
                if z == 2 && x == 0 {
                    0
                } else {
                    id(x - usize::from(z == 2), 2, z)
                }
            })
        })
        .collect();
    assert_eq!(sampled.ids(), expected);
    assert_eq!(sampled.eligibility().iter().filter(|v| **v == 0).count(), 1);
    let sagittal = plane(
        &labels,
        [6, 3],
        [11.5, 20., 30.],
        [0., 1., 0., 0., 0., 1.],
        [2., 1.5],
    );
    let sampled = labels.sample_plane(&sagittal, || false).unwrap();
    assert_eq!(
        sampled.ids(),
        [2, 2, 1]
            .into_iter()
            .enumerate()
            .flat_map(|(z, x)| (0..6).map(move |y| id(x, y, z)))
            .collect::<Vec<_>>()
    );
    let angle = std::f64::consts::FRAC_1_SQRT_2;
    let oblique = plane(
        &labels,
        [3, 2],
        [11.75, 23., 32.],
        [angle, angle, 0., -angle, angle, 0.],
        [1.2 * 2f64.sqrt(), 0.6 * 2f64.sqrt()],
    );
    let sampled = labels.sample_plane(&oblique, || false).unwrap();
    assert_eq!(
        sampled.ids(),
        [
            id(2, 2, 1),
            id(3, 2, 1),
            id(4, 3, 1),
            id(0, 3, 1),
            id(1, 3, 1),
            id(2, 4, 1)
        ]
    );
    assert!(sampled.eligibility().iter().all(|v| *v == 1));
}

#[test]
fn categorical_sampling_respects_half_open_cells_ties_and_reversed_affine_steps() {
    let labels = asymmetric(tests::source());
    for (x, expected, valid) in [
        (-0.500001, 0, 0),
        (-0.5, id(0, 1, 0), 1),
        (0.5, id(1, 1, 0), 1),
        (7.499999, id(7, 1, 0), 1),
        (7.5, 0, 0),
    ] {
        let position = labels.source.voxel_to_world([x, 1., 0.]);
        let image = plane(
            &labels,
            [1, 1],
            position,
            [1., 0., 0., 0., 1., 0.],
            [1., 1.],
        );
        let sampled = labels.sample_plane(&image, || false).unwrap();
        assert_eq!(sampled.ids(), [expected], "voxel x {x}");
        assert_eq!(sampled.eligibility(), [valid], "voxel x {x}");
    }
    let original = tests::source();
    let mut frames = original
        .frames()
        .iter()
        .map(|frame| {
            let mut frame = (**frame).clone();
            frame.geometry.as_mut().unwrap().orientation[0] = -1.;
            Arc::new(frame)
        })
        .collect::<Vec<_>>();
    frames.swap(0, 2);
    let reversed = asymmetric(Arc::new(Volume::new(frames).unwrap()));
    assert!(
        reversed.source.voxel_to_world([0., 0., 1.])[2]
            < reversed.source.voxel_to_world([0., 0., 0.])[2]
    );
    for (z, expected) in [(30., id(2, 2, 0)), (32., id(2, 2, 1)), (34., id(3, 2, 2))] {
        let image = plane(
            &reversed,
            [1, 1],
            [8.5, 23., z],
            [1., 0., 0., 0., 1., 0.],
            [1., 1.],
        );
        assert_eq!(
            reversed.sample_plane(&image, || false).unwrap().ids(),
            [expected]
        );
    }
}

#[test]
fn labels_on_missing_source_and_output_samples_are_preserved_but_not_display_eligible() {
    for color in [false, true] {
        let original = tests::source();
        let frames = original
            .frames()
            .iter()
            .map(|frame| {
                let mut frame = (**frame).clone();
                if color {
                    frame.pixels = Arc::new([]);
                    let mut rgba = vec![255; 48 * 4];
                    rgba[3] = 0;
                    frame.rgba = Some(rgba.into());
                } else {
                    let mut mask = vec![1; 48];
                    mask[0] = 0;
                    frame.validity = Some(mask.into());
                }
                Arc::new(frame)
            })
            .collect();
        let labels = asymmetric(Arc::new(Volume::new(frames).unwrap()));
        let mut image = plane(
            &labels,
            [3, 1],
            [10., 20., 30.],
            [1., 0., 0., 0., 1., 0.],
            [1.5, 0.75],
        );
        image.pixels = vec![1., f32::NAN, 1.].into();
        let sampled = labels.sample_plane(&image, || false).unwrap();
        assert_eq!(sampled.ids(), [1, 2, 3]);
        assert_eq!(sampled.eligibility(), [0, 0, 1]);
        assert_eq!(labels.values().get(0).copied().unwrap(), 1);
    }
}

#[test]
fn source_label_counts_keep_missing_values_and_use_affine_determinant_instead_of_step_lengths() {
    let original = tests::source();
    let mut frames = original.frames().to_vec();
    let mut first = (*frames[0]).clone();
    let mut mask = vec![1; 48];
    mask[0] = 0;
    mask[1] = 0;
    first.validity = Some(mask.into());
    frames[0] = Arc::new(first);
    let mut labels = Labels::empty(Arc::new(Volume::new(frames).unwrap())).unwrap();
    for index in [0, 1, 3, 48, 49, 143] {
        labels.set_value(index, 5).unwrap();
    }
    labels.set_value(50, 7).unwrap();
    let counts = labels.statistics(|| false).unwrap();
    assert_eq!(
        counts.by_id[5],
        LabelCount {
            stored_voxels: 6,
            valid_voxels: 4
        }
    );
    assert_eq!(counts.by_id[5].missing_voxels(), 2);
    assert_eq!(
        counts.by_id[7],
        LabelCount {
            stored_voxels: 1,
            valid_voxels: 1
        }
    );
    assert_eq!(
        counts.by_id[0],
        LabelCount {
            stored_voxels: 137,
            valid_voxels: 137
        }
    );
    assert_eq!(
        counts.by_id.iter().map(|c| c.stored_voxels).sum::<u64>(),
        144
    );
    assert!((counts.voxel_volume_mm3 - 2.25).abs() < 1e-12);
    assert!((counts.by_id[5].stored_voxels as f64 * counts.voxel_volume_mm3 - 13.5).abs() < 1e-12);
    assert!((counts.by_id[5].valid_voxels as f64 * counts.voxel_volume_mm3 - 9.).abs() < 1e-12);
    assert_eq!(labels.values().get(0).copied().unwrap(), 5);
    assert!(labels.statistics(|| true).is_err());
}

#[test]
fn categorical_planes_refuse_wrong_references_slabs_malformed_or_large_outputs_and_cancel() {
    let labels = asymmetric(tests::source());
    let original = labels.source.frames()[0].clone();
    let mut image = (*original).clone();
    image.geometry.as_mut().unwrap().frame_of_reference = Some("other-generated-reference".into());
    assert!(labels.sample_plane(&image, || false).is_err());
    image = (*original).clone();
    image.slab = Some(crate::slab::Slab {
        projection: crate::slab::SlabProjection::Maximum,
        thickness_mm: 2.,
        intervals: 4,
    });
    assert!(labels.sample_plane(&image, || false).is_err());
    image = (*original).clone();
    image.pixels = Arc::new([]);
    assert!(labels.sample_plane(&image, || false).is_err());
    image = (*original).clone();
    image.width = MAX_PLANE_PIXELS as u32 + 1;
    image.height = 1;
    assert!(labels.sample_plane(&image, || false).is_err());
    let large = plane(
        &labels,
        [2048, 2],
        [10., 20., 30.],
        [1., 0., 0., 0., 1., 0.],
        [1., 1.],
    );
    let calls = Cell::new(0);
    let result = labels.sample_plane(&large, || {
        calls.set(calls.get() + 1);
        calls.get() >= 3
    });
    assert!(matches!(result, Err("Cancelled")));
    assert!(labels.sample_plane(&original, || true).is_err());
}
