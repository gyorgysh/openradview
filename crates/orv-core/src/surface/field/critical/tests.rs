// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

fn polynomial(a: i64, b: i64, c: i64) -> SectionDeterminant {
    SectionDeterminant {
        quadratic: a,
        linear: b,
        constant: c,
    }
}
fn finite_roots(p: SectionDeterminant) -> Vec<AlgebraicRoot> {
    let Roots::Finite(values) = roots(p).unwrap() else {
        panic!("Expected isolated roots")
    };
    values.into_iter().flatten().collect()
}
fn points(field: Trilinear, axis: usize) -> Vec<ProjectionPoint> {
    let ProjectionAnalysis::Finite {
        points,
        whole_sections,
    } = field.projection_critical(axis).unwrap()
    else {
        panic!("Expected isolated critical points")
    };
    assert!(whole_sections.iter().all(Option::is_none));
    points.into_iter().flatten().collect()
}
// Compare a rational threshold with the ordered roots using the polynomial's
// sign and its vertex, without evaluating any square root or production sign test.
fn reference_sign(a: i64, b: i64, c: i64, upper: bool, constant: i32, slope: i32) -> i8 {
    if slope == 0 {
        return constant.signum() as i8;
    }
    let mut n = -i128::from(constant);
    let mut d = i128::from(slope);
    if d < 0 {
        (n, d) = (-n, -d);
    }
    let relative_to_vertex = 2 * i128::from(a) * n + i128::from(b) * d;
    let value = i128::from(a) * n * n + i128::from(b) * n * d + i128::from(c) * d * d;
    let relative = if upper {
        if relative_to_vertex < 0 {
            1
        } else {
            -value.signum() as i8
        }
    } else if relative_to_vertex > 0 {
        -1
    } else {
        value.signum() as i8
    };
    relative * slope.signum() as i8
}

#[test]
fn algebraic_root_signs_match_independent_polynomial_threshold_tests() {
    let mut checked = 0;
    for a in 1..7 {
        for b in -9..10 {
            for c in -9..10 {
                let p = polynomial(a, b, c);
                let roots = finite_roots(p);
                for (index, root) in roots.iter().copied().enumerate() {
                    assert_eq!(finite_roots(polynomial(a * 7, b * 7, c * 7))[index], root);
                    assert_eq!(
                        finite_roots(polynomial(-a * 3, -b * 3, -c * 3))[index],
                        root
                    );
                    let t = root.approximate();
                    let residual = a as f64 * t * t + b as f64 * t + c as f64;
                    assert!(
                        residual.abs()
                            < 1e-12
                                * (1.
                                    + (a as f64 * t * t).abs()
                                    + (b as f64 * t).abs()
                                    + (c as f64).abs())
                    );
                    for constant in -8..9 {
                        for slope in -8..9 {
                            let expected = if let Some(value) = root.rational() {
                                (i128::from(constant) * i128::from(value.denominator())
                                    + i128::from(slope) * i128::from(value.numerator()))
                                .signum() as i8
                            } else {
                                reference_sign(a, b, c, index == 1, constant, slope)
                            };
                            assert_eq!(
                                root.sign_linear(constant, slope),
                                expected,
                                "{p:?}, root{index}, {constant}+{slope}t"
                            );
                            checked += 1;
                        }
                    }
                }
            }
        }
    }
    assert!(checked > 500000);
    assert!(matches!(roots(polynomial(0, 0, 0)).unwrap(), Roots::All));
    assert!(finite_roots(polynomial(0, 0, 7)).is_empty());
    let linear = finite_roots(polynomial(0, -7, 3));
    assert_eq!(
        (
            linear[0].rational().unwrap().numerator(),
            linear[0].rational().unwrap().denominator()
        ),
        (3, 7)
    );
    assert_eq!(finite_roots(polynomial(4, -4, 1)).len(), 1);
    assert!(roots(polynomial(i64::MIN, 0, 0)).is_err());
}

