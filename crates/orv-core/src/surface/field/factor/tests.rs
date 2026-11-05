// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use std::cell::Cell;

fn tensor(values: [i16; 8], p: [i128; 3], q: i128) -> i128 {
    values
        .into_iter()
        .enumerate()
        .map(|(i, v)| {
            i128::from(v)
                * (0..3)
                    .map(|a| if i & (1 << a) == 0 { q - p[a] } else { p[a] })
                    .product::<i128>()
        })
        .sum()
}
fn factor_at(axis: usize, values: [i16; 2]) -> AxisFactor {
    let field = Trilinear::new(std::array::from_fn(|i| values[(i >> axis) & 1])).unwrap();
    let result = field.factorization(|| false).unwrap();
    assert_eq!(result.factors().count(), 1);
    *result.factors().next().unwrap()
}
fn verify(result: CellFactorization) {
    assert_eq!(
        result.reconstruct_values().unwrap(),
        result.original.values()
    );
    let mut factor_axes = 0;
    for factor in result.factors() {
        assert_eq!(factor_axes & (1 << factor.axis()), 0);
        factor_axes |= 1 << factor.axis();
        let [a, b] = factor.values();
        assert_ne!(a, b);
        assert!(if a != 0 { a > 0 } else { b > 0 });
        let (mut p, mut q) = (a.unsigned_abs(), b.unsigned_abs());
        while q != 0 {
            (p, q) = (q, p % q);
        }
        assert_eq!(p, 1);
        if let Some(t) = factor.zero().unwrap() {
            let n = i128::from(t.numerator());
            let d = i128::from(t.denominator());
            assert!((0..=d).contains(&n));
            assert_eq!(i128::from(a) * (d - n) + i128::from(b) * n, 0);
        } else {
            assert!(a > 0 && b > 0);
        }
    }
    let residual = result.residual();
    assert_eq!(factor_axes & residual.dependent_axes(), 0);
    assert!(
        residual
            .field()
            .values()
            .into_iter()
            .all(|v| (-255..=255).contains(&v))
    );
    match residual.kind() {
        ResidualKind::Whole => {
            assert_eq!(result.original.values(), [0; 8]);
            assert_eq!(factor_axes, 0);
        }
        ResidualKind::Constant => assert!(
            residual
                .field()
                .values()
                .into_iter()
                .all(|v| v == residual.field().values()[0] && v != 0)
        ),
        ResidualKind::Bilinear => {
            let (axes, field) = residual.bilinear().unwrap();
            assert!(axes[0] < axes[1]);
            assert_eq!(residual.dependent_axes(), (1 << axes[0]) | (1 << axes[1]));
            let [a, b, c, d] = field.values().map(i32::from);
            assert_ne!(a * c - b * d, 0);
        }
        ResidualKind::Trilinear => {
            assert_eq!(residual.dependent_axes(), 7);
            assert_eq!(factor_axes, 0);
            assert!(
                !residual
                    .field()
                    .critical_graph(|| false)
                    .unwrap()
                    .requires_family_resolution()
            );
        }
    }
    for p in [[1, 1, 1], [1, 2, 3], [3, 1, 2], [3, 3, 3]] {
        let original =
            tensor(result.original.values(), p, 4) * 4i128.pow(result.factors().count() as u32);
        let product = result
            .factors()
            .map(|f| {
                let [a, b] = f.values();
                i128::from(a) * (4 - p[f.axis()]) + i128::from(b) * p[f.axis()]
            })
            .product::<i128>();
        assert_eq!(original, tensor(residual.field().values(), p, 4) * product);
    }
}

