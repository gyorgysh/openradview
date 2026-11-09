// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use crate::mask::MaskView;
use std::cell::Cell;

fn encoding(kind: FractionalKind, maximum: u8) -> FractionalEncoding {
    FractionalEncoding { kind, maximum }
}
fn source() -> Arc<Volume> {
    super::super::tests::source(3, 2)
}

#[test]
fn original_values_meaning_overlap_and_unknown_planes_survive_the_binary_view() {
    let source = source();
    let frames = [(65535, 2), (2, 0), (65535, 0), (2, 1), (42, 0)];
    let values = [
        [0, 99, 100, 200, 1, 150],
        [0, 200, 99, 100, 0, 100],
        [0, 100, 200, 100, 0, 0],
        [0; 6],
        [0, 150, 0, 100, 0, 0],
    ]
    .concat();
    for kind in [FractionalKind::Probability, FractionalKind::Occupancy] {
        let masks = Segments::from_fractional(
            source.clone(),
            &[65535, 2, 42],
            &frames,
            &values,
            encoding(kind, 200),
            Some(true),
            || false,
        )
        .unwrap();
        assert!(Arc::ptr_eq(&masks.source_arc(), &source));
        assert_eq!(
            masks.encoding(),
            SegmentEncoding::Fractional(encoding(kind, 200))
        );
        for (f, &(id, z)) in frames.iter().enumerate() {
            let mask = masks.mask(id).unwrap();
            assert_eq!(mask.encoding().maximum(), 200);
            for i in 0..6 {
                assert_eq!(mask.raw_value([i % 3, i / 3, z]), Some(values[f * 6 + i]));
            }
        }
        let mask = masks.mask(65535).unwrap();
        assert_eq!(mask.raw_value([1, 0, 2]), Some(99));
        assert_eq!(mask.value([1, 0, 2]), Some(false));
        assert_eq!(mask.raw_value([2, 0, 2]), Some(100));
        assert_eq!(mask.value([2, 0, 2]), Some(true));
        assert_eq!(mask.raw_value([0, 0, 1]), None);
        assert_eq!(mask.raw_value([3, 0, 0]), None);
        assert_eq!(masks.mask(2).unwrap().raw_value([0, 0, 1]), Some(0));
        assert_eq!(masks.mask(2).unwrap().raw_value([0, 0, 2]), None);
        let sample = MaskView::supplied(mask).sample([1, 0, 0]).unwrap();
        assert!(sample.known && sample.present && !sample.source_valid);
        assert_eq!(mask.raw_value([1, 0, 0]), Some(100));
        assert_eq!(masks.overlap_cells(), 2); // A third membership is counted once.
        let info = masks.infos().next().unwrap();
        assert_eq!((info.positive_cells, info.valid_source_cells), (6, 5));
        for z in 0..3 {
            let sampled = masks.sample_plane(&source.frames()[z], || false).unwrap();
            for i in 0..6 {
                for (slot, &id) in sampled.numbers().iter().enumerate() {
                    let expected = masks.value(id, [i % 3, i / 3, z]);
                    assert_eq!(
                        sampled.positive_bits()[i] & (1 << slot) != 0,
                        expected == Some(true)
                    );
                    assert_eq!(
                        sampled.supplied_bits()[i] & (1 << slot) != 0,
                        expected.is_some()
                    );
                }
            }
        }
    }
}

#[test]
fn raw_rows_cover_byte_boundaries_sparse_planes_and_binary_values() {
    let source = super::super::tests::source(35, 3);
    let frames = [(900, 2), (900, 0)];
    let values = (0..210).map(|i| (i % 201) as u8).collect::<Vec<_>>();
    let fractional = Segments::from_fractional(
        source.clone(),
        &[900],
        &frames,
        &values,
        encoding(FractionalKind::Occupancy, 200),
        None,
        || false,
    )
    .unwrap();
    let mut bits = vec![0u8; 210usize.div_ceil(8).next_multiple_of(2)];
    for i in 0..210 {
        if i % 3 == 1 {
            set(&mut bits, i);
        }
    }
    let binary = Segments::from_binary(source, &[900], &frames, &bits, None, || false).unwrap();
    assert_eq!(binary.encoding(), SegmentEncoding::Binary);
    let mut output = [255; 35];
    for masks in [&fractional, &binary] {
        let mask = masks.mask(900).unwrap();
        for z in 0..3 {
            for y in 0..3 {
                let known = mask.copy_raw_row(z, y, &mut output).unwrap();
                assert_eq!(known, z != 1);
                for (x, &value) in output.iter().enumerate() {
                    assert_eq!(mask.raw_value([x, y, z]), known.then_some(value));
                    if !known {
                        assert_eq!(value, 0);
                    }
                }
            }
        }
        for (z, y, length) in [(3, 0, 35), (0, 3, 35), (0, 0, 34), (0, 0, 0)] {
            assert!(mask.copy_raw_row(z, y, &mut output[..length]).is_err());
        }
    }
    assert!(
        fractional
            .mask(900)
            .unwrap()
            .copy_raw_row(2, 2, &mut output)
            .unwrap()
    );
    assert_eq!(output, values[70..105]);
}

