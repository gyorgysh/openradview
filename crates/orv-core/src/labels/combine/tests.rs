// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use crate::{
    segments::Segments,
    study::{Geometry, demo_study},
    volume::Volume,
};
use std::cell::Cell;

fn fixture() -> (Arc<Labels>, MaskOwner) {
    let template = demo_study().series.remove(0).frames.remove(0);
    let frames = (0..2)
        .map(|z| {
            let mut frame = (*template).clone();
            frame.width = 4;
            frame.height = 2;
            frame.pixels = vec![1.; 8].into();
            let mut valid = vec![1; 8];
            if z == 0 {
                valid[2] = 0;
            }
            frame.validity = Some(valid.into());
            frame.geometry = Some(Geometry {
                position: [z as f64 * 0.25, 0., -(z as f64) * 2.],
                orientation: [1., 0., 0., 0., -1., 0.],
                spacing: [2., 1.],
                frame_of_reference: Some("generated-combine".into()),
                nifti_transform: None,
            });
            Arc::new(frame)
        })
        .collect();
    let source = Arc::new(Volume::new(frames).unwrap());
    let mut labels = Labels::empty(source.clone()).unwrap();
    for (i, v) in [1, 1, 1, 0, 9, 0, 1, 0, 1, 0, 0, 0, 0, 0, 0, 0]
        .into_iter()
        .enumerate()
    {
        labels.set_value(i, v).unwrap();
    }
    let segments = Segments::from_binary(
        source,
        &[40000],
        &[(40000, 0)],
        &[0x1d, 0],
        Some(true),
        || false,
    )
    .unwrap();
    (Arc::new(labels), MaskOwner::Supplied(Arc::new(segments)))
}
fn request(operation: CombineOperation) -> CombineRequest {
    CombineRequest {
        scope: LabelScope::Volume,
        target: 1,
        operand: 40000,
        operation,
    }
}
#[test]
fn operations_preserve_unknown_missing_and_competing_labels() {
    for (operation, adds, removes, protected, first_plane) in [
        (CombineOperation::Union, 1, 0, 1, [1, 1, 1, 1, 9, 0, 1, 0]),
        (
            CombineOperation::Intersection,
            0,
            2,
            0,
            [1, 0, 1, 0, 9, 0, 0, 0],
        ),
        (
            CombineOperation::Subtract,
            0,
            1,
            0,
            [0, 1, 1, 0, 9, 0, 1, 0],
        ),
        (CombineOperation::Copy, 1, 2, 1, [1, 0, 1, 1, 9, 0, 0, 0]),
    ] {
        let (base, operand) = fixture();
        let preview = base
            .combine(operand.clone(), request(operation), || false)
            .unwrap();
        assert_eq!(
            preview.summary(),
            CombineSummary {
                added: adds,
                removed: removes,
                protected,
                unknown: 8,
                missing_source: 1
            }
        );
        let result = preview.apply(&base, &operand, || false).unwrap();
        assert_eq!(
            result.values().iter().take(8).copied().collect::<Vec<_>>(),
            first_plane
        );
        assert_eq!(
            result.values().iter().skip(8).copied().collect::<Vec<_>>(),
            base.values().iter().skip(8).copied().collect::<Vec<_>>()
        );
        assert_eq!(base.value([3, 0, 0]), Some(0));
        assert!(Arc::ptr_eq(preview.base(), &base));
    }
}
#[test]
fn slice_scope_and_manual_operand_remain_bound_to_both_revisions() {
    let (base, supplied) = fixture();
    let mut req = request(CombineOperation::Copy);
    req.scope = LabelScope::Slice(1);
    let preview = base.combine(supplied.clone(), req, || false).unwrap();
    assert_eq!(preview.summary().unknown, 8);
    let result = preview.apply(&base, &supplied, || false).unwrap();
    assert_eq!(result.revision(), base.revision());
    let mut edited = base.try_clone(|| false).unwrap();
    edited.begin_edit();
    edited.set_value(3, 7).unwrap();
    let changed = Arc::new(edited);
    assert!(preview.apply(&changed, &supplied, || false).is_err());
    let other_source = fixture().1;
    assert!(preview.apply(&base, &other_source, || false).is_err());
    let manual = MaskOwner::Manual(base.clone());
    let mut req = request(CombineOperation::Union);
    req.operand = 1;
    let self_preview = base.combine(manual.clone(), req, || false).unwrap();
    assert_eq!(self_preview.summary().added, 0);
    let changed_operand = MaskOwner::Manual(changed);
    assert!(
        self_preview
            .apply(&base, &changed_operand, || false)
            .is_err()
    );
}
#[test]
fn errors_and_cancellation_preserve_input_and_release_preview_reservation() {
    let (base, operand) = fixture();
    let baseline = base.values().iter().copied().collect::<Vec<_>>();
    let budget = MemoryBudget::new(0);
    assert!(
        base.combine_with_budget(
            operand.clone(),
            request(CombineOperation::Copy),
            &budget,
            &|| false
        )
        .is_err()
    );
    let budget = MemoryBudget::new(4096);
    let calls = Cell::new(0);
    assert!(
        base.combine_with_budget(
            operand.clone(),
            request(CombineOperation::Copy),
            &budget,
            &|| {
                calls.set(calls.get() + 1);
                calls.get() >= 2
            }
        )
        .is_err()
    );
    assert_eq!(budget.used(), 0);
    let preview = base
        .combine_with_budget(
            operand.clone(),
            request(CombineOperation::Copy),
            &budget,
            &|| false,
        )
        .unwrap();
    assert!(budget.used() > 0);
    assert!(preview.apply(&base, &operand, || true).is_err());
    let apply_calls = Cell::new(0);
    assert!(
        preview
            .apply(&base, &operand, || {
                apply_calls.set(apply_calls.get() + 1);
                apply_calls.get() >= 4
            })
            .is_err()
    );
    drop(preview);
    assert_eq!(budget.used(), 0);
    assert!(
        base.combine_with_budget(
            operand.clone(),
            request(CombineOperation::Copy),
            &budget,
            &|| false
        )
        .is_ok()
    );
    assert_eq!(base.values().iter().copied().collect::<Vec<_>>(), baseline);
    let mut bad = request(CombineOperation::Copy);
    bad.target = 0;
    assert!(base.combine(operand.clone(), bad, || false).is_err());
    bad.target = 1;
    bad.operand = 0;
    assert!(base.combine(operand.clone(), bad, || false).is_err());
    bad.operand = 40000;
    bad.scope = LabelScope::Slice(2);
    assert!(base.combine(operand, bad, || false).is_err());
}