#[test]
fn near_boundary_irrational_roots_and_extreme_linear_forms_keep_exact_signs() {
    for p in [
        polynomial(1 << 20, 1 << 20, -1),
        polynomial(1 << 20, -(1 << 20), -1),
        polynomial(1, -(1 << 20), 1),
        polynomial(1 << 20, 1, -1),
    ] {
        let roots = finite_roots(p);
        assert_eq!(roots.len(), 2);
        assert!(roots.iter().all(|r| r.rational().is_none()));
        assert!(roots[0].approximate() < roots[1].approximate());
        for (index, root) in roots.into_iter().enumerate() {
            for constant in [i32::MIN, -1048576, -1, 0, 1, 1048576, i32::MAX] {
                for slope in [i32::MIN, -1048576, -1, 0, 1, 1048576, i32::MAX] {
                    assert_eq!(
                        root.sign_linear(constant, slope),
                        reference_sign(
                            p.quadratic,
                            p.linear,
                            p.constant,
                            index == 1,
                            constant,
                            slope
                        )
                    );
                }
            }
        }
    }
}

#[test]
fn near_cancelling_coordinate_forms_use_exact_conjugate_numerators() {
    let a = 1 << 20;
    let root = finite_roots(polynomial(a, a, -1))[1];
    let t = root.approximate();
    // The defining equation gives a*t-1 = -a*t*t independently of the radical.
    let expected = -(a as f64) * t * t;
    let stable = root.approximate_linear(-1, a as i32);
    assert!((stable - expected).abs() < 3e-15 * expected.abs());
    assert!(((-1. + a as f64 * t) - expected).abs() > 1e-12 * expected.abs());
    for constant in [i32::MIN, -1, 0, 1, i32::MAX] {
        for slope in [i32::MIN, -1, 0, 1, i32::MAX] {
            let value = root.approximate_linear(constant, slope);
            assert!(value.is_finite());
            let sign = if value == 0. { 0 } else { value.signum() as i8 };
            assert_eq!(sign, root.sign_linear(constant, slope));
        }
    }
    assert!(std::mem::size_of::<AlgebraicRoot>() <= 24);
    assert!(std::mem::size_of::<ProjectionAnalysis>() <= 192);
}

#[test]
fn incident_cells_keep_their_one_sided_connections_at_a_shared_singular_face() {
    let above = Trilinear::new([1, -1, -1, 1, 2, 0, 0, 2]).unwrap(); // XY + z
    let below = Trilinear::new([0, -2, -2, 0, 1, -1, -1, 1]).unwrap(); // XY + z - 1
    let folded = Trilinear::new([2, 0, 0, 2, 1, -1, -1, 1]).unwrap(); // XY + 1 - z
    let a = above.face_interior_limit(2, false).unwrap();
    let b = below.face_interior_limit(2, true).unwrap();
    let c = folded.face_interior_limit(2, true).unwrap();
    assert_eq!(a.corner_signs, b.corner_signs);
    assert_eq!(a.determinant_sign, 1);
    assert_eq!(b.determinant_sign, -1);
    assert_eq!(a, c);
    assert!(matches!(
        a.contours,
        LimitContours::Split {
            joined_corners: [0, 2],
            ..
        }
    ));
    assert!(matches!(
        b.contours,
        LimitContours::Split {
            joined_corners: [1, 3],
            ..
        }
    ));
    assert_eq!(above.face(2, false), below.face(2, true));
    assert_eq!(above.face(2, false), folded.face(2, true));
    assert_eq!(points(above, 2)[0].normal_sign(), 1);
    assert_eq!(points(folded, 2)[0].normal_sign(), -1);
}

#[test]
fn interior_limits_retain_persistent_zero_edges_and_saddle_families() {
    for values in [
        [0; 8],
        [1, -1, -1, 1, 1, -1, -1, 1],
        [0, 0, 0, 0, 1, -1, -1, 1],
        [0, 1, 0, 1, 0, 1, 0, 1],
    ] {
        assert_eq!(
            Trilinear::new(values)
                .unwrap()
                .face_interior_limit(2, false)
                .unwrap()
                .contours,
            LimitContours::Critical
        );
    }
    let field = Trilinear::new([1; 8]).unwrap();
    assert_eq!(
        field.face_interior_limit(2, false).unwrap().contours,
        LimitContours::Empty
    );
    assert!(field.face_interior_limit(3, false).is_err());
}

