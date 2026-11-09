// SPDX-License-Identifier: GPL-3.0-or-later
use super::super::Labels;
use super::*;
use crate::mask_identity::Identity;
use crate::{study::Frame, volume::Volume};
use std::cell::Cell;

fn source() -> Arc<Volume> {
    let original = super::super::tests::source();
    let frames = (0..34)
        .map(|z| {
            let mut frame: Frame = (*original.frames()[0]).clone();
            frame.width = 67;
            frame.height = 35;
            frame.pixels = vec![10.; 67 * 35].into();
            frame.geometry.as_mut().unwrap().position =
                [10. + z as f64 * 0.25, 20., 30. + z as f64 * 2.];
            Arc::new(frame)
        })
        .collect();
    Arc::new(Volume::new(frames).unwrap())
}
#[test]
fn irregular_tile_edges_stream_and_export_in_exact_source_order() {
    let source = source();
    let mut labels = Labels::empty(source.clone()).unwrap();
    let expected = (0..34)
        .flat_map(|z| {
            (0..35).flat_map(move |y| (0..67).map(move |x| ((x + 11 * y + 37 * z) % 256) as u8))
        })
        .collect::<Vec<_>>();
    for (index, value) in expected.iter().copied().enumerate() {
        labels.set_value(index, value).unwrap();
    }
    assert_eq!(labels.values().len(), 67 * 35 * 34);
    assert!(labels.values().iter().eq(expected.iter()));
    assert_eq!(labels.values().iter().len(), expected.len());
    assert!(labels.value([67, 0, 0]).is_none());
    assert!(labels.values().get(expected.len()).is_none());
    for (offset, count) in [
        (0, 65),
        (31, 65536),
        (67 * 32 + 31, 900),
        (expected.len() - 17, 17),
    ] {
        let mut bytes = vec![255; count];
        labels.values.copy_into(offset, &mut bytes).unwrap();
        assert_eq!(bytes, &expected[offset..offset + count]);
    }
    let sampled = labels.sample_plane(&source.frames()[32], || false).unwrap();
    assert_eq!(sampled.ids(), &expected[32 * 67 * 35..33 * 67 * 35]);
    assert!(sampled.eligibility().iter().all(|v| *v == 1));
    let mut bytes = Vec::new();
    labels.write_nifti(&mut bytes, || false).unwrap();
    assert_eq!(&bytes[544..], expected);
    assert!(
        labels
            .values
            .range(expected.len()..expected.len() + 1)
            .is_none()
    );
}

#[test]
fn checkpoints_share_pages_and_edits_publish_only_their_exact_tile_bounds() {
    let budget = MemoryBudget::new(8 * 1024 * 1024);
    let base = Labels::empty_with_budget(source(), budget.clone()).unwrap();
    let before = budget.used();
    assert_eq!(base.storage_statistics().allocated_tiles, 0);
    let mut first = base.try_clone(|| false).unwrap();
    assert_eq!(
        budget.used() - before,
        first.storage_statistics().directory_bytes
    );
    assert_eq!(first.revision(), base.revision());
    assert_eq!(first.object_id(), base.object_id());
    first.paint(0, &[[1., 1.]], 1, 7, || false).unwrap();
    assert_eq!(first.parent_revision(), Some(base.revision()));
    assert_ne!(first.revision(), base.revision());
    assert!(
        budget.used() - before < 40_000,
        "A small stroke must not copy the 79k-cell map"
    );
    assert_eq!(
        first.changed_tiles().collect::<Vec<_>>(),
        vec![LabelTile {
            coordinate: [0, 0, 0],
            bounds: [[0, 0, 0], [32, 32, 32]]
        }]
    );
    let mut second = first.try_clone(|| false).unwrap();
    assert!(Arc::ptr_eq(
        first.values.pages[0].as_ref().unwrap(),
        second.values.pages[0].as_ref().unwrap()
    ));
    second.paint(33, &[[65., 33.]], 1, 9, || false).unwrap();
    assert_eq!(second.parent_revision(), Some(first.revision()));
    assert_eq!(
        second.changed_tiles().collect::<Vec<_>>(),
        vec![LabelTile {
            coordinate: [2, 1, 1],
            bounds: [[64, 32, 32], [67, 35, 34]]
        }]
    );
    assert!(Arc::ptr_eq(
        first.values.pages[0].as_ref().unwrap(),
        second.values.pages[0].as_ref().unwrap()
    ));
    assert_eq!(first.value([65, 33, 33]), Some(0));
    assert_eq!(second.value([65, 33, 33]), Some(9));
    assert_eq!(base.value([1, 1, 0]), Some(0));
    let other = Labels::empty_with_budget(base.source_arc(), budget).unwrap();
    assert_ne!(base.object_id(), other.object_id());
}

