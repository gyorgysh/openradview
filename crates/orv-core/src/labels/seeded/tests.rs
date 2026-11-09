// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use crate::{labels::tests::source, volume::Volume};
use std::cell::Cell;

fn map(row: [f32; 8], missing: &[usize]) -> Labels {
    let source = source();
    let mut frames = source.frames().to_vec();
    let mut first = (*frames[0]).clone();
    let mut pixels = first.pixels.to_vec();
    pixels[..8].copy_from_slice(&row);
    let mut validity = vec![1; pixels.len()];
    for i in missing {
        validity[*i] = 0;
    }
    first.pixels = pixels.into();
    first.validity = Some(validity.into());
    frames[0] = Arc::new(first);
    Labels::empty(Arc::new(Volume::new(frames).unwrap())).unwrap()
}
fn request() -> SeededRequest {
    SeededRequest {
        slice: 0,
        origin: [0, 0],
        size: [7, 1],
        label: 7,
        max_iterations: 50,
    }
}
fn seeds() -> [SeedPoint; 2] {
    [
        SeedPoint {
            position: [0, 0],
            class: SeedClass::Include,
        },
        SeedPoint {
            position: [6, 0],
            class: SeedClass::Exclude,
        },
    ]
}
#[test]
fn flat_data_uses_fixed_seeds_four_neighbors_and_deterministic_exclude_ties() {
    use SeedClass::{Exclude as E, Include as I};
    let base = Arc::new(map([0.; 8], &[]));
    let preview = base.seeded_region(request(), &seeds(), || false).unwrap();
    assert!(Arc::ptr_eq(preview.base(), &base));
    assert_eq!(preview.request(), request());
    assert_eq!(
        preview.classes().collect::<Vec<_>>(),
        [
            Some(I),
            Some(I),
            Some(I),
            Some(E),
            Some(E),
            Some(E),
            Some(E)
        ]
    );
    assert_eq!(
        preview.seeds().collect::<Vec<_>>(),
        [Some(I), None, None, None, None, None, Some(E)]
    );
    assert_eq!(preview.summary().included, 3);
    assert_eq!(preview.summary().excluded, 4);
    assert_eq!(preview.summary().iterations, 4);
    assert_eq!(preview.summary().added, 3);
    assert_eq!(preview.summary().removed, 0);
    let updated = preview.apply(|| false).unwrap();
    assert_eq!(
        updated.values().iter().take(8).copied().collect::<Vec<_>>(),
        [7, 7, 7, 0, 0, 0, 0, 0]
    );
    assert!(base.values().iter().all(|v| *v == 0));
    assert_ne!(updated.revision(), base.revision());
    assert_eq!(updated.parent_revision(), Some(base.revision()));
}
#[test]
fn strong_boundaries_preserve_the_nearest_intensity_class() {
    use SeedClass::{Exclude as E, Include as I};
    let base = Arc::new(map([0., 0., 1., 10., 10., 10., 10., 0.], &[]));
    let preview = base.seeded_region(request(), &seeds(), || false).unwrap();
    assert_eq!(
        preview.classes().collect::<Vec<_>>(),
        [
            Some(I),
            Some(I),
            Some(I),
            Some(E),
            Some(E),
            Some(E),
            Some(E)
        ]
    );
    let reverse = [seeds()[1], seeds()[0]];
    let other = base.seeded_region(request(), &reverse, || false).unwrap();
    assert_eq!(
        preview.classes().collect::<Vec<_>>(),
        other.classes().collect::<Vec<_>>()
    );
}
#[test]
fn missing_and_other_labels_block_propagation_and_unreached_cells_stay_unchanged() {
    use SeedClass::{Exclude as E, Include as I};
    let mut labels = map([0.; 8], &[2]);
    for i in [1, 2, 3, 5] {
        labels.set_value(i, 7).unwrap();
    }
    labels.set_value(4, 9).unwrap();
    let base = Arc::new(labels);
    let preview = base.seeded_region(request(), &seeds(), || false).unwrap();
    assert_eq!(
        preview.classes().collect::<Vec<_>>(),
        [Some(I), Some(I), None, None, None, Some(E), Some(E)]
    );
    assert_eq!(preview.summary().added, 1);
    assert_eq!(preview.summary().removed, 1);
    assert_eq!(preview.summary().unresolved, 3);
    let result = preview.apply(|| false).unwrap();
    assert_eq!(
        result.values().iter().take(8).copied().collect::<Vec<_>>(),
        [7, 7, 7, 7, 9, 0, 0, 0]
    );
    assert_eq!(
        base.values().iter().take(8).copied().collect::<Vec<_>>(),
        [0, 7, 7, 7, 9, 7, 0, 0]
    );
}