#[test]
fn one_sided_limits_match_independent_exact_rational_sections_for_all_ternary_fields() {
    // With integer coefficients bounded by 520200, epsilon=2^-20 makes the
    // first nonzero coefficient dominate all higher orders. This is a test-only
    // rational section, not a displacement used by the production classifier.
    const N: i128 = 1 << 20;
    for mut code in 0..3usize.pow(8) {
        let values = std::array::from_fn(|_| {
            let value = [-255, 0, 255][code % 3];
            code /= 3;
            value
        });
        let field = Trilinear::new(values).unwrap();
        for axis in 0..3 {
            for upper in [false, true] {
                let result = field.face_interior_limit(axis, upper).unwrap();
                let [u, v] = transverse_axes(axis);
                let section: [i128; 4] = std::array::from_fn(|i| {
                    let (x, y) = [(0, 0), (1, 0), (1, 1), (0, 1)][i];
                    let face = (x << u) | (y << v) | (usize::from(upper) << axis);
                    let opposite = face ^ (1 << axis);
                    i128::from(values[face]) * (N - 1) + i128::from(values[opposite])
                });
                assert_eq!(result.corner_signs, section.map(|v| v.signum() as i8));
                assert_eq!(
                    result.determinant_sign,
                    (section[0] * section[2] - section[1] * section[3]).signum() as i8
                );
                let complement = Trilinear::new(values.map(|v| -v))
                    .unwrap()
                    .face_interior_limit(axis, upper)
                    .unwrap();
                assert_eq!(complement.corner_signs, result.corner_signs.map(|v| -v));
                assert_eq!(complement.determinant_sign, result.determinant_sign);
                assert_eq!(complement.contours, result.contours);
            }
        }
    }
}

fn tensor(values: [i16; 8], point: [f64; 3], derivative: Option<usize>) -> f64 {
    values
        .into_iter()
        .enumerate()
        .map(|(i, value)| {
            let mut term = f64::from(value);
            for (a, t) in point.into_iter().enumerate() {
                term *= if derivative == Some(a) {
                    if i & (1 << a) == 0 { -1. } else { 1. }
                } else if i & (1 << a) == 0 {
                    1. - t
                } else {
                    t
                };
            }
            term
        })
        .sum()
}
fn exact_tensor(values: [i16; 8], point: [Rational; 3], derivative: Option<usize>) -> i128 {
    values
        .into_iter()
        .enumerate()
        .map(|(i, value)| {
            let mut term = i128::from(value);
            for (a, t) in point.into_iter().enumerate() {
                term *= if derivative == Some(a) {
                    if i & (1 << a) == 0 { -1 } else { 1 }
                } else if i & (1 << a) == 0 {
                    i128::from(t.denominator()) - i128::from(t.numerator())
                } else {
                    i128::from(t.numerator())
                };
            }
            term
        })
        .sum()
}
fn verify(field: Trilinear, point: ProjectionPoint) {
    let position = point
        .approximate_point()
        .expect("Generated point must be representable");
    assert!(position.into_iter().all(|v| (0. ..=1.).contains(&v)));
    assert!(
        tensor(field.values(), position, None).abs() < 1e-8,
        "{field:?} at {position:?}"
    );
    for a in 0..3 {
        let gradient = tensor(field.values(), position, Some(a));
        if a == point.axis() {
            if point.normal_sign() == 0 {
                assert!(gradient.abs() < 1e-8);
            } else {
                assert_eq!(gradient.signum() as i8, point.normal_sign());
            }
        } else {
            assert!(
                gradient.abs() < 1e-8,
                "{field:?} at {position:?}, axis{a}: {gradient}"
            );
        }
    }
    if let Some(position) = point.rational_point() {
        assert_eq!(exact_tensor(field.values(), position, None), 0);
        for a in 0..3 {
            let gradient = exact_tensor(field.values(), position, Some(a));
            assert_eq!(
                gradient.signum() as i8,
                if a == point.axis() {
                    point.normal_sign()
                } else {
                    0
                }
            );
        }
    }
}

#[test]
fn interior_saddle_points_include_two_branches_and_true_singular_contacts() {
    for (values, expected, rational) in [
        ([13, -3, -3, -3, -3, -3, -3, 13], 2, true),
        ([10, -2, -2, -2, -2, -2, -2, 10], 2, false),
        ([3, -1, -1, -1, -1, -1, -1, 3], 1, true),
    ] {
        let field = Trilinear::new(values).unwrap();
        let complement = Trilinear::new(values.map(|v| -v)).unwrap();
        for axis in 0..3 {
            let result = points(field, axis);
            assert_eq!(result.len(), expected);
            let reverse = points(complement, axis);
            for (i, point) in result.into_iter().enumerate() {
                verify(field, point);
                assert_eq!(point.rational_point().is_some(), rational);
                assert_eq!(point.parameter(), reverse[i].parameter());
                assert_eq!(point.approximate_point(), reverse[i].approximate_point());
                assert_eq!(point.normal_sign(), -reverse[i].normal_sign());
                assert_eq!(point.boundary(), 0);
                if expected == 1 {
                    assert_eq!(point.approximate_point(), Some([0.5; 3]));
                    assert_eq!(point.normal_sign(), 0);
                } else {
                    assert_eq!(point.normal_sign(), if i == 0 { 1 } else { -1 });
                }
            }
        }
    }
}

