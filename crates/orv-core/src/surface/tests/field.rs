// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use crate::surface::field::*;
use std::collections::{BTreeMap, VecDeque};

fn tensor(values: [i16; 8], [x, y, z]: [f64; 3]) -> f64 {
    let lerp = |a: f64, b: f64, t: f64| a * (1. - t) + b * t;
    let row = |i| lerp(f64::from(values[i]), f64::from(values[i + 1]), x);
    lerp(lerp(row(0), row(2), y), lerp(row(4), row(6), y), z)
}

#[test]
fn source_bound_field_retains_manual_identity_without_changing_labels() {
    let source = source([4; 3], true, false);
    let labels = labels(source.clone(), |p| if p[0] == 1 { 7 } else { 9 });
    let field = MaskField::new(MaskOwner::Manual(labels.clone()), 7).unwrap();
    let original = labels.values().iter().copied().collect::<Vec<_>>();
    assert!(Arc::ptr_eq(field.source(), &source));
    assert_eq!(field.key(), &MaskView::manual(&labels, 7).unwrap().key());
    assert_eq!(field.revision(), labels.revision());
    let weak = Arc::downgrade(&labels);
    drop(labels);
    assert!(weak.upgrade().is_some());
    for x in 0..4 {
        let sample = field.sample([x, 1, 1]).unwrap();
        assert_eq!(sample.value, Some(if x == 1 { 1 } else { -1 }));
        assert!(sample.source_valid);
    }
    let cell = field.cell([0; 3]).unwrap();
    assert_eq!(cell.origin(), [0; 3]);
    assert_eq!(
        cell.interpolant().unwrap().values(),
        [-1, 1, -1, 1, -1, 1, -1, 1]
    );
    assert_eq!(cell.interpolant().unwrap().evaluate([0.5; 3]), Some(0.));
    assert_eq!(field.cell([3, 0, 0]), Err(FieldError::Bounds));
    assert_eq!(field.cell([usize::MAX, 0, 0]), Err(FieldError::Bounds));
    assert_eq!(field.sample([4, 0, 0]), None);
    let MaskOwner::Manual(map) = field.owner() else {
        panic!("Manual owner lost")
    };
    assert_eq!(map.values().iter().copied().collect::<Vec<_>>(), original);
    assert!(MaskField::new(field.owner().clone(), 0).is_err());
    assert!(MaskField::new(field.owner().clone(), 256).is_err());
    drop(field);
    assert!(weak.upgrade().is_none());
}

#[test]
fn fractional_half_level_ties_select_whole_zero_cells_without_changing_samples() {
    for (maximum, value, phase, kind) in [
        (200, 100, CellPhase::Selected, ResidualKind::Whole),
        (255, 127, CellPhase::Unselected, ResidualKind::Constant),
        (255, 128, CellPhase::Selected, ResidualKind::Constant),
    ] {
        let masks = Arc::new(
            Segments::from_fractional(
                source([2; 3], false, false),
                &[65535],
                &[(65535, 0), (65535, 1)],
                &[value; 8],
                FractionalEncoding {
                    kind: FractionalKind::Probability,
                    maximum,
                },
                Some(true),
                || false,
            )
            .unwrap(),
        );
        let field = MaskField::new(MaskOwner::Supplied(masks.clone()), 65535).unwrap();
        let cell = field.cell([0; 3]).unwrap().interpolant().unwrap();
        assert_eq!(cell.phase(), phase);
        assert_eq!(
            cell.factorization(|| false).unwrap().residual().kind(),
            kind
        );
        let face = field
            .source_face(
                FaceKey {
                    origin: [0; 3],
                    axis: 0,
                },
                || false,
            )
            .unwrap();
        assert_eq!(face.cells()[1].phase(), Some(phase));
        assert_eq!(face.factorizations()[1].unwrap().original(), cell);
        for x in 0..2 {
            for y in 0..2 {
                for z in 0..2 {
                    assert_eq!(masks.mask(65535).unwrap().raw_value([x, y, z]), Some(value));
                    assert_eq!(
                        masks.mask(65535).unwrap().value([x, y, z]),
                        Some(phase == CellPhase::Selected)
                    );
                }
            }
        }
    }
}

