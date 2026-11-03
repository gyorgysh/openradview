// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use crate::{
    WindowLevel,
    study::{Frame, Geometry},
    volume::Volume,
};
use std::cell::Cell;
fn map(missing: &[usize]) -> Labels {
    let foreground = [0, 1, 6, 8, 12, 13, 4, 15, 26, 27, 28, 44];
    let frames = (0..3)
        .map(|z| {
            let mut values = vec![-100.; 20];
            let mut valid = vec![1; 20];
            for &index in &foreground {
                if index / 20 == z {
                    values[index % 20] = 10.;
                }
            }
            for &index in missing {
                if index / 20 == z {
                    valid[index % 20] = 0;
                }
            }
            Arc::new(Frame {
                presentation: None,
                width: 5,
                height: 4,
                pixels: values.into(),
                validity: Some(valid.into()),
                geometry: Some(Geometry {
                    position: [z as f64 * 0.25, 0., -(z as f64) * 2.],
                    orientation: [1., 0., 0., 0., -1., 0.],
                    spacing: [2., 1.],
                    frame_of_reference: Some("generated-connectivity".into()),
                    nifti_transform: None,
                }),
                default_window: WindowLevel::source_range(-100., 10.),
                instance: z as i32,
                frame_index: 0,
                slab: None,
                scalar: None,
                color: None,
                nifti: None,
                rgba: None,
                monochrome1: false,
            })
        })
        .collect();
    Labels::empty(Arc::new(Volume::new(frames).unwrap())).unwrap()
}
fn request(scope: LabelScope, operation: RegionOperation) -> SelectionRequest {
    SelectionRequest {
        scope,
        label: 7,
        operation,
    }
}
fn grow(scope: LabelScope) -> SelectionRequest {
    request(
        scope,
        RegionOperation::Grow {
            seed: [0, 0, 0],
            range: [10., 10.],
        },
    )
}
fn selected(map: &Labels, label: u8) -> Vec<usize> {
    map.values
        .iter()
        .enumerate()
        .filter_map(|(i, v)| (*v == label).then_some(i))
        .collect()
}
#[test]
fn face_connectivity_is_scope_specific_and_excludes_diagonals_on_a_tilted_reversed_grid() {
    let base = Arc::new(map(&[]));
    let slice = base
        .select_region(grow(LabelScope::Slice(0)), || false)
        .unwrap();
    assert_eq!(
        slice.summary(),
        SelectionSummary {
            region_voxels: 3,
            changed_voxels: 3,
            components: 1
        }
    );
    assert_eq!(selected(&slice.apply(|| false).unwrap(), 7), [0, 1, 6]);
    let volume = base
        .select_region(grow(LabelScope::Volume), || false)
        .unwrap();
    assert_eq!(volume.summary().region_voxels, 9);
    assert_eq!(
        selected(&volume.apply(|| false).unwrap(), 7),
        [0, 1, 6, 8, 12, 13, 26, 27, 28]
    );
    assert!(base.values.iter().all(|v| *v == 0));
    assert!(Arc::ptr_eq(volume.base(), &base));
    assert_eq!(volume.request(), grow(LabelScope::Volume));
    assert!(base.source.voxel_to_world([0., 0., 1.])[2] < 0.);
}
#[test]
fn missing_samples_and_competing_ids_are_barriers_and_existing_target_cells_are_not_additions() {
    for missing in [false, true] {
        let mut labels = map(if missing { &[6] } else { &[] });
        labels.values[0] = 7;
        labels.values[6] = if missing { 7 } else { 9 };
        let base = Arc::new(labels);
        let selection = base
            .select_region(grow(LabelScope::Volume), || false)
            .unwrap();
        assert_eq!(selection.summary().region_voxels, 2);
        assert_eq!(selection.summary().changed_voxels, 1);
        let edited = selection.apply(|| false).unwrap();
        assert_eq!(
            &edited.values[..8],
            &[7, 7, 0, 0, 0, 0, if missing { 7 } else { 9 }, 0]
        );
        assert_eq!(base.values[1], 0);
    }
}
#[test]
fn inclusive_interval_and_boundary_seed_refusals_are_explicit() {
    let mut labels = map(&[]);
    let mut frames = labels.source.frames().to_vec();
    let mut frame = (*frames[0]).clone();
    let mut values = frame.pixels.to_vec();
    values[0] = 9.;
    values[6] = 11.;
    frame.pixels = values.into();
    frames[0] = Arc::new(frame);
    labels.source = Arc::new(Volume::new(frames).unwrap());
    let base = Arc::new(labels);
    let mut req = grow(LabelScope::Slice(0));
    req.operation = RegionOperation::Grow {
        seed: [0, 0, 0],
        range: [9., 11.],
    };
    assert_eq!(
        base.select_region(req, || false)
            .unwrap()
            .summary()
            .region_voxels,
        3
    );
    for (seed, range) in [
        ([0, 0, 0], [10., 10.]),
        ([5, 0, 0], [9., 11.]),
        ([0, 4, 0], [9., 11.]),
        ([0, 0, 3], [9., 11.]),
        ([0, 0, 1], [9., 11.]),
        ([0, 0, 0], [11., 9.]),
        ([0, 0, 0], [f32::NAN, 11.]),
        ([0, 0, 0], [9., f32::INFINITY]),
    ] {
        req.operation = RegionOperation::Grow { seed, range };
        assert!(base.select_region(req, || false).is_err(), "{req:?}");
    }
    req.operation = RegionOperation::Grow {
        seed: [4, 0, 0],
        range: [10., 10.],
    };
    assert_eq!(
        selected(
            &base
                .select_region(req, || false)
                .unwrap()
                .apply(|| false)
                .unwrap(),
            7
        ),
        [4]
    );
    req.label = 0;
    assert!(base.select_region(req, || false).is_err());
}
fn components() -> Arc<Labels> {
    let mut labels = map(&[6]);
    for index in [0, 1, 6, 8, 12, 13, 4, 15, 26, 27, 28] {
        labels.values[index] = 7;
    }
    labels.values[14] = 9;
    Arc::new(labels)
}
#[test]
fn keep_seeded_component_preserves_missing_stored_labels_and_unselected_scopes() {
    let base = components();
    let req = request(
        LabelScope::Slice(0),
        RegionOperation::KeepComponent { seed: [0, 0, 0] },
    );
    let selection = base.select_region(req, || false).unwrap();
    assert_eq!(
        selection.summary(),
        SelectionSummary {
            region_voxels: 2,
            changed_voxels: 5,
            components: 1
        }
    );
    let edited = selection.apply(|| false).unwrap();
    assert_eq!(selected(&edited, 7), [0, 1, 6, 26, 27, 28]);
    assert_eq!(edited.values[14], 9);
    assert!(
        base.select_region(
            request(
                LabelScope::Volume,
                RegionOperation::KeepComponent { seed: [1, 1, 0] }
            ),
            || false
        )
        .is_err()
    );
    assert!(
        base.select_region(
            request(
                LabelScope::Volume,
                RegionOperation::KeepComponent { seed: [2, 0, 0] }
            ),
            || false
        )
        .is_err()
    );
}
#[test]
fn island_cutoff_is_strict_and_component_counts_are_independently_known() {
    let base = components();
    let req = request(
        LabelScope::Volume,
        RegionOperation::RemoveSmall { minimum_voxels: 3 },
    );
    let selection = base.select_region(req, || false).unwrap();
    assert_eq!(
        selection.summary(),
        SelectionSummary {
            region_voxels: 4,
            changed_voxels: 4,
            components: 3
        }
    );
    assert_eq!(
        selected(&selection.apply(|| false).unwrap(), 7),
        [6, 8, 12, 13, 26, 27, 28]
    );
    let equal = base
        .select_region(
            request(
                LabelScope::Slice(0),
                RegionOperation::RemoveSmall { minimum_voxels: 2 },
            ),
            || false,
        )
        .unwrap();
    assert_eq!(
        selected(&equal.apply(|| false).unwrap(), 7),
        [0, 1, 6, 8, 12, 13, 26, 27, 28]
    );
    assert_eq!(equal.summary().changed_voxels, 2);
    let none = base
        .select_region(
            request(
                LabelScope::Volume,
                RegionOperation::RemoveSmall { minimum_voxels: 1 },
            ),
            || false,
        )
        .unwrap();
    assert_eq!(none.summary().changed_voxels, 0);
    for cutoff in [0, super::super::MAX_VOXELS + 1] {
        assert!(
            base.select_region(
                request(
                    LabelScope::Volume,
                    RegionOperation::RemoveSmall {
                        minimum_voxels: cutoff
                    }
                ),
                || false
            )
            .is_err()
        );
    }
}
#[test]
fn traversal_admission_cancellation_and_checkpoint_failures_release_private_buffers() {
    let base = components();
    let original = base.values.clone();
    let budget = MemoryBudget::new(299); // 60 mask bytes + 240 queue bytes required.
    assert!(
        base.select_with_budget(grow(LabelScope::Volume), &|| false, &budget)
            .is_err()
    );
    assert_eq!(budget.used(), 0);
    budget.set_limit(300);
    let req = request(
        LabelScope::Volume,
        RegionOperation::RemoveSmall { minimum_voxels: 3 },
    );
    let selection = base.select_with_budget(req, &|| false, &budget).unwrap();
    assert_eq!(budget.used(), 60); // Only the proposal mask remains.
    assert!(selection.apply(|| true).is_err());
    drop(selection);
    assert_eq!(budget.used(), 0);
    for after in 0..8 {
        let calls = Cell::new(0);
        let cancel = || {
            let n = calls.get();
            calls.set(n + 1);
            n >= after
        };
        let result = base.select_with_budget(req, &cancel, &budget);
        assert!(result.is_err(), "cancellation after check {after}");
        assert_eq!(budget.used(), 0);
        assert_eq!(base.values, original);
    }
    assert!(Labels::check_dimensions([usize::MAX, 2, 2]).is_err());
    assert!(Labels::check_dimensions([2049, 2048, 2]).is_err());
    assert!(Labels::check_dimensions([1024, 1024, 33]).is_err());
    assert!(Labels::check_dimensions([5, 4, 0]).is_err());
    assert_eq!(Labels::check_dimensions([5, 4, 3]), Ok(60));
}
#[test]
fn slice_scopes_use_source_order_offsets_and_color_growing_is_refused() {
    let base = Arc::new(map(&[]));
    let selection = base
        .select_region(
            request(
                LabelScope::Slice(1),
                RegionOperation::Grow {
                    seed: [1, 1, 1],
                    range: [10., 10.],
                },
            ),
            || false,
        )
        .unwrap();
    assert_eq!(
        selected(&selection.apply(|| false).unwrap(), 7),
        [26, 27, 28]
    );
    assert!(
        base.select_region(grow(LabelScope::Slice(3)), || false)
            .is_err()
    );
    let frames = base
        .source
        .frames()
        .iter()
        .map(|f| {
            let mut f = (**f).clone();
            f.rgba = Some(vec![255; 80].into());
            f.scalar = None;
            f.validity = None;
            f.pixels = Vec::new().into();
            Arc::new(f)
        })
        .collect();
    let color = Arc::new(Labels::empty(Arc::new(Volume::new(frames).unwrap())).unwrap());
    assert!(
        color
            .select_region(grow(LabelScope::Volume), || false)
            .is_err()
    );
}