#[test]
fn erase_compresses_empty_pages_but_live_snapshots_keep_their_charges() {
    let budget = MemoryBudget::new(8 * 1024 * 1024);
    let mut original = Labels::empty_with_budget(source(), budget.clone()).unwrap();
    original.paint(0, &[[1., 1.]], 1, 7, || false).unwrap();
    let old_revision = original.revision().clone();
    let mut erased = original.try_clone(|| false).unwrap();
    erased.paint(0, &[[1., 1.]], 1, 0, || false).unwrap();
    assert_eq!(erased.storage_statistics().allocated_tiles, 0);
    assert_eq!(original.storage_statistics().allocated_tiles, 1);
    assert_eq!(original.value([1, 1, 0]), Some(7));
    assert_eq!(erased.value([1, 1, 0]), Some(0));
    let charged = budget.used();
    let tile_bytes = original.storage_statistics().referenced_tile_bytes;
    drop(original);
    assert!(charged - budget.used() >= tile_bytes);
    drop(erased);
    assert_eq!(
        budget.used(),
        (std::mem::size_of::<Identity>() + 2 * std::mem::size_of::<usize>()) as u64
    );
    drop(old_revision);
    assert_eq!(budget.used(), 0);
}

#[test]
fn no_op_strokes_keep_the_revision_and_do_not_allocate_pages() {
    let budget = MemoryBudget::new(8 * 1024 * 1024);
    let mut labels = Labels::empty_with_budget(source(), budget.clone()).unwrap();
    let revision = labels.revision().clone();
    let used = budget.used();
    labels.paint(0, &[[1., 1.]], 1, 0, || false).unwrap();
    assert_eq!(labels.revision(), &revision);
    assert!(labels.parent_revision().is_none());
    assert_eq!(labels.changed_tiles().count(), 0);
    assert_eq!(budget.used(), used);
    labels.paint(0, &[[1., 1.]], 1, 3, || false).unwrap();
    let revision = labels.revision().clone();
    let used = budget.used();
    labels.paint(0, &[[1., 1.]], 1, 3, || false).unwrap();
    assert_eq!(labels.revision(), &revision);
    assert_eq!(labels.changed_tiles().count(), 0);
    assert!(budget.used() <= used);
}

#[test]
fn failed_private_edits_and_cancelled_directory_copies_release_all_new_charges() {
    let budget = MemoryBudget::new(8 * 1024 * 1024);
    let base = Labels::empty_with_budget(source(), budget.clone()).unwrap();
    let retained = budget.used();
    for cancel_after in [0, 1] {
        let calls = Cell::new(0);
        let result = base.try_clone(|| {
            let n = calls.get();
            calls.set(n + 1);
            n >= cancel_after
        });
        assert!(result.is_err());
        assert_eq!(budget.used(), retained);
    }
    let mut private = base.try_clone(|| false).unwrap();
    let tile_bytes =
        32 * 32 * 32 + (std::mem::size_of::<Tile>() + 2 * std::mem::size_of::<usize>());
    let revision_bytes = std::mem::size_of::<Identity>() + 2 * std::mem::size_of::<usize>();
    budget.set_limit(budget.used() + (tile_bytes + revision_bytes) as u64);
    // A circle crosses x=32: the first tile fits, the second does not. The
    // unpublished checkpoint may be partial; its immutable base stays unchanged.
    assert!(private.paint(0, &[[31., 8.]], 2, 7, || false).is_err());
    assert!(base.values().iter().all(|v| *v == 0));
    assert_eq!(private.storage_statistics().allocated_tiles, 1);
    drop(private);
    assert_eq!(budget.used(), retained);
    drop(base);
    assert_eq!(budget.used(), 0);
}