#[test]
fn fractional_field_uses_original_amplitudes_and_keeps_missing_and_unknown_separate() {
    let source = source([3; 3], false, true);
    for (maximum, kind) in [
        (200, FractionalKind::Probability),
        (255, FractionalKind::Occupancy),
    ] {
        let frames = [(65535, 0), (65535, 1), (2, 0)];
        let mut payload: Vec<_> = (0..27)
            .map(|i| match i % 4 {
                0 => 0,
                1 => maximum / 2,
                2 => maximum / 2 + 1,
                _ => maximum,
            })
            .collect();
        payload.push(0);
        let complete = Arc::new(
            Segments::from_fractional(
                super::source([3; 3], false, false),
                &[65535, 2],
                &frames,
                &payload,
                FractionalEncoding { kind, maximum },
                Some(true),
                || false,
            )
            .unwrap(),
        );
        let complete = MaskField::new(MaskOwner::Supplied(complete), 65535).unwrap();
        assert_eq!(
            complete
                .cell([0; 3])
                .unwrap()
                .interpolant()
                .unwrap()
                .values(),
            [0, 1, 3, 4, 9, 10, 12, 13].map(|i| 2 * i16::from(payload[i]) - i16::from(maximum))
        );
        let masks = Arc::new(
            Segments::from_fractional(
                source.clone(),
                &[65535, 2],
                &frames,
                &payload,
                FractionalEncoding { kind, maximum },
                Some(true),
                || false,
            )
            .unwrap(),
        );
        let field = MaskField::new(MaskOwner::Supplied(masks.clone()), 65535).unwrap();
        assert_eq!(
            field.key(),
            &MaskView::supplied(masks.mask(65535).unwrap()).key()
        );
        assert_eq!(field.revision(), masks.revision());
        for (i, value) in payload[..18].iter().enumerate() {
            let voxel = [i % 3, i / 3 % 3, i / 9];
            let sample = field.sample(voxel).unwrap();
            assert_eq!(
                sample.value,
                Some(2 * i16::from(*value) - i16::from(maximum))
            );
            assert_eq!(sample.source_valid, voxel != [1; 3]);
            assert_eq!(masks.mask(65535).unwrap().raw_value(voxel), Some(*value));
        }
        let cell = field.cell([0; 3]).unwrap();
        assert!(cell.corners().iter().all(|v| v.value.is_some()));
        assert!(
            cell.interpolant().is_none(),
            "Missing source corner must stay open"
        );
        let unknown = field.sample([0, 0, 2]).unwrap();
        assert_eq!(unknown.value, None);
        assert!(unknown.source_valid);
        assert!(field.cell([0, 0, 1]).unwrap().interpolant().is_none());
        assert!(field.cell([0; 3]).unwrap().face(0, false).is_none());
        assert!(MaskField::new(MaskOwner::Supplied(masks), 42).is_err());
    }
}

#[test]
fn binary_supplied_fields_keep_high_ids_and_overlapping_memberships_separate() {
    let source = source([2; 3], false, false);
    let masks = Arc::new(
        Segments::from_binary(
            source,
            &[65535, 2],
            &[(65535, 0), (2, 0), (65535, 1), (2, 1)],
            &[0x35, 0x35],
            Some(true),
            || false,
        )
        .unwrap(),
    );
    let first = MaskField::new(MaskOwner::Supplied(masks.clone()), 65535).unwrap();
    let second = MaskField::new(MaskOwner::Supplied(masks), 2).unwrap();
    assert_ne!(first.key(), second.key());
    assert_eq!(first.key().object, second.key().object);
    assert_eq!(first.revision(), second.revision());
    assert_eq!(
        first.cell([0; 3]).unwrap().interpolant().unwrap().values(),
        [1, -1, 1, -1, 1, -1, 1, -1]
    );
    assert_eq!(
        second.cell([0; 3]).unwrap().interpolant().unwrap().values(),
        [1, 1, -1, -1, 1, 1, -1, -1]
    );
}

