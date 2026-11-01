// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use crate::{
    WindowLevel,
    study::{Frame, Geometry},
};
use std::cell::Cell;
fn source(width: u32, height: u32) -> Arc<Volume> {
    Arc::new(
        Volume::new(
            (0..3)
                .map(|z| {
                    let count = width as usize * height as usize;
                    let mut valid = vec![1; count];
                    if z == 0 {
                        valid[1] = 0;
                    }
                    Arc::new(Frame {
                        width,
                        height,
                        pixels: vec![20.; count].into(),
                        validity: Some(valid.into()),
                        geometry: Some(Geometry {
                            position: [z as f64 * 0.25, 0., z as f64 * 2.],
                            orientation: [1., 0., 0., 0., 1., 0.],
                            spacing: [2., 1.],
                            frame_of_reference: Some("generated-mask-grid".into()),
                            nifti_transform: None,
                        }),
                        default_window: WindowLevel::source_range(0., 20.),
                        instance: z,
                        frame_index: 0,
                        slab: None,
                        scalar: None,
                        color: None,
                        nifti: None,
                        rgba: None,
                        monochrome1: false,
                    })
                })
                .collect(),
        )
        .unwrap(),
    )
}
const FRAMES: [(u16, usize); 4] = [(9, 2), (2, 0), (9, 0), (2, 1)];
const DATA: [u8; 4] = [0xa9, 0xb2, 0xd0, 0xff];
#[test]
fn bit_frames_cross_byte_boundaries_and_preserve_overlap_order_coverage_and_missing_source_cells() {
    let source = source(3, 2);
    let masks = Segments::from_binary(
        source.clone(),
        &[9, 2, 42],
        &FRAMES,
        &DATA,
        Some(true),
        || false,
    )
    .unwrap();
    assert!(Arc::ptr_eq(&masks.source_arc(), &source));
    assert_eq!(
        masks.infos().collect::<Vec<_>>(),
        [
            SegmentInfo {
                number: 9,
                positive_cells: 6,
                valid_source_cells: 5,
                supplied_planes: 2
            },
            SegmentInfo {
                number: 2,
                positive_cells: 5,
                valid_source_cells: 4,
                supplied_planes: 2
            },
            SegmentInfo {
                number: 42,
                positive_cells: 0,
                valid_source_cells: 0,
                supplied_planes: 0
            },
        ]
    );
    assert_eq!(masks.overlap_cells(), 2);
    let third = Segments::from_binary(
        source.clone(),
        &[9, 2, 42],
        &[(9, 2), (2, 0), (9, 0), (2, 1), (42, 0)],
        &[0xa9, 0xb2, 0xd0, 0xca],
        None,
        || false,
    )
    .unwrap();
    assert_eq!(third.overlap_cells(), 2); // The same shared cells counted once, not per pair.
    assert_eq!(third.infos().last().unwrap().positive_cells, 2);
    for (number, expected) in [(9, vec![0, 1, 3, 12, 15, 17]), (2, vec![1, 3, 8, 10, 11])] {
        let actual = (0..18)
            .filter(|i| masks.value(number, [i % 3, i % 6 / 3, i / 6]) == Some(true))
            .collect::<Vec<_>>();
        assert_eq!(actual, expected);
    }
    assert_eq!(masks.value(9, [1, 0, 0]), Some(true)); // Stored on a missing source cell.
    assert_eq!(masks.value(9, [2, 0, 0]), Some(false));
    assert_eq!(masks.value(9, [2, 0, 1]), None); // Omitted plane, not a supplied zero.
    assert!(!masks.plane_is_supplied(42, 0));
    assert!(!masks.plane_is_supplied(9, 3));
    assert_eq!(masks.value(99, [0, 0, 0]), None);
    assert_eq!(masks.value(9, [3, 0, 0]), None);
    assert!(Segments::from_binary(source, &[9, 2], &FRAMES, &DATA, Some(false), || false).is_err());
}
#[test]
fn supplied_empty_planes_are_distinct_and_unused_final_padding_bits_are_ignored() {
    let src = source(3, 2);
    let empty =
        Segments::from_binary(src.clone(), &[7], &[(7, 1)], &[0xc0, 0xff], None, || false).unwrap();
    assert_eq!(empty.infos().next().unwrap().positive_cells, 0);
    assert!(empty.plane_is_supplied(7, 1));
    assert_eq!(empty.value(7, [2, 1, 1]), Some(false));
    assert_eq!(empty.value(7, [2, 1, 0]), None);
    let solid =
        Segments::from_binary(src, &[7], &[(7, 1)], &[0x3f, 0], Some(false), || false).unwrap();
    assert_eq!(solid.infos().next().unwrap().positive_cells, 6);
    assert_eq!(solid.overlap_cells(), 0);
}
#[test]
fn ambiguous_plane_frames_ids_payloads_and_resource_bounds_are_refused() {
    let src = source(3, 2);
    for (numbers, frames, payload) in [
        (vec![0], vec![(0, 0)], vec![0, 0]),
        (vec![1, 1], vec![(1, 0)], vec![0, 0]),
        (vec![1], vec![(2, 0)], vec![0, 0]),
        (vec![1], vec![(1, 3)], vec![0, 0]),
        (vec![1], vec![(1, 0), (1, 0)], vec![0, 0]),
        (vec![1], vec![], vec![]),
        (vec![1], vec![(1, 0)], vec![0]),
        (vec![1], vec![(1, 0)], vec![0; 4]),
        (vec![], vec![(1, 0)], vec![0, 0]),
    ] {
        assert!(
            Segments::from_binary(src.clone(), &numbers, &frames, &payload, None, || false)
                .is_err()
        );
    }
    assert!(shape([3, 2, 3], 65).is_err());
    assert!(shape([3, 2, 4097], 1).is_err());
    assert!(shape([usize::MAX, 2, 2], 1).is_err());
    assert!(shape([1024, 1024, 32], 64).is_err());
    let budget = MemoryBudget::new(1);
    assert!(
        Segments::with_budget(
            src.clone(),
            &[9, 2],
            &FRAMES,
            &DATA,
            None,
            &|| false,
            &budget
        )
        .is_err()
    );
    assert_eq!(budget.used(), 0);
    budget.set_limit(shape([3, 2, 3], 2).unwrap().3);
    assert!(
        Segments::with_budget(
            src.clone(),
            &[9, 2],
            &FRAMES,
            &DATA,
            None,
            &|| false,
            &budget
        )
        .is_err()
    );
    assert_eq!(budget.used(), 0); // Retained admission succeeded; temporary admission failed.
    budget.set_limit(1024 * 1024);
    let masks =
        Segments::with_budget(src, &[9, 2], &FRAMES, &DATA, None, &|| false, &budget).unwrap();
    assert!(budget.used() > 0);
    drop(masks);
    assert_eq!(budget.used(), 0);
}
#[test]
fn cancellation_in_construction_discards_buffers_and_retains_no_partial_set() {
    let src = source(2048, 2);
    let payload = vec![0xff; 4096 * 3 / 8];
    let frames = [(1, 0), (1, 1), (1, 2)];
    let budget = MemoryBudget::new(1024 * 1024);
    for after in [0, 1, 2, 4, 8] {
        let calls = Cell::new(0);
        let cancel = || {
            let n = calls.get();
            calls.set(n + 1);
            n >= after
        };
        assert!(
            Segments::with_budget(src.clone(), &[1], &frames, &payload, None, &cancel, &budget)
                .is_err()
        );
        assert_eq!(budget.used(), 0);
        assert_eq!(src.frames()[0].pixels[0], 20.);
    }
    let masks =
        Segments::with_budget(src, &[1], &frames, &payload, None, &|| false, &budget).unwrap();
    assert_eq!(masks.infos().next().unwrap().positive_cells, 12288);
    assert_eq!(masks.infos().next().unwrap().valid_source_cells, 12287);
    drop(masks);
    assert_eq!(budget.used(), 0);
}