#[test]
fn tile_sizes_preserve_membership_while_small_copy_cost_tracks_tile_volume() {
    let dimensions = [129, 67, 66];
    let index = (33 * dimensions[1] + 33) * dimensions[0] + 65;
    let mut sizes = Vec::new();
    for side in [32, 64] {
        let budget = MemoryBudget::new(4 * 1024 * 1024);
        let base = Store::empty(dimensions, side, budget.clone()).unwrap();
        let mut changed = base.try_clone(|| false).unwrap();
        changed.set(index, 19).unwrap();
        for (i, &value) in changed.iter().enumerate() {
            assert_eq!(value, if i == index { 19 } else { 0 });
        }
        assert_eq!(base.get(index), Some(&0));
        sizes.push(changed.statistics().referenced_tile_bytes);
        drop(changed);
        drop(base);
        assert_eq!(budget.used(), 0);
    }
    let overhead = (std::mem::size_of::<Tile>() + 2 * std::mem::size_of::<usize>()) as u64;
    assert_eq!(sizes[1] - overhead, 8 * (sizes[0] - overhead));
}

#[test]
fn forward_and_undo_differences_keep_object_and_source_boundaries() {
    let mut a = Labels::empty(source()).unwrap();
    a.paint(0, &[[1., 1.]], 1, 3, || false).unwrap();
    let mut b = a.try_clone(|| false).unwrap();
    b.paint(33, &[[65., 33.]], 1, 4, || false).unwrap();
    let tile = LabelTile {
        coordinate: [2, 1, 1],
        bounds: [[64, 32, 32], [67, 35, 34]],
    };
    assert_eq!(
        b.changed_tiles_since(&a).unwrap().collect::<Vec<_>>(),
        vec![tile]
    );
    assert_eq!(
        a.changed_tiles_since(&b).unwrap().collect::<Vec<_>>(),
        vec![tile]
    );
    assert_eq!(a.changed_tiles_since(&a).unwrap().count(), 0);
    let other = Labels::empty(a.source_arc()).unwrap();
    assert!(a.changed_tiles_since(&other).is_err());
}

#[test]
fn clipped_tile_reads_stream_exact_values_and_refuse_forged_bounds_or_lengths() {
    let mut labels = Labels::empty(source()).unwrap();
    for z in 32..34 {
        for y in 32..35 {
            for x in 64..67 {
                labels
                    .set_value((z * 35 + y) * 67 + x, (x + y + z) as u8)
                    .unwrap();
            }
        }
    }
    let tiles = labels.tiles().collect::<Vec<_>>();
    assert_eq!(tiles.len(), 12);
    let edge = *tiles.last().unwrap();
    let mut values = vec![0; 18];
    labels.copy_tile(edge, &mut values).unwrap();
    let expected = (32..34)
        .flat_map(|z| (32..35).flat_map(move |y| (64..67).map(move |x| (x + y + z) as u8)))
        .collect::<Vec<_>>();
    assert_eq!(values, expected);
    assert!(labels.copy_tile(edge, &mut values[..17]).is_err());
    let mut wrong = edge;
    wrong.bounds[1][0] += 1;
    assert!(labels.copy_tile(wrong, &mut values).is_err());
    wrong = edge;
    wrong.coordinate[0] = usize::MAX;
    assert!(labels.copy_tile(wrong, &mut values).is_err());
}
