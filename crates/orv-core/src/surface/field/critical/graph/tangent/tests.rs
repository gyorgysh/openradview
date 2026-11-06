// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

fn signs(mut code: usize) -> [i8; 3] {
    std::array::from_fn(|_| {
        let s = (code % 3) as i8 - 1;
        code /= 3;
        s
    })
}
fn rays(g: [i8; 3]) -> Vec<TangentRay> {
    let mut rays = Vec::new();
    for axis in 0..3 {
        for positive in [true, false] {
            if g[axis] == 0 {
                rays.push(TangentRay::Axis {
                    axis: axis as u8,
                    positive,
                });
            }
            if (0..3).any(|a| a != axis && g[a] != 0) {
                rays.push(TangentRay::Face {
                    axis: axis as u8,
                    positive,
                });
            }
        }
    }
    rays
}
fn dot(a: [i128; 3], b: [i128; 3]) -> i128 {
    a.into_iter().zip(b).map(|(a, b)| a * b).sum()
}
fn cross(a: [i128; 3], b: [i128; 3]) -> [i128; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn vector(ray: TangentRay, normal: [i128; 3]) -> [i128; 3] {
    let (axis, sign) = ray.parts();
    let unit = std::array::from_fn(|a| i128::from(a == axis));
    let v = match ray {
        TangentRay::Axis { .. } => unit,
        TangentRay::Face { .. } => cross(normal, unit),
    };
    v.map(|v| v * i128::from(sign))
}
fn angle(reference: [i128; 3], ray: [i128; 3], normal: [i128; 3]) -> f64 {
    let magnitude = (dot(normal, normal) as f64).sqrt();
    let a =
        (dot(normal, cross(reference, ray)) as f64 / magnitude).atan2(dot(reference, ray) as f64);
    if a < 0. { a + std::f64::consts::TAU } else { a }
}

#[test]
fn tangent_predicates_match_full_integer_vectors_for_every_sign_and_extreme_scale() {
    let mut ties = 0;
    for g in (0..27).map(signs).filter(|g| *g != [0; 3]) {
        let ctx = RegularTangents::new(g).unwrap();
        let rays = rays(g);
        for amplitudes in [
            [1, 1, 1],
            [1, 2, 3],
            [7, 17, 31],
            [1, 1 << 20, 1 << 30],
            [1 << 30, 1, 1 << 10],
        ] {
            let normal = std::array::from_fn(|a| -i128::from(g[a]) * amplitudes[a]);
            for a in &rays {
                for b in &rays {
                    let va = vector(*a, normal);
                    let vb = vector(*b, normal);
                    let expected_dot = dot(va, vb).signum() as i8;
                    let expected_cross = dot(cross(va, vb), normal).signum() as i8;
                    assert_eq!(ctx.dot_sign(*a, *b).unwrap(), expected_dot);
                    assert_eq!(ctx.cross_sign(*a, *b).unwrap(), expected_cross);
                    assert_eq!(ctx.dot_sign(a.reversed(), *b).unwrap(), -expected_dot);
                    assert_eq!(ctx.cross_sign(*a, b.reversed()).unwrap(), -expected_cross);
                    assert_eq!(a.reversed().reversed(), *a);
                    if expected_cross == 0 && expected_dot == 1 && a != b {
                        ties += 1;
                    }
                }
            }
            for reference in &rays {
                for a in &rays {
                    for b in &rays {
                        let vr = vector(*reference, normal);
                        let va = vector(*a, normal);
                        let vb = vector(*b, normal);
                        let half = |v| {
                            let y = dot(cross(vr, v), normal);
                            let x = dot(vr, v);
                            if y > 0 || (y == 0 && x > 0) { 0 } else { 1 }
                        };
                        let expected = half(va)
                            .cmp(&half(vb))
                            .then_with(|| 0.cmp(&dot(cross(va, vb), normal)));
                        assert_eq!(ctx.compare(*reference, *a, *b).unwrap(), expected);
                    }
                }
            }
        }
    }
    assert!(ties > 100);
}

#[test]
fn tangent_order_matches_angles_and_keeps_direction_ties_and_transitivity() {
    for g in (0..27).map(signs).filter(|g| *g != [0; 3]) {
        let ctx = RegularTangents::new(g).unwrap();
        let rays = rays(g);
        for amplitudes in [[1, 1, 1], [1, 3, 7], [13, 5, 2]] {
            let normal = std::array::from_fn(|a| -i128::from(g[a]) * amplitudes[a]);
            for reference in &rays {
                let r = vector(*reference, normal);
                for a in &rays {
                    for b in &rays {
                        let va = vector(*a, normal);
                        let vb = vector(*b, normal);
                        let actual = ctx.compare(*reference, *a, *b).unwrap();
                        let same = dot(cross(va, vb), normal) == 0 && dot(va, vb) > 0;
                        let expected = if same {
                            Ordering::Equal
                        } else {
                            angle(r, va, normal)
                                .partial_cmp(&angle(r, vb, normal))
                                .unwrap()
                        };
                        assert_eq!(actual, expected);
                        assert_eq!(ctx.compare(*reference, *b, *a).unwrap(), actual.reverse());
                        for c in &rays {
                            let bc = ctx.compare(*reference, *b, *c).unwrap();
                            if actual != Ordering::Greater && bc != Ordering::Greater {
                                assert_ne!(
                                    ctx.compare(*reference, *a, *c).unwrap(),
                                    Ordering::Greater
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}
fn transform(ray: TangentRay, perm: [usize; 3], flips: u8, orientation: i8) -> TangentRay {
    let (axis, sign) = ray.parts();
    let axis = perm.iter().position(|a| *a == axis).unwrap();
    let sign = sign * if flips & (1 << axis) == 0 { 1 } else { -1 };
    match ray {
        TangentRay::Axis { .. } => TangentRay::Axis {
            axis: axis as u8,
            positive: sign > 0,
        },
        TangentRay::Face { .. } => TangentRay::Face {
            axis: axis as u8,
            positive: sign * orientation > 0,
        },
    }
}
#[test]
fn tangent_order_and_orientation_transform_under_all_axes_reflections_and_complements() {
    for g in (0..27).map(signs).filter(|g| *g != [0; 3]) {
        let ctx = RegularTangents::new(g).unwrap();
        let rays = rays(g);
        for perm in [
            [0, 1, 2],
            [0, 2, 1],
            [1, 0, 2],
            [1, 2, 0],
            [2, 0, 1],
            [2, 1, 0],
        ] {
            let inversions = (0..3)
                .flat_map(|a| (a + 1..3).map(move |b| (a, b)))
                .filter(|(a, b)| perm[*a] > perm[*b])
                .count();
            for flips in 0..8u8 {
                for complement in [1, -1] {
                    let determinant =
                        if (inversions + flips.count_ones() as usize).is_multiple_of(2) {
                            1
                        } else {
                            -1
                        };
                    let orientation = determinant * complement;
                    let other_g = std::array::from_fn(|a| {
                        g[perm[a]] * complement * if flips & (1 << a) == 0 { 1 } else { -1 }
                    });
                    let other = RegularTangents::new(other_g).unwrap();
                    let map = |r| transform(r, perm, flips, orientation);
                    for a in &rays {
                        for b in &rays {
                            assert_eq!(
                                other.dot_sign(map(*a), map(*b)).unwrap(),
                                ctx.dot_sign(*a, *b).unwrap()
                            );
                            assert_eq!(
                                other.cross_sign(map(*a), map(*b)).unwrap(),
                                orientation * ctx.cross_sign(*a, *b).unwrap()
                            );
                            for reference in &rays {
                                let order = ctx.compare(*reference, *a, *b).unwrap();
                                let at_start = |r| {
                                    ctx.cross_sign(*reference, r).unwrap() == 0
                                        && ctx.dot_sign(*reference, r).unwrap() > 0
                                };
                                let expected = if orientation < 0 && !at_start(*a) && !at_start(*b)
                                {
                                    order.reverse()
                                } else {
                                    order
                                };
                                assert_eq!(
                                    other.compare(map(*reference), map(*a), map(*b)).unwrap(),
                                    expected
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}

fn tensor(values: [i16; 8], p: [f64; 3], derivative: Option<usize>) -> f64 {
    values
        .into_iter()
        .enumerate()
        .map(|(i, v)| {
            f64::from(v)
                * (0..3)
                    .map(|a| {
                        if derivative == Some(a) {
                            if i & (1 << a) == 0 { -1. } else { 1. }
                        } else if i & (1 << a) == 0 {
                            1. - p[a]
                        } else {
                            p[a]
                        }
                    })
                    .product::<f64>()
        })
        .sum()
}
#[test]
fn face_tangents_follow_tensor_contours_and_positive_cell_boundary_directions() {
    let mut singular_faces = 0;
    for g in (0..27).map(signs).filter(|g| *g != [0; 3]) {
        let ctx = RegularTangents::new(g).unwrap();
        for axis in 0..3 {
            for upper in [false, true] {
                let mut center_twice = [1i16; 3];
                center_twice[axis] = if upper { 2 } else { 0 };
                for cubic in [-1, 0, 1] {
                    let field = Trilinear::new(std::array::from_fn(|i| {
                        let x: [i16; 3] =
                            std::array::from_fn(|a| 2 * ((i >> a) & 1) as i16 - center_twice[a]);
                        g.into_iter()
                            .zip([1, 3, 7])
                            .zip(x)
                            .map(|((s, m), x)| i16::from(s) * m * x)
                            .sum::<i16>()
                            + 2 * x[0] * x[1]
                            - x[0] * x[2]
                            + 3 * x[1] * x[2]
                            + cubic * x[0] * x[1] * x[2]
                    }))
                    .unwrap();
                    let at = center_twice.map(|v| f64::from(v) / 2.);
                    assert_eq!(tensor(field.values, at, None), 0.);
                    let gradient: [i128; 3] =
                        std::array::from_fn(|a| tensor(field.values, at, Some(a)) as i128);
                    assert_eq!(gradient.map(|v| v.signum() as i8), g);
                    if (0..3).all(|a| a == axis || g[a] == 0) {
                        assert_eq!(
                            ctx.ascending_face(axis as u8, true),
                            Err(FieldError::Arithmetic)
                        );
                        assert_eq!(
                            ctx.cell_boundary(axis as u8, upper),
                            Err(FieldError::Arithmetic)
                        );
                        singular_faces += 1;
                        continue;
                    }
                    let normal = gradient.map(|v| -v);
                    let asc = ctx.ascending_face(axis as u8, true).unwrap();
                    let [u, v] = transverse_axes(axis);
                    let expected = std::array::from_fn(|a| {
                        if a == u {
                            -gradient[v]
                        } else if a == v {
                            gradient[u]
                        } else {
                            0
                        }
                    });
                    let actual = vector(asc, normal);
                    assert_eq!(actual, expected);
                    assert_eq!(
                        ctx.ascending_face(axis as u8, false).unwrap(),
                        asc.reversed()
                    );
                    let boundary = ctx.cell_boundary(axis as u8, upper).unwrap();
                    let n_face: [i128; 3] = std::array::from_fn(|a| {
                        if a == axis {
                            if upper { 1 } else { -1 }
                        } else {
                            0
                        }
                    });
                    assert_eq!(vector(boundary, normal), cross(normal, n_face));
                    assert!(dot(cross(normal, vector(boundary, normal)), n_face.map(|v| -v)) > 0);
                    assert_eq!(
                        ctx.dot_sign(asc, boundary).unwrap(),
                        if upper == (axis != 1) { 1 } else { -1 }
                    );
                    assert_eq!(ctx.cross_sign(asc, boundary).unwrap(), 0);
                    let mut selected = at;
                    let mut unselected = at;
                    for a in [u, v] {
                        selected[a] += gradient[a] as f64 / 65536.;
                        unselected[a] -= gradient[a] as f64 / 65536.;
                    }
                    assert!(tensor(field.values, selected, None) > 0.);
                    assert!(tensor(field.values, unselected, None) < 0.);
                }
            }
        }
    }
    assert!(singular_faces > 0);
}

#[test]
fn tangent_invalid_gradients_axes_and_vanishing_directions_are_refused() {
    for g in [[0; 3], [2, 0, 0], [-2, 0, 0], [0, 0, 127], [0, -128, 0]] {
        assert_eq!(RegularTangents::new(g), Err(FieldError::Arithmetic));
    }
    let ctx = RegularTangents::new([0, 0, 1]).unwrap();
    let reference = TangentRay::Axis {
        axis: 0,
        positive: true,
    };
    for ray in [
        TangentRay::Axis {
            axis: 2,
            positive: true,
        },
        TangentRay::Face {
            axis: 2,
            positive: false,
        },
        TangentRay::Axis {
            axis: 255,
            positive: true,
        },
        TangentRay::Face {
            axis: 3,
            positive: true,
        },
    ] {
        let error = if ray.parts().0 >= 3 {
            FieldError::Bounds
        } else {
            FieldError::Arithmetic
        };
        assert_eq!(ctx.dot_sign(reference, ray), Err(error));
        assert_eq!(ctx.cross_sign(ray, reference), Err(error));
        assert_eq!(ctx.compare(ray, reference, reference), Err(error));
        assert_eq!(ctx.compare(reference, ray, reference), Err(error));
        assert_eq!(ctx.compare(reference, reference, ray), Err(error));
    }
    for axis in [3, 255] {
        assert_eq!(ctx.ascending_face(axis, true), Err(FieldError::Bounds));
        assert_eq!(ctx.cell_boundary(axis, false), Err(FieldError::Bounds));
    }
    assert!(std::mem::size_of::<RegularTangents>() <= 3);
}

#[test]
fn shared_tangent_kernel_preserves_the_original_regular_anchor_start_and_order() {
    for mut code in 0..6561 {
        let field = Trilinear::new(std::array::from_fn(|_| {
            let s = (code % 3) as i16 - 1;
            code /= 3;
            s
        }))
        .unwrap();
        let graph = field.critical_graph(|| false).unwrap();
        for (id, _) in graph.nodes() {
            let AnchorStar::Regular {
                gradient_signs: g,
                ring,
                ..
            } = graph.anchor_star(id, || false).unwrap()
            else {
                continue;
            };
            let axes: Vec<_> = (0..3).filter(|a| g[*a] == 0).collect();
            let order = match axes.as_slice() {
                [a] => vec![(*a, true), (*a, false)],
                [a, b] => {
                    let k = 3 - a - b;
                    let forward = -g[k] * if k == 1 { -1 } else { 1 } > 0;
                    vec![(*a, true), (*b, forward), (*a, false), (*b, !forward)]
                }
                _ => panic!("Regular anchor axes required"),
            };
            let expected: Vec<_> = order
                .into_iter()
                .filter_map(|(axis, positive)| {
                    ring.rays()
                        .iter()
                        .find(|r| usize::from(r.axis) == axis && r.positive == positive)
                        .copied()
                })
                .collect();
            assert_eq!(ring.rays(), expected);
        }
    }
}