#[test]
fn inclusive_cell_phases_and_boundary_strata_match_exact_bernstein_support() {
    for mut code in 0..6561 {
        let values = std::array::from_fn(|_| {
            let v = code % 3;
            code /= 3;
            v as i16 - 1
        });
        let field = Trilinear::new(values).unwrap();
        let phase = field.phase();
        let boundary = field.zero_boundary();
        assert_eq!(
            phase,
            if values.iter().all(|v| *v >= 0) {
                CellPhase::Selected
            } else if values.iter().all(|v| *v < 0) {
                CellPhase::Unselected
            } else if values.iter().all(|v| *v <= 0) {
                CellPhase::BoundaryOnly
            } else {
                CellPhase::Crossing
            }
        );
        for x in 0..=4 {
            for y in 0..=4 {
                for z in 0..=4 {
                    let p = [x, y, z];
                    let value = tensor(values, p, 4);
                    match phase {
                        CellPhase::Selected => assert!(value >= 0),
                        CellPhase::Unselected => assert!(value < 0),
                        CellPhase::BoundaryOnly => {
                            if p.into_iter().all(|v| v > 0 && v < 4) {
                                assert!(value < 0);
                            }
                            let vertex = (0..8).any(|i| {
                                boundary.vertices & (1 << i) != 0
                                    && (0..3).all(|a| p[a] == if i & (1 << a) == 0 { 0 } else { 4 })
                            });
                            let edge = (0..3).any(|axis| {
                                let [u, v] = transverse(axis);
                                (0..4).any(|i| {
                                    let expected_u = if i & 1 == 0 { 0 } else { 4 };
                                    let expected_v = if i & 2 == 0 { 0 } else { 4 };
                                    boundary.edges & (1 << (axis * 4 + i)) != 0
                                        && p[u] == expected_u
                                        && p[v] == expected_v
                                })
                            });
                            let face = (0..3).any(|axis| {
                                (0..2).any(|upper| {
                                    boundary.faces & (1 << (axis * 2 + upper)) != 0
                                        && p[axis] == 4 * upper as i128
                                })
                            });
                            assert_eq!(value == 0, vertex || edge || face);
                        }
                        CellPhase::Crossing => {}
                    }
                }
            }
        }
        if phase == CellPhase::Crossing {
            for sign in [-1, 1] {
                let corner = values.iter().position(|v| *v == sign).unwrap();
                let p = std::array::from_fn(|a| if corner & (1 << a) == 0 { 1 } else { 1023 });
                assert_eq!(tensor(values, p, 1024).signum(), i128::from(sign));
            }
        }
    }
}

#[test]
fn factorization_separates_constants_planes_extrusions_and_three_axis_products() {
    let fixtures = [
        ([0; 8], ResidualKind::Whole, 0),
        ([17; 8], ResidualKind::Constant, 0),
        ([-2, 4, -2, 4, -2, 4, -2, 4], ResidualKind::Constant, 1),
        ([1, -1, -1, 1, -1, 1, 1, -1], ResidualKind::Constant, 3),
        ([1, 2, -1, -2, 1, 2, 1, 2], ResidualKind::Bilinear, 1),
        ([1, 1, -1, -1, 1, 1, 1, 1], ResidualKind::Bilinear, 0),
        ([-1, 0, 0, 1, 0, 1, 1, 2], ResidualKind::Trilinear, 0),
        ([3, -1, -1, -1, -1, -1, -1, 3], ResidualKind::Trilinear, 0),
    ];
    for (values, kind, factors) in fixtures {
        let result = Trilinear::new(values)
            .unwrap()
            .factorization(|| false)
            .unwrap();
        assert_eq!(result.residual().kind(), kind, "{values:?}");
        assert_eq!(result.factors().count(), factors);
        verify(result);
    }
    let factored = Trilinear::new([1, 2, -1, -2, 1, 2, 1, 2])
        .unwrap()
        .factorization(|| false)
        .unwrap();
    assert_eq!(factored.factors().next().unwrap().values(), [1, 2]);
    assert_eq!(factored.factors().next().unwrap().zero().unwrap(), None);
    assert_eq!(factored.residual().bilinear().unwrap().0, [1, 2]);
}