#[test]
fn all_cube_signs_and_axis_transforms_match_independent_tensor_interpolation() {
    let permutations = [
        [0, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ];
    for signs in 0..256 {
        for mode in 0..3 {
            let values = std::array::from_fn(|i| {
                let magnitude = match mode {
                    0 => 1,
                    1 => (i * 17 + signs * 3) % 255 + 1,
                    _ => 255,
                };
                if signs & (1 << i) == 0 {
                    -(magnitude as i16)
                } else {
                    magnitude as i16
                }
            });
            let field = Trilinear::new(values).unwrap();
            for x in [0., 0.125, 0.5, 0.875, 1.] {
                for y in [0., 0.25, 0.75, 1.] {
                    for z in [0., 0.5, 1.] {
                        assert_eq!(field.evaluate([x, y, z]), Some(tensor(values, [x, y, z])));
                    }
                }
            }
            for permutation in permutations {
                for flips in 0..8 {
                    let transformed = std::array::from_fn(|i| {
                        let mut old = 0;
                        for (a, p) in permutation.into_iter().enumerate() {
                            old |= (((i >> a) & 1) ^ ((flips >> a) & 1)) << p;
                        }
                        values[old]
                    });
                    let point = [0.125, 0.5, 0.75];
                    let mut old_point = [0.; 3];
                    for (a, p) in permutation.into_iter().enumerate() {
                        old_point[p] = if flips & (1 << a) == 0 {
                            point[a]
                        } else {
                            1. - point[a]
                        };
                    }
                    assert_eq!(
                        Trilinear::new(transformed).unwrap().evaluate(point),
                        Some(tensor(values, old_point))
                    );
                }
            }
        }
    }
    assert_eq!(Trilinear::new([256; 8]), Err(FieldError::Value));
    assert_eq!(Bilinear::new([i16::MIN; 4]), Err(FieldError::Value));
    let field = Trilinear::new([1; 8]).unwrap();
    for point in [
        [f64::NAN, 0., 0.],
        [0., f64::INFINITY, 0.],
        [0., 0., -0.0001],
        [1.00001, 0., 0.],
    ] {
        assert_eq!(field.evaluate(point), None);
    }
    assert!(field.face(3, false).is_none());
    assert!(field.section_extremum(3).is_err());
}

fn sampled_connected(values: [i16; 4], from: usize, to: usize) -> bool {
    const N: usize = 128;
    let endpoints = [[0, 0], [N, 0], [N, N], [0, N]];
    let phase = values[from].signum();
    let inside = |x: usize, y: usize| {
        let [a, b, c, d] = values.map(i64::from);
        let (x, y, n) = (x as i64, y as i64, N as i64);
        (a * (n - x) * (n - y) + b * x * (n - y) + c * x * y + d * (n - x) * y).signum()
            == i64::from(phase)
    };
    let mut seen = vec![false; (N + 1) * (N + 1)];
    let mut queue = VecDeque::from([endpoints[from]]);
    seen[endpoints[from][1] * (N + 1) + endpoints[from][0]] = true;
    while let Some([x, y]) = queue.pop_front() {
        if [x, y] == endpoints[to] {
            return true;
        }
        for [dx, dy] in [[-1, 0], [1, 0], [0, -1], [0, 1]] {
            if let (Some(x), Some(y)) = (x.checked_add_signed(dx), y.checked_add_signed(dy))
                && x <= N
                && y <= N
                && !seen[y * (N + 1) + x]
                && inside(x, y)
            {
                seen[y * (N + 1) + x] = true;
                queue.push_back([x, y]);
            }
        }
    }
    false
}
fn arc_set(topology: FaceTopology, offset: u8) -> BTreeSet<[u8; 2]> {
    let arcs = match topology {
        FaceTopology::Arc(a) => vec![a],
        FaceTopology::Split { arcs, .. } => arcs.to_vec(),
        _ => vec![],
    };
    arcs.into_iter()
        .map(|a| {
            let mut a = a.map(|v| (v + offset) % 4);
            a.sort();
            a
        })
        .collect()
}

#[test]
fn strict_face_connections_agree_with_independent_sampled_regions_and_dihedral_symmetry() {
    for signs in 0..16 {
        let values = std::array::from_fn(|i| {
            if signs & (1 << i) == 0 {
                -([2, 5, 3, 7][i])
            } else {
                [2, 5, 3, 7][i]
            }
        });
        let base = Bilinear::new(values).unwrap().topology().unwrap();
        let crossing = (0..4)
            .filter(|i| values[*i].signum() != values[(*i + 1) % 4].signum())
            .count();
        assert_eq!(arc_set(base, 0).len(), crossing / 2);
        if let FaceTopology::Split {
            joined_corners: [a, b],
            ..
        } = base
        {
            assert!(sampled_connected(values, a as usize, b as usize));
            let other: Vec<_> = (0..4)
                .filter(|i| *i != a as usize && *i != b as usize)
                .collect();
            assert!(!sampled_connected(values, other[0], other[1]));
        }
        for offset in 0..4 {
            let rotated = std::array::from_fn(|i| values[(i + offset) % 4]);
            let topology = Bilinear::new(rotated).unwrap().topology().unwrap();
            assert_eq!(arc_set(topology, offset as u8), arc_set(base, 0));
            assert_eq!(
                Bilinear::new(rotated.map(|v| -v))
                    .unwrap()
                    .topology()
                    .unwrap(),
                topology
            );
        }
        let reflected = [values[0], values[3], values[2], values[1]];
        let mirror = Bilinear::new(reflected).unwrap().topology().unwrap();
        let reflected_arcs: BTreeSet<_> = arc_set(mirror, 0)
            .into_iter()
            .map(|pair| {
                let mut pair = pair.map(|e| 3 - e);
                pair.sort();
                pair
            })
            .collect();
        assert_eq!(reflected_arcs, arc_set(base, 0));
    }
}

#[test]
fn exact_saddles_near_critical_products_and_zero_contacts_are_not_empty_faces() {
    for [a, b] in [[1, 1], [2, 3], [17, 29], [254, 255]] {
        // The independent factorization is (1-2u)*(a*(1-v)-b*v).
        let values = [a, -a, b, -b];
        let FaceTopology::Saddle { position: [u, v] } =
            Bilinear::new(values).unwrap().topology().unwrap()
        else {
            panic!("Exact saddle lost")
        };
        assert_eq!(u.as_f64(), 0.5);
        assert_eq!(
            v.numerator() * i64::from(a + b),
            i64::from(a) * v.denominator() as i64
        );
    }
    assert!(matches!(
        Bilinear::new([254, -255, 254, -253])
            .unwrap()
            .topology()
            .unwrap(),
        FaceTopology::Split {
            joined_corners: [0, 2],
            ..
        }
    ));
    assert!(matches!(
        Bilinear::new([255, -254, 253, -254])
            .unwrap()
            .topology()
            .unwrap(),
        FaceTopology::Split {
            joined_corners: [1, 3],
            ..
        }
    ));
    for mask in 1..16 {
        let values = std::array::from_fn(|i| if mask & (1 << i) != 0 { 0 } else { 1 });
        assert_eq!(
            Bilinear::new(values).unwrap().topology().unwrap(),
            FaceTopology::NonRegular { zero_corners: mask }
        );
    }
    for (a, b, expected) in [
        (0, 0, 2),
        (0, 255, 0),
        (-255, 0, 1),
        (-255, 255, 3),
        (255, 1, 4),
    ] {
        let mut values = [1; 8];
        values[0] = a;
        values[1] = b;
        let field = Trilinear::new(values).unwrap();
        match (field.edge(0, 0).unwrap(), expected) {
            (EdgeZero::Entire, 2) | (EdgeZero::Empty, 4) => (),
            (EdgeZero::Point(t), 0) => assert_eq!((t.numerator(), t.denominator()), (0, 1)),
            (EdgeZero::Point(t), 1) => assert_eq!((t.numerator(), t.denominator()), (1, 1)),
            (EdgeZero::Point(t), 3) => assert_eq!((t.numerator(), t.denominator()), (1, 2)),
            other => panic!("Unexpected edge zero: {other:?}"),
        }
        assert!(field.edge(3, 0).is_none());
        assert!(field.edge(0, 1).is_none());
        assert!(field.edge(1, 8).is_none());
    }
}

#[test]
fn neighboring_cells_share_exact_faces_and_edge_positions_across_chunk_boundaries() {
    let source = source([35; 3], true, false);
    let map = labels(source, |p| u8::from((p[0] + p[1] + p[2]) % 2 == 0));
    let field = MaskField::new(MaskOwner::Manual(map), 1).unwrap();
    let mut edges = BTreeMap::new();
    for z in 30..33 {
        for y in 30..33 {
            for x in 30..33 {
                let cell = field.cell([x, y, z]).unwrap();
                for axis in 0..3 {
                    let mut neighbor = [x, y, z];
                    neighbor[axis] += 1;
                    assert_eq!(
                        cell.face(axis, true),
                        field.cell(neighbor).unwrap().face(axis, false)
                    );
                    for corner in 0..8 {
                        if corner & (1 << axis) == 0 {
                            let (key, zero) = cell.edge(axis, corner).unwrap();
                            if let Some(previous) = edges.insert(key, zero) {
                                assert_eq!(previous, zero);
                            }
                            let EdgeZero::Point(t) = zero else {
                                panic!("Checkerboard edge lost")
                            };
                            assert_eq!((t.numerator(), t.denominator()), (1, 2));
                        }
                    }
                }
            }
        }
    }
    assert!(edges.keys().any(|key| key.origin.contains(&32)));
}

#[test]
fn section_extrema_keep_exact_zero_and_both_phases_without_claiming_a_diagonal_connection() {
    for k in [2, 3, 4] {
        let values = [7, -k, -k, -1, -1, -k, -k, 7];
        for sign in [-1, 1] {
            let field = Trilinear::new(values.map(|v| v * sign)).unwrap();
            let candidate = field.section_extremum(2).unwrap().unwrap();
            assert_eq!(
                (
                    candidate.coordinate.numerator(),
                    candidate.coordinate.denominator()
                ),
                (1, 2)
            );
            assert_eq!(
                candidate.corners.map(Rational::as_f64),
                [3., -(k as f64), 3., -(k as f64)].map(|v| v * f64::from(sign))
            );
            assert_eq!(candidate.determinant.numerator(), i64::from(9 - k * k));
            assert_eq!(candidate.determinant.denominator(), 1);
            assert!(!candidate.minimum);
        }
    }
    let field = Trilinear::new([1; 8]).unwrap();
    assert!(field.section_extremum(0).unwrap().is_none());
    // No zero surface exists, even though this positive field has a determinant extremum.
    let positive = Trilinear::new([1, 2, 2, 9, 9, 2, 2, 1]).unwrap();
    assert!(positive.section_extremum(2).unwrap().is_some());
    assert_eq!(
        positive.face(0, false).unwrap().topology().unwrap(),
        FaceTopology::Empty
    );
}

#[test]
fn ternary_and_nonuniform_patterns_bound_section_arithmetic_and_match_direct_bernstein_sections() {
    let ternary = (0..3usize.pow(8)).map(|mut code| {
        std::array::from_fn(|_| {
            let v = [-255, 0, 255][code % 3];
            code /= 3;
            v
        })
    });
    let nonuniform = (0..4096u64).map(|seed| {
        let mut state = seed + 1;
        std::array::from_fn(|_| {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (state % 511) as i16 - 255
        })
    });
    let mut widest_denominator = 0;
    for values in ternary.chain(nonuniform) {
        let field = Trilinear::new(values).unwrap();
        for axis in 0..3 {
            let polynomial = field.section_determinant(axis).unwrap();
            let mut section_corners = [[0., 0., 0.], [0., 0., 0.], [0., 0., 0.], [0., 0., 0.]];
            let tangential: Vec<_> = (0..3).filter(|a| *a != axis).collect();
            for (p, [u, v]) in
                section_corners
                    .iter_mut()
                    .zip([[0., 0.], [1., 0.], [1., 1.], [0., 1.]])
            {
                p[tangential[0]] = u;
                p[tangential[1]] = v;
            }
            let direct = |t| {
                let values = section_corners.map(|mut p| {
                    p[axis] = t;
                    tensor(values, p)
                });
                values[0] * values[2] - values[1] * values[3]
            };
            for t in [0., 0.125, 0.5, 0.875, 1.] {
                assert_eq!(
                    direct(t),
                    polynomial.quadratic as f64 * t * t
                        + polynomial.linear as f64 * t
                        + polynomial.constant as f64
                );
            }
            if let Some(candidate) = field.section_extremum(axis).unwrap() {
                let t = candidate.coordinate;
                widest_denominator = widest_denominator.max(t.denominator());
                assert_eq!(
                    2 * i128::from(polynomial.quadratic) * i128::from(t.numerator())
                        + i128::from(polynomial.linear) * i128::from(t.denominator()),
                    0
                );
                assert!((candidate.determinant.as_f64() - direct(t.as_f64())).abs() < 1e-8);
            }
        }
    }
    assert!(widest_denominator > 10000);
}
