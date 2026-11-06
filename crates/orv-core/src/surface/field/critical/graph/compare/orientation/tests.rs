// SPDX-License-Identifier: GPL-3.0-or-later
use super::super::super::tests::{coord, positive_root, rational};
use super::*;
use std::cell::Cell;

fn root(n: i64) -> ExactCoordinate {
    coord(positive_root(n, 0, -1), [0, 1], [1, 0])
}

#[test]
fn large_coordinate_forms_keep_every_radical_dispatch_and_turn_orientation() {
    let budget = MemoryBudget::new(2 * 1024 * 1024);
    let m = MAX_FORM as i32;
    for count in 1..=6 {
        let mut points = [[rational(0, 1); 2]; 3];
        for (i, value) in points.iter_mut().flatten().enumerate() {
            let field = i % count;
            let root = positive_root(
                (1 << 20) - 1 - field as i64,
                (1 << 20) - 3,
                -((1 << 20) - 5),
            );
            *value = coord(
                root,
                [m / (i as i32 + 2), m / (i as i32 + 3)],
                [m - 1, m - 3],
            );
        }
        let expected =
            independent(points).expect("Independent dyadic enclosures separate the turn");
        let [a, b, c] = points;
        for (points, sign) in [([a, b, c], 1), ([a, c, b], -1), ([b, c, a], 1)] {
            assert_eq!(orientation(points, &budget, &|| false), Ok(sign * expected));
            assert_eq!(budget.used(), 0);
        }
    }
}
fn independent(points: [[ExactCoordinate; 2]; 3]) -> Option<i8> {
    let mut values = [[[0i128; 2]; 2]; 3];
    let grid = 1i128 << 24;
    for (output, point) in values.iter_mut().zip(points) {
        for (output, coordinate) in output.iter_mut().zip(point) {
            let bounds = super::super::tests::coordinate_bounds(coordinate, 32)?;
            output[0] = bounds[0].0 * grid / bounds[0].1;
            output[1] = (bounds[1].0 * grid + bounds[1].1 - 1) / bounds[1].1;
        }
    }
    let [a, b, c] = values;
    let difference = |a: [[i128; 2]; 2], b: [[i128; 2]; 2], axis: usize| {
        [a[axis][0] - b[axis][1], a[axis][1] - b[axis][0]]
    };
    let product = |a: [i128; 2], b: [i128; 2]| -> Option<[i128; 2]> {
        let v = [a[0] * b[0], a[0] * b[1], a[1] * b[0], a[1] * b[1]];
        Some([*v.iter().min()?, *v.iter().max()?])
    };
    let p: [i128; 2] = product(difference(b, a, 0), difference(c, a, 1))?;
    let q: [i128; 2] = product(difference(b, a, 1), difference(c, a, 0))?;
    if p[0] > q[1] {
        Some(1)
    } else if p[1] < q[0] {
        Some(-1)
    } else {
        None
    }
}
#[test]
fn exact_turns_match_independent_polynomial_enclosures_and_permutations() {
    let budget = MemoryBudget::new(2 * 1024 * 1024);
    let coords = [
        root(2),
        root(3),
        root(5),
        root(7),
        root(11),
        root(13),
        rational(0, 1),
        rational(1, 1),
    ];
    let mut state = 291u64;
    let mut checked_count = 0;
    for _ in 0..96 {
        let mut points = [[rational(0, 1); 2]; 3];
        for coordinate in points.iter_mut().flatten() {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            *coordinate = coords[(state >> 32) as usize % coords.len()];
        }
        let Some(expected) = independent(points) else {
            continue;
        };
        let [a, b, c] = points;
        for (points, sign) in [
            ([a, b, c], 1),
            ([b, c, a], 1),
            ([c, a, b], 1),
            ([b, a, c], -1),
            ([a, c, b], -1),
            ([c, b, a], -1),
        ] {
            assert_eq!(
                orientation(points, &budget, &|| false).unwrap(),
                expected * sign
            );
        }
        checked_count += 1;
        assert_eq!(budget.used(), 0);
    }
    assert!(checked_count > 65);
    let six = [[root(2), root(3)], [root(5), root(7)], [root(11), root(13)]];
    assert_eq!(
        orientation(six, &budget, &|| false).unwrap(),
        independent(six).unwrap()
    );
}
#[test]
fn dependent_radicals_and_equal_rounded_coordinates_keep_exact_collinearity() {
    let budget = MemoryBudget::new(2 * 1024 * 1024);
    let zero = [rational(0, 1); 2];
    // (1/sqrt2)*(1/sqrt3)=1/sqrt6, despite three different discriminants.
    assert_eq!(
        orientation(
            [zero, [root(2), root(6)], [rational(1, 1), root(3)]],
            &budget,
            &|| false
        ),
        Ok(0)
    );
    let n = (1 << 20) - 3;
    let x = coord(positive_root(n, n, -1), [1, 1], [2, 0]);
    let y = coord(positive_root(n + 2, n, -1), [1, 1], [2, 0]);
    assert_eq!(x.approximate(), y.approximate());
    assert_eq!(
        orientation([zero, [rational(1, 1); 2], [x, y]], &budget, &|| false),
        Ok(-1)
    );
    let large = rational(i64::MAX, i128::from(i64::MAX) + 1);
    assert_eq!(
        orientation([zero, [large, large], [large, large]], &budget, &|| false),
        Ok(0)
    );
    assert_eq!(budget.used(), 0);
}
#[test]
fn exact_predicate_admission_and_every_cancellation_release_scratch() {
    let points = [
        [rational(0, 1); 2],
        [root(2), root(6)],
        [rational(1, 1), root(3)],
    ];
    let budget = MemoryBudget::new(2 * 1024 * 1024);
    let calls = Cell::new(0usize);
    assert_eq!(
        orientation(points, &budget, &|| {
            calls.set(calls.get() + 1);
            false
        }),
        Ok(0)
    );
    let count = calls.get();
    for stop in 1..=count {
        calls.set(0);
        assert_eq!(
            orientation(points, &budget, &|| {
                calls.set(calls.get() + 1);
                calls.get() == stop
            }),
            Err(SurfaceError::Cancelled)
        );
        assert_eq!(budget.used(), 0);
    }
    assert_eq!(
        orientation(points, &MemoryBudget::new(1), &|| false),
        Err(SurfaceError::Memory)
    );
}