#[test]
fn generated_factors_reconstruct_exactly_and_remove_all_elimination_families() {
    let mut fixtures = Vec::new();
    for mut code in 0..6561 {
        fixtures.push(std::array::from_fn(|_| {
            let v = code % 3;
            code /= 3;
            v as i16 - 1
        }));
    }
    for signs in 0..256 {
        for mode in 0..3 {
            fixtures.push(std::array::from_fn(|i| {
                let magnitude = match mode {
                    0 => 1,
                    1 => ((i * 17 + signs * 3) % 255 + 1) as i16,
                    _ => 255,
                };
                if signs & (1 << i) == 0 {
                    -magnitude
                } else {
                    magnitude
                }
            }));
        }
    }
    let mut state = 31u64;
    for _ in 0..2048 {
        fixtures.push(std::array::from_fn(|_| {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (state % 511) as i16 - 255
        }));
    }
    for values in fixtures {
        let field = Trilinear::new(values).unwrap();
        let result = field.factorization(|| false).unwrap();
        verify(result);
        for axis in 0..3 {
            if matches!(
                field.projection_critical(axis).unwrap(),
                ProjectionAnalysis::Degenerate
            ) {
                assert_ne!(
                    result.residual().kind(),
                    ResidualKind::Trilinear,
                    "{values:?}"
                );
            }
        }
    }
}

#[test]
fn factor_planes_and_exact_signs_handle_rational_boundaries_and_irrational_points() {
    let field = Trilinear::new([1, 1, 1, 5, 1, -1, -3, -1]).unwrap();
    let ProjectionAnalysis::Finite { points, .. } = field.projection_critical(2).unwrap() else {
        panic!("Finite root required");
    };
    let irrational = points
        .into_iter()
        .flatten()
        .next()
        .unwrap()
        .coordinate(2)
        .unwrap();
    assert!(irrational.rational_value().is_none());
    for (values, expected) in [
        ([2, -1], -1),
        ([3, -1], 1),
        ([1, 2], 1),
        ([0, 1], 1),
        ([1, 0], 1),
    ] {
        let factor = factor_at(0, values);
        assert_eq!(factor.sign_at(irrational), Ok(expected));
        if let Some(t) = factor.zero().unwrap() {
            let at = ExactCoordinate::from_rational(t).unwrap().unwrap();
            assert_eq!(factor.sign_at(at), Ok(0));
        }
    }
    let zero = Trilinear::new([0, 1, 0, 1, 0, 1, 0, 1])
        .unwrap()
        .edge(0, 0)
        .unwrap();
    let one = Trilinear::new([1, 0, 1, 0, 1, 0, 1, 0])
        .unwrap()
        .edge(0, 0)
        .unwrap();
    let (EdgeZero::Point(zero), EdgeZero::Point(one)) = (zero, one) else {
        panic!("Boundary points required");
    };
    let zero = ExactCoordinate::from_rational(zero).unwrap().unwrap();
    let one = ExactCoordinate::from_rational(one).unwrap().unwrap();
    assert_eq!(factor_at(0, [0, 1]).sign_at(zero), Ok(0));
    assert_eq!(factor_at(0, [1, 0]).sign_at(one), Ok(0));
    assert_eq!(factor_at(0, [0, 1]).sign_at(one), Ok(1));
    assert_eq!(factor_at(0, [1, 0]).sign_at(zero), Ok(1));
}