#[test]
fn projection_points_transform_with_all_cube_axes_and_reflections() {
    let permutations = [
        [0, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ];
    for values in [
        [13, -3, -3, -3, -3, -3, -3, 13],
        [10, -2, -2, -2, -2, -2, -2, 10],
        [1, -1, -1, 1, 2, 0, 0, 2],
    ] {
        let field = Trilinear::new(values).unwrap();
        for permutation in permutations {
            for flips in 0..8 {
                let transformed = Trilinear::new(std::array::from_fn(|i| {
                    let mut original = 0;
                    for (a, p) in permutation.into_iter().enumerate() {
                        original |= (((i >> a) & 1) ^ ((flips >> a) & 1)) << p;
                    }
                    values[original]
                }))
                .unwrap();
                for (new_axis, old_axis) in permutation.into_iter().enumerate() {
                    match (
                        field.projection_critical(old_axis).unwrap(),
                        transformed.projection_critical(new_axis).unwrap(),
                    ) {
                        (ProjectionAnalysis::Degenerate, ProjectionAnalysis::Degenerate) => (),
                        (
                            ProjectionAnalysis::Finite {
                                points: old,
                                whole_sections: a,
                            },
                            ProjectionAnalysis::Finite {
                                points: new,
                                whole_sections: b,
                            },
                        ) => {
                            assert!(a.iter().all(Option::is_none) && b.iter().all(Option::is_none));
                            let old: Vec<_> = old.into_iter().flatten().collect();
                            let new: Vec<_> = new.into_iter().flatten().collect();
                            assert_eq!(old.len(), new.len());
                            for point in old {
                                let position = point.approximate_point().unwrap();
                                let expected: [f64; 3] = std::array::from_fn(|a| {
                                    if flips & (1 << a) == 0 {
                                        position[permutation[a]]
                                    } else {
                                        1. - position[permutation[a]]
                                    }
                                });
                                let normal = point.normal_sign()
                                    * if flips & (1 << new_axis) == 0 { 1 } else { -1 };
                                assert!(new.iter().any(|p| {
                                    p.normal_sign() == normal
                                        && p.approximate_point()
                                            .unwrap()
                                            .into_iter()
                                            .zip(expected)
                                            .all(|(a, b)| (a - b).abs() < 1e-10)
                                }));
                            }
                            for point in new {
                                verify(transformed, point);
                            }
                        }
                        other => panic!("Projection classification changed: {other:?}"),
                    }
                }
            }
        }
    }
}

#[test]
fn face_crosses_are_distinguished_from_true_singularities_and_shared_exactly() {
    let upper = Trilinear::new([1, -1, -1, 1, 2, 0, 0, 2]).unwrap(); // XY + z
    let lower = Trilinear::new([0, -2, -2, 0, 1, -1, -1, 1]).unwrap(); // XY + z - 1
    let a = points(upper, 2)[0];
    let b = points(lower, 2)[0];
    assert_eq!(a.boundary(), 1 << 4);
    assert_eq!(b.boundary(), 1 << 5);
    assert_eq!(a.normal_sign(), 1);
    assert_eq!(b.normal_sign(), 1);
    let pa = a.rational_point().unwrap();
    let pb = b.rational_point().unwrap();
    assert_eq!(pa[..2], pb[..2]);
    assert_eq!((pa[2].numerator(), pa[2].denominator()), (0, 1));
    assert_eq!((pb[2].numerator(), pb[2].denominator()), (1, 1));
    let singular = Trilinear::new([1, -1, -1, 1, -1, -1, -1, 3]).unwrap();
    let point = points(singular, 2)[0];
    assert_eq!(point.normal_sign(), 0);
    assert_eq!(point.boundary(), 1 << 4);
    verify(singular, point);
}

#[test]
fn whole_sections_and_identically_zero_determinants_remain_explicit() {
    let plane = Trilinear::new([-1, -2, -2, -3, 1, 2, 2, 3]).unwrap();
    let ProjectionAnalysis::Finite {
        points,
        whole_sections,
    } = plane.projection_critical(2).unwrap()
    else {
        panic!("Expected one whole section")
    };
    assert!(points.iter().all(Option::is_none));
    let sections: Vec<_> = whole_sections.into_iter().flatten().collect();
    assert_eq!(sections.len(), 1);
    assert_eq!(sections[0].approximate(), 0.5);
    let extraneous = Trilinear::new([1, 1, 2, 2, 1, 2, 2, 3]).unwrap();
    assert!(points_for_empty(extraneous, 2));
    for values in [
        [1; 8],
        [0; 8],
        [1, -1, -1, 1, 1, -1, -1, 1],
        [0, 1, 0, 1, 0, 1, 0, 1],
    ] {
        assert_eq!(
            Trilinear::new(values)
                .unwrap()
                .projection_critical(2)
                .unwrap(),
            ProjectionAnalysis::Degenerate
        );
    }
    assert_eq!(plane.projection_critical(3), Err(FieldError::Bounds));
}
fn points_for_empty(field: Trilinear, axis: usize) -> bool {
    matches!(field.projection_critical(axis).unwrap(),ProjectionAnalysis::Finite{points,whole_sections} if points.iter().all(Option::is_none)&&whole_sections.iter().all(Option::is_none))
}

#[test]
fn closed_cell_bounds_include_exact_edge_contacts_and_reject_outside_stationary_points() {
    for (values, expected, mask) in [
        (
            [-1, -1, -1, 1, 1, 1, 1, 3],
            [0., 0., 0.5],
            (1 << 0) | (1 << 2),
        ),
        (
            [1, -1, -1, -1, 3, 1, 1, 1],
            [1., 1., 0.5],
            (1 << 1) | (1 << 3),
        ),
        (
            [0, -127, -2, 125, 2, -125, 0, 127],
            [1. / 127., 0.5, 0.5],
            0,
        ),
    ] {
        let field = Trilinear::new(values).unwrap();
        let result = points(field, 2);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].boundary(), mask);
        assert_eq!(result[0].approximate_point(), Some(expected));
        verify(field, result[0]);
    }
    let outside = Trilinear::new([-2, -129, 0, 127, 0, -127, 2, 129]).unwrap();
    assert!(points_for_empty(outside, 2));
}