#[test]
fn cropped_flat_plane_follows_manhattan_distance_and_preserves_outside_cells() {
    let mut labels = map([0.; 8], &[]);
    let mut frames = labels.source.frames().to_vec();
    let mut frame = (*frames[0]).clone();
    frame.pixels = vec![0.; 48].into();
    frames[0] = Arc::new(frame);
    labels.source = Arc::new(Volume::new(frames).unwrap());
    labels.set_value(0, 9).unwrap();
    let base = Arc::new(labels);
    let req = SeededRequest {
        origin: [1, 1],
        size: [5, 3],
        ..request()
    };
    let seeds = [
        SeedPoint {
            position: [1, 1],
            class: SeedClass::Include,
        },
        SeedPoint {
            position: [5, 3],
            class: SeedClass::Exclude,
        },
    ];
    let preview = base.seeded_region(req, &seeds, || false).unwrap();
    let result = preview.apply(|| false).unwrap();
    for y in 0..6 {
        for x in 0..8 {
            let expected =
                if (1..=5).contains(&x) && (1..=3).contains(&y) && x - 1 + y - 1 < 5 - x + 3 - y {
                    7
                } else if x == 0 && y == 0 {
                    9
                } else {
                    0
                };
            assert_eq!(result.value([x, y, 0]), Some(expected), "{x},{y}");
        }
    }
}
#[test]
fn seed_scope_conflicts_iteration_caps_and_stale_results_are_explicit() {
    let base = Arc::new(map([0.; 8], &[3]));
    let bad = [
        vec![],
        vec![seeds()[0]],
        vec![
            seeds()[0],
            SeedPoint {
                position: [0, 0],
                class: SeedClass::Exclude,
            },
        ],
        vec![
            seeds()[0],
            SeedPoint {
                position: [3, 0],
                class: SeedClass::Exclude,
            },
        ],
        vec![
            seeds()[0],
            SeedPoint {
                position: [7, 0],
                class: SeedClass::Exclude,
            },
        ],
    ];
    for s in bad {
        assert!(base.seeded_region(request(), &s, || false).is_err());
    }
    for req in [
        SeededRequest {
            size: [0, 1],
            ..request()
        },
        SeededRequest {
            origin: [usize::MAX, 0],
            ..request()
        },
        SeededRequest {
            slice: 3,
            ..request()
        },
        SeededRequest {
            label: 0,
            ..request()
        },
        SeededRequest {
            max_iterations: 0,
            ..request()
        },
        SeededRequest {
            max_iterations: MAX_ITERATIONS + 1,
            ..request()
        },
    ] {
        assert!(base.seeded_region(req, &seeds(), || false).is_err());
    }
    assert!(matches!(
        base.seeded_region(
            SeededRequest {
                max_iterations: 1,
                ..request()
            },
            &seeds(),
            || false
        ),
        Err("Seeded region did not converge within its iteration limit")
    ));
    let result = base.seeded_region(request(), &seeds(), || false).unwrap();
    let mut edited = base.try_clone(|| false).unwrap();
    edited.set_value(7, 4).unwrap();
    assert_ne!(result.base().revision(), edited.revision());
    assert!(result.apply(|| true).is_err());
    assert_eq!(base.value([7, 0, 0]), Some(0));
}
#[test]
fn cancellation_and_admission_release_scratch_and_keep_the_prior_map() {
    let base = Arc::new(map([0.; 8], &[]));
    let budget = MemoryBudget::new(1024 * 1024);
    let calls = Cell::new(0usize);
    let preview = base
        .seeded_with_budget(
            request(),
            &seeds(),
            &|| {
                calls.set(calls.get() + 1);
                false
            },
            &budget,
        )
        .unwrap();
    let count = calls.get();
    let used = budget.used();
    let owners = Arc::strong_count(&base);
    for stop in 1..=count {
        calls.set(0);
        assert!(matches!(
            base.seeded_with_budget(
                request(),
                &seeds(),
                &|| {
                    calls.set(calls.get() + 1);
                    calls.get() == stop
                },
                &budget
            ),
            Err("Cancelled")
        ));
        assert_eq!(budget.used(), used);
        assert_eq!(Arc::strong_count(&base), owners);
    }
    calls.set(0);
    drop(
        preview
            .apply(|| {
                calls.set(calls.get() + 1);
                false
            })
            .unwrap(),
    );
    let apply_calls = calls.get();
    for stop in 1..=apply_calls {
        calls.set(0);
        assert!(matches!(
            preview.apply(|| {
                calls.set(calls.get() + 1);
                calls.get() == stop
            }),
            Err("Cancelled")
        ));
        assert!(base.values().iter().all(|v| *v == 0));
    }
    // Retained-output admission succeeds, then scratch admission must fail.
    budget.set_limit(used * 2);
    assert!(matches!(
        base.seeded_with_budget(request(), &seeds(), &|| false, &budget),
        Err("Not enough memory for seeded region")
    ));
    assert_eq!(budget.used(), used);
    budget.set_limit(used);
    assert!(matches!(
        base.seeded_with_budget(request(), &seeds(), &|| false, &budget),
        Err("Not enough memory for seeded region")
    ));
    assert_eq!(budget.used(), used);
    assert_eq!(preview.summary().added, 3);
    drop(preview);
    assert_eq!(budget.used(), 0);
}