#[test]
fn nonunit_factor_ratios_keep_integer_quotients_and_exact_source_amplitudes() {
    let mut residuals = Vec::new();
    for mut code in 0..81 {
        residuals.push(std::array::from_fn(|_| {
            let v = code % 3;
            code /= 3;
            7 * (v as i16 - 1)
        }));
    }
    residuals.push([1, -3, 5, 7]);
    for axis in 0..3 {
        for a in -4i16..=4 {
            for b in -4i16..=4 {
                for quotient in &residuals {
                    let mut values = [0; 8];
                    for (i, value) in face_indices(axis, false)
                        .unwrap()
                        .into_iter()
                        .zip(*quotient)
                    {
                        values[i] = a * value;
                        values[i | (1 << axis)] = b * value;
                    }
                    let result = Trilinear::new(values)
                        .unwrap()
                        .factorization(|| false)
                        .unwrap();
                    verify(result);
                    let factor = result.factors().find(|f| f.axis() == axis);
                    if a == b || *quotient == [0; 4] {
                        assert!(factor.is_none());
                    } else {
                        let [left, right] = factor.unwrap().values();
                        assert_eq!(
                            i32::from(left) * i32::from(b),
                            i32::from(right) * i32::from(a)
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn factorization_is_canonical_under_axis_reflections_and_field_complement() {
    let permutations = [
        [0, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ];
    for values in [
        [1, -1, -1, 1, -1, 1, 1, -1],
        [0, 0, -2, 0, 0, 0, 2, 0],
        [1, 2, -1, -2, 1, 2, 1, 2],
        [-1, 0, 0, 1, 0, 1, 1, 2],
        [3, -1, -1, -1, -1, -1, -1, 3],
    ] {
        let original = Trilinear::new(values)
            .unwrap()
            .factorization(|| false)
            .unwrap();
        let negative = Trilinear::new(values.map(|v| -v))
            .unwrap()
            .factorization(|| false)
            .unwrap();
        assert_eq!(original.factors, negative.factors);
        assert_eq!(
            negative.residual().field().values(),
            original.residual().field().values().map(|v| -v)
        );
        for perm in permutations {
            for flips in 0..8 {
                let transformed = std::array::from_fn(|i| {
                    let mut old = 0;
                    for (a, original_axis) in perm.into_iter().enumerate() {
                        old |= (((i ^ flips) >> a) & 1) << original_axis;
                    }
                    values[old]
                });
                let result = Trilinear::new(transformed)
                    .unwrap()
                    .factorization(|| false)
                    .unwrap();
                verify(result);
                assert_eq!(result.residual().kind(), original.residual().kind());
                assert_eq!(result.original.phase(), original.original.phase());
                for factor in original.factors() {
                    let new_axis = perm.iter().position(|a| *a == factor.axis()).unwrap();
                    let mut expected = factor.values();
                    if flips & (1 << new_axis) != 0 {
                        expected.reverse();
                    }
                    if expected.into_iter().find(|v| *v != 0).unwrap() < 0 {
                        expected = expected.map(|v| -v);
                    }
                    assert_eq!(
                        result
                            .factors()
                            .find(|f| f.axis() == new_axis)
                            .unwrap()
                            .values(),
                        expected
                    );
                }
            }
        }
    }
}

#[test]
fn factorization_has_fixed_storage_and_every_cancellation_preserves_prior_result() {
    let field = Trilinear::new([1, -1, -1, 1, -1, 1, 1, -1]).unwrap();
    let calls = Cell::new(0);
    let prior = field
        .factorization(|| {
            calls.set(calls.get() + 1);
            false
        })
        .unwrap();
    assert!(std::mem::size_of::<CellFactorization>() <= 128);
    assert!(std::mem::size_of::<AxisFactor>() <= 8);
    for at in 0..calls.get() {
        let count = Cell::new(0);
        assert_eq!(
            field.factorization(|| {
                let n = count.get();
                count.set(n + 1);
                n == at
            }),
            Err(FieldError::Cancelled)
        );
        assert_eq!(field.factorization(|| false).unwrap(), prior);
    }
    for field in [
        Trilinear::new([0; 8]).unwrap(),
        Trilinear::new([-1; 8]).unwrap(),
    ] {
        assert_eq!(field.factorization(|| true), Err(FieldError::Cancelled));
    }
}