#[test]
fn generated_sign_amplitude_and_degenerate_patterns_satisfy_independent_field_equations() {
    let binary = (0..256).flat_map(|signs| {
        (0..3).map(move |mode| {
            std::array::from_fn(move |i| {
                let magnitude = match mode {
                    0 => 1,
                    1 => (i * 37 + signs * 7) % 255 + 1,
                    _ => 255,
                };
                if signs & (1 << i) == 0 {
                    -(magnitude as i16)
                } else {
                    magnitude as i16
                }
            })
        })
    });
    let ternary = (0..3usize.pow(8)).map(|mut code| {
        std::array::from_fn(|_| {
            let value = [-255, 0, 255][code % 3];
            code /= 3;
            value
        })
    });
    let random = (1..4097u64).map(|mut state| {
        std::array::from_fn(|_| {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (state % 511) as i16 - 255
        })
    });
    let mut total = 0;
    let mut irrational = 0;
    let mut singular = 0;
    for values in binary.chain(ternary).chain(random) {
        let field = Trilinear::new(values).unwrap();
        for axis in 0..3 {
            if let ProjectionAnalysis::Finite {
                points,
                whole_sections,
            } = field.projection_critical(axis).unwrap()
            {
                for point in points.into_iter().flatten() {
                    verify(field, point);
                    total += 1;
                    irrational += usize::from(point.rational_point().is_none());
                    singular += usize::from(point.normal_sign() == 0);
                }
                for root in whole_sections.into_iter().flatten() {
                    for x in [0., 0.25, 1.] {
                        for y in [0., 0.75, 1.] {
                            let mut point = [0.; 3];
                            point[axis] = root.approximate();
                            let [u, v] = transverse_axes(axis);
                            point[u] = x;
                            point[v] = y;
                            assert!(tensor(values, point, None).abs() < 1e-8);
                        }
                    }
                }
            }
        }
    }
    assert!(
        total > 1000 && irrational > 100 && singular > 100,
        "{total}/{irrational}/{singular}"
    );
}