#[test]
fn sparse_storage_depends_on_supplied_frames_and_odd_maxima_keep_raw_bytes() {
    let budget = MemoryBudget::new(1024 * 1024);
    let source = super::super::tests::source(3, 3);
    let numbers = (1..=64).collect::<Vec<_>>();
    let input = Input {
        numbers: &numbers,
        frames: &[(655, 1)],
        payload: &[0, 1, 2, 0, 0, 0, 0, 0, 0, 255],
        encoding: encoding(FractionalKind::Probability, 3),
        declared_overlap: None,
    };
    assert!(build(source.clone(), input, &|| false, &budget).is_err());
    let input = Input {
        numbers: &numbers,
        frames: &[(64, 1)],
        payload: &[0, 1, 2, 0, 0, 0, 0, 0, 0, 255],
        encoding: encoding(FractionalKind::Probability, 3),
        declared_overlap: None,
    };
    let masks = build(source, input, &|| false, &budget).unwrap();
    let stored = masks
        .masks
        .iter()
        .map(|mask| match &mask.samples {
            Samples::Fractional { values, .. } => values.len(),
            _ => panic!("Wrong sample representation"),
        })
        .sum::<usize>();
    assert_eq!(stored, 9); // No full grid per ID, and no derived dense bit volume.
    let selected = masks.mask(64).unwrap();
    assert_eq!(selected.raw_value([1, 0, 1]), Some(1));
    assert_eq!(selected.value([1, 0, 1]), Some(false));
    assert_eq!(selected.raw_value([2, 0, 1]), Some(2));
    assert_eq!(selected.value([2, 0, 1]), Some(true));
    assert_eq!(masks.infos().last().unwrap().positive_cells, 1);
    assert!(
        masks.masks[..63]
            .iter()
            .all(|m| m.info.supplied_planes == 0)
    );
    let (_, bytes, retained) = shape([512, 512, 128], 64, 1).unwrap();
    assert_eq!(bytes, 262144);
    assert!(retained < 512 * 1024);
    drop(masks);
    assert_eq!(budget.used(), 0);
}

#[test]
fn invalid_fractional_inputs_and_limits_cannot_leave_a_partial_set() {
    let source = source();
    let budget = MemoryBudget::new(1024 * 1024);
    for (numbers, frames, payload, maximum) in [
        (vec![1], vec![(1, 0)], vec![0; 6], 0),
        (vec![1], vec![(1, 0)], vec![201; 6], 200),
        (vec![1], vec![(1, 0)], vec![0; 5], 200),
        (vec![1], vec![(1, 0)], vec![0; 8], 200),
        (vec![1], vec![(1, 0), (1, 0)], vec![0; 12], 200),
        (vec![1], vec![(2, 0)], vec![0; 6], 200),
        (vec![1], vec![(1, 3)], vec![0; 6], 200),
        (vec![0], vec![(0, 0)], vec![0; 6], 200),
        (vec![1, 1], vec![(1, 0)], vec![0; 6], 200),
        (vec![], vec![(1, 0)], vec![0; 6], 200),
        (vec![1], vec![], vec![], 200),
    ] {
        let input = Input {
            numbers: &numbers,
            frames: &frames,
            payload: &payload,
            encoding: encoding(FractionalKind::Probability, maximum),
            declared_overlap: None,
        };
        assert!(build(source.clone(), input, &|| false, &budget).is_err());
        assert_eq!(budget.used(), 0);
    }
    for (dimensions, ids, frames) in [
        ([512, 512, 128], 1, 513),
        ([3, 2, 3], 65, 1),
        ([3, 2, 4097], 1, 1),
        ([3, 2, 3], 1, 4097),
        ([usize::MAX, 2, 3], 1, 1),
    ] {
        assert!(shape(dimensions, ids, frames).is_err());
    }
    assert!(
        Segments::from_fractional(
            source,
            &[1, 2],
            &[(1, 0), (2, 0)],
            &[200; 12],
            encoding(FractionalKind::Probability, 200),
            Some(false),
            || false
        )
        .is_err()
    );
}

#[test]
fn cancellation_and_admission_release_every_private_fractional_allocation() {
    let source = super::super::tests::source(2048, 2);
    let frames = [(1, 0), (1, 1), (1, 2)];
    let payload = vec![101; 4096 * 3];
    let budget = MemoryBudget::new(1);
    let input = || Input {
        numbers: &[1],
        frames: &frames,
        payload: &payload,
        encoding: encoding(FractionalKind::Occupancy, 200),
        declared_overlap: None,
    };
    assert!(build(source.clone(), input(), &|| false, &budget).is_err());
    assert_eq!(budget.used(), 0);
    budget.set_limit(shape(source.dimensions(), 1, 3).unwrap().2);
    assert!(build(source.clone(), input(), &|| false, &budget).is_err());
    assert_eq!(budget.used(), 0);
    budget.set_limit(1024 * 1024);
    for after in [0, 1, 2, 4, 8, 15] {
        let calls = Cell::new(0);
        let cancel = || {
            let call = calls.get();
            calls.set(call + 1);
            call >= after
        };
        assert!(matches!(
            build(source.clone(), input(), &cancel, &budget),
            Err("Cancelled")
        ));
        assert_eq!(budget.used(), 0);
    }
    let masks = build(source.clone(), input(), &|| false, &budget).unwrap();
    assert_eq!(masks.infos().next().unwrap().positive_cells, 12288);
    assert_eq!(masks.infos().next().unwrap().valid_source_cells, 12287);
    assert_eq!(source.frames()[0].pixels[0], 20.);
    drop(masks);
    assert_eq!(budget.used(), 0);
}
