// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use std::cell::Cell;

fn rational(n: i64, d: i128) -> ExactCoordinate {
    rational_coordinate(Rational::new(i128::from(n), d).unwrap()).unwrap()
}
fn contains(component: CommonZero, point: [ExactCoordinate; 2]) -> bool {
    match component {
        CommonZero::Whole => true,
        CommonZero::Curve(field) => field.sign_at(point).unwrap() == 0,
        CommonZero::Axis { axis, position } => {
            point[usize::from(axis)]
                .compare(rational_coordinate(position).unwrap())
                .unwrap()
                == Ordering::Equal
        }
    }
}
fn tensor(field: Bilinear, u: i128, v: i128, q: i128) -> i128 {
    let [a, b, c, d] = field.values.map(i128::from);
    a * (q - u) * (q - v) + b * u * (q - v) + c * u * v + d * (q - u) * v
}
fn verify(fields: [Bilinear; 2], result: &BilinearIntersections) {
    let mut previous: Option<[ExactCoordinate; 2]> = None;
    for point in result.points() {
        for field in fields {
            assert_eq!(field.sign_at(point.coordinates).unwrap(), 0);
        }
        if let [Some(u), Some(v)] = point.coordinates.map(ExactCoordinate::rational_value) {
            // Independent rational Bernstein derivatives, without an eliminant
            // or the production radical sign predicate.
            let gradients = fields.map(|field| {
                let [a, b, c, d] = field.values.map(i128::from);
                let (nu, du) = (i128::from(u.numerator), i128::from(u.denominator));
                let (nv, dv) = (i128::from(v.numerator), i128::from(v.denominator));
                [
                    (b - a) * (dv - nv) + (c - d) * nv,
                    (d - a) * (du - nu) + (c - b) * nu,
                ]
            });
            let jacobian = gradients[0][0] * gradients[1][1] - gradients[0][1] * gradients[1][0];
            let expected = if gradients.contains(&[0, 0]) {
                IntersectionContact::Singular
            } else if jacobian == 0 {
                IntersectionContact::Tangent
            } else {
                IntersectionContact::Transverse
            };
            assert_eq!(point.contact, expected);
        } else {
            // An isolated tangency has a repeated quadratic root, hence rational
            // u and v. Irrational finite events are transverse regular crossings.
            assert_eq!(point.contact, IntersectionContact::Transverse);
        }
        if let Some(before) = previous {
            let order = before[0].compare(point.coordinates[0]).unwrap();
            assert!(
                order == Ordering::Less
                    || (order == Ordering::Equal
                        && before[1].compare(point.coordinates[1]).unwrap() == Ordering::Less)
            );
        }
        previous = Some(point.coordinates);
    }
    for u in 0..=4 {
        for v in 0..=4 {
            let point = [rational(u, 4), rational(v, 4)];
            let actual = fields
                .into_iter()
                .all(|f| tensor(f, i128::from(u), i128::from(v), 4) == 0);
            let common = result.common().any(|c| contains(*c, point));
            let finite = result
                .points()
                .any(|p| same_point(p.coordinates, point).unwrap());
            assert_eq!(actual, common || finite, "{:?} {:?}", fields, point);
        }
    }
}
type Signature = (Vec<([[i128; 10]; 2], IntersectionContact)>, Vec<CommonZero>);
fn signature(result: &BilinearIntersections) -> Signature {
    (
        result
            .points()
            .map(|p| (p.coordinates.map(representation), p.contact))
            .collect(),
        result.common().copied().collect(),
    )
}
#[test]
fn all_ternary_pairs_preserve_finite_and_shared_zero_loci() {
    let fields: Vec<_> = (0..81)
        .map(|mut n| {
            Bilinear::new(std::array::from_fn(|_| {
                let v = (n % 3) as i16 - 1;
                n /= 3;
                v
            }))
            .unwrap()
        })
        .collect();
    for a in &fields {
        for b in &fields {
            let result = a
                .intersections(*b, || false)
                .unwrap_or_else(|e| panic!("{a:?} {b:?}: {e:?}"));
            verify([*a, *b], &result);
            assert_eq!(
                signature(&result),
                signature(&b.intersections(*a, || false).unwrap())
            );
            assert!(result.points().count() <= 4);
            assert!(result.common().count() <= 2);
        }
    }
}
#[test]
fn tangencies_irrational_crossings_shared_axes_and_whole_squares_are_explicit() {
    let line = Bilinear::new([0, -1, 0, 1]).unwrap();
    let tangent = Bilinear::new([1, -3, 1, 1]).unwrap();
    let result = line.intersections(tangent, || false).unwrap();
    assert_eq!(result.points().count(), 1);
    let point = result.points().next().unwrap();
    assert_eq!(point.contact(), IntersectionContact::Tangent);
    assert!(same_point(point.coordinates, [rational(1, 2); 2]).unwrap());
    let curve = Bilinear::new([2, -6, 2, -6]).unwrap();
    let result = line.intersections(curve, || false).unwrap();
    assert_eq!(result.points().count(), 2);
    for point in result.points() {
        assert_eq!(point.contact(), IntersectionContact::Transverse);
        assert!(point.coordinates[0].rational_value().is_none());
        assert_eq!(
            point.coordinates[0].compare(point.coordinates[1]),
            Ok(Ordering::Equal)
        );
    }
    let cross = Bilinear::new([1, -1, 1, -1]).unwrap();
    let axis = Bilinear::new([-1, 1, 1, -1]).unwrap();
    for other in [axis, cross, Bilinear::new([0; 4]).unwrap()] {
        let result = cross.intersections(other, || false).unwrap();
        assert_eq!(result.points().count(), 1);
        assert_eq!(
            result.points().next().unwrap().contact(),
            IntersectionContact::Singular
        );
        assert_eq!(result.common().count(), if other == axis { 1 } else { 2 });
        verify([cross, other], &result);
    }
    let whole = Bilinear::new([0; 4])
        .unwrap()
        .intersections(Bilinear::new([0; 4]).unwrap(), || false)
        .unwrap();
    assert_eq!(
        whole.common().copied().collect::<Vec<_>>(),
        vec![CommonZero::Whole]
    );
    assert_eq!(whole.points().count(), 0);
}
fn transform(point: [ExactCoordinate; 2], swap: bool, flip: u8) -> [ExactCoordinate; 2] {
    let mut result = if swap { [point[1], point[0]] } else { point };
    for (axis, value) in result.iter_mut().enumerate() {
        if flip & (1 << axis) != 0 {
            *value = ExactCoordinate::unit(
                value.parameter,
                value.denominator.subtract(value.numerator),
                value.denominator,
            )
            .unwrap()
            .unwrap();
        }
    }
    result
}
fn transform_field(field: Bilinear, swap: bool, flip: u8) -> Bilinear {
    let corners = [[0, 0], [1, 0], [1, 1], [0, 1]];
    let mut values = [0; 4];
    for (i, mut p) in corners.into_iter().enumerate() {
        if swap {
            p.swap(0, 1);
        }
        for (axis, v) in p.iter_mut().enumerate() {
            if flip & (1 << axis) != 0 {
                *v = 1 - *v;
            }
        }
        values[corners.iter().position(|q| *q == p).unwrap()] = field.values[i];
    }
    Bilinear::new(values).unwrap()
}
#[test]
fn square_symmetries_preserve_exact_contacts_and_scaled_geometry() {
    for values in [
        ([0, -1, 0, 1], [1, -3, 1, 1]),
        ([0, -1, 0, 1], [2, -6, 2, -6]),
        ([1, -1, 1, -1], [-1, 1, 1, -1]),
    ] {
        let fields = [values.0, values.1].map(|v| Bilinear::new(v).unwrap());
        let original = fields[0].intersections(fields[1], || false).unwrap();
        let scaled = fields.map(|f| Bilinear::new(f.values.map(|v| -3 * v)).unwrap());
        assert_eq!(
            signature(&original),
            signature(&scaled[1].intersections(scaled[0], || false).unwrap())
        );
        for swap in [false, true] {
            for flip in 0..4 {
                let fields = fields.map(|f| transform_field(f, swap, flip));
                let result = fields[0].intersections(fields[1], || false).unwrap();
                verify(fields, &result);
                assert_eq!(result.points().count(), original.points().count());
                assert_eq!(result.common().count(), original.common().count());
                for point in original.points() {
                    let transformed = transform(point.coordinates, swap, flip);
                    assert!(
                        result
                            .points()
                            .any(|p| same_point(p.coordinates, transformed).unwrap()
                                && p.contact == point.contact)
                    );
                }
            }
        }
    }
}
#[test]
fn extreme_and_random_corner_pairs_keep_bounds_and_deterministic_results() {
    let mut state = 431u64;
    for case in 0..2304 {
        let fields = std::array::from_fn::<_, 2, _>(|slot| {
            Bilinear::new(std::array::from_fn(|i| {
                if case < 256 {
                    if ((case >> (4 * slot + i)) & 1) == 0 {
                        -255
                    } else {
                        255
                    }
                } else {
                    state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
                    (state % 511) as i16 - 255
                }
            }))
            .unwrap()
        });
        let result = fields[0]
            .intersections(fields[1], || false)
            .unwrap_or_else(|e| panic!("{fields:?}: {e:?}"));
        verify(fields, &result);
        assert_eq!(
            signature(&result),
            signature(&fields[1].intersections(fields[0], || false).unwrap())
        );
    }
    assert!(std::mem::size_of::<BilinearIntersections>() <= 2048);
}
#[test]
fn clipped_shared_curves_keep_corner_only_contacts_without_fabricating_area() {
    let field = Bilinear::new([0, 1, 2, 1]).unwrap();
    let result = field.intersections(field, || false).unwrap();
    assert_eq!(result.points().count(), 0);
    assert_eq!(result.common().count(), 1);
    verify([field, field], &result);
    for values in [[1; 4], [-1; 4], [1, 2, 3, 2]] {
        assert_eq!(
            Bilinear::new(values)
                .unwrap()
                .intersections(Bilinear::new([0; 4]).unwrap(), || false)
                .unwrap()
                .common()
                .count(),
            0
        );
    }
}
#[test]
fn every_intersection_cancellation_returns_no_partial_result() {
    for fields in [([0, -1, 0, 1], [2, -6, 2, -6]), ([0; 4], [1, -1, 1, -1])] {
        let [a, b] = [fields.0, fields.1].map(|v| Bilinear::new(v).unwrap());
        let calls = Cell::new(0usize);
        let prior = a
            .intersections(b, || {
                calls.set(calls.get() + 1);
                false
            })
            .unwrap();
        let signature_before = signature(&prior);
        for stop in 1..=calls.get() {
            let n = Cell::new(0usize);
            assert!(matches!(
                a.intersections(b, || {
                    n.set(n.get() + 1);
                    n.get() == stop
                }),
                Err(FieldError::Cancelled)
            ));
            assert_eq!(signature(&prior), signature_before);
        }
    }
}