#[test]
fn cancellation_during_a_large_traversal_or_private_apply_never_publishes_partial_cells() {
    let frames = map(&[])
        .source
        .frames()
        .iter()
        .map(|f| {
            let mut frame = (**f).clone();
            frame.width = 2048;
            frame.height = 2;
            frame.pixels = vec![10.; 4096].into();
            frame.validity = None;
            Arc::new(frame)
        })
        .collect();
    let base = Arc::new(Labels::empty(Arc::new(Volume::new(frames).unwrap())).unwrap());
    let budget = MemoryBudget::new(5 * 12288);
    let calls = Cell::new(0);
    let cancelled = || {
        let call = calls.get();
        calls.set(call + 1);
        call >= 4
    };
    assert!(
        base.select_with_budget(grow(LabelScope::Volume), &cancelled, &budget)
            .is_err()
    );
    assert_eq!(budget.used(), 0);
    let selection = base
        .select_with_budget(grow(LabelScope::Volume), &|| false, &budget)
        .unwrap();
    assert_eq!(selection.summary().changed_voxels, 12288);
    calls.set(0);
    assert!(selection.apply(cancelled).is_err());
    assert!(base.values.iter().all(|v| *v == 0));
    drop(selection);
    assert_eq!(budget.used(), 0);
}
