// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use crate::{
    mask::MaskOwner,
    segments::{FractionalEncoding, FractionalKind, Segments},
    study::{Geometry, demo_study},
    volume::Volume,
};
use std::sync::Arc;

fn source(reflected: bool, missing: bool) -> Arc<Volume> {
    let template = demo_study().series.remove(0).frames.remove(0);
    let frames = (0..2)
        .map(|z| {
            let mut frame = (*template).clone();
            frame.width = 2;
            frame.height = 2;
            frame.pixels = vec![1.; 4].into();
            frame.validity = missing.then(|| {
                if z == 0 {
                    vec![1, 0, 1, 1].into()
                } else {
                    vec![1; 4].into()
                }
            });
            frame.geometry = Some(Geometry {
                position: [
                    1_000_000. + z as f64 * 0.2,
                    -2_000_000. + z as f64 * 0.1,
                    3_000_000. + z as f64 * if reflected { -2. } else { 2. },
                ],
                orientation: if reflected {
                    [-1., 0., 0., 0., 1., 0.]
                } else {
                    [1., 0., 0., 0., 1., 0.]
                },
                spacing: [2., 0.75],
                frame_of_reference: Some("generated affine cell".into()),
                nifti_transform: None,
            });
            Arc::new(frame)
        })
        .collect();
    Arc::new(Volume::new(frames).unwrap())
}
fn field(source: Arc<Volume>, values: [i16; 8], slices: &[(u16, usize)]) -> MaskField {
    let payload: Vec<u8> = slices
        .iter()
        .flat_map(|(_, z)| {
            values[z * 4..z * 4 + 4]
                .iter()
                .map(|v| ((v + 200) / 2) as u8)
        })
        .collect();
    let masks = Arc::new(
        Segments::from_fractional(
            source,
            &[7],
            slices,
            &payload,
            FractionalEncoding {
                kind: FractionalKind::Probability,
                maximum: 200,
            },
            Some(true),
            || false,
        )
        .unwrap(),
    );
    MaskField::new(MaskOwner::Supplied(masks), 7).unwrap()
}
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a.into_iter().zip(b).map(|(a, b)| a * b).sum()
}
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn relative_world(source: &Volume, p: [Rational; 3]) -> [f64; 3] {
    source.voxel_to_world(p.map(Rational::as_f64))
}

#[test]
fn source_bound_planes_have_exact_vertices_outward_winding_and_normal_with_shear_and_reflection() {
    for reflected in [false, true] {
        let source = source(reflected, false);
        let field = field(
            source.clone(),
            [-4, -2, 0, 2, 2, 4, 6, 8],
            &[(7, 0), (7, 1)],
        );
        let cell = field.affine_cell([0; 3], || false).unwrap();
        assert_eq!(cell.disposition(), AffineDisposition::Plane);
        assert_eq!(cell.gradient(), Some([2, 4, 6]));
        assert_eq!(cell.field().key(), field.key());
        assert_eq!(cell.field().revision(), field.revision());
        assert!((3..=6).contains(&cell.vertices().len()));
        assert_eq!(cell.triangles().count(), cell.vertices().len() - 2);
        let normal = cell.physical_normal().unwrap();
        for triangle in cell.triangles() {
            let [a, b, c] = triangle.map(|i| relative_world(&source, cell.vertices()[i as usize]));
            let ab = std::array::from_fn(|i| b[i] - a[i]);
            let ac = std::array::from_fn(|i| c[i] - a[i]);
            assert!(dot(cross(ab, ac), normal) > 0.);
        }
        for p in cell.vertices() {
            assert_exact_plane(*p, -4, [2, 4, 6]);
        }
    }
}
fn assert_exact_plane(p: [Rational; 3], base: i32, gradient: [i32; 3]) {
    let den = p.map(|v| i128::from(v.denominator()));
    let common = den.into_iter().product::<i128>();
    let value = i128::from(base) * common
        + (0..3)
            .map(|i| i128::from(gradient[i]) * i128::from(p[i].numerator()) * (common / den[i]))
            .sum::<i128>();
    assert_eq!(value, 0, "{p:?}, base {base}, gradient {gradient:?}");
}
#[test]
fn all_small_affine_crossings_cover_every_cube_edge_intersection() {
    let mut checked = 0;
    for gx in -3i32..=3 {
        for gy in -3i32..=3 {
            for gz in -3i32..=3 {
                for base in -5i32..=5 {
                    let gradient = [gx, gy, gz];
                    let values = std::array::from_fn::<_, 8, _>(|i| {
                        (base
                            + (0..3)
                                .map(|a| gradient[a] * ((i >> a) & 1) as i32)
                                .sum::<i32>()) as i16
                    });
                    if !values.iter().any(|v| *v < 0) || !values.iter().any(|v| *v > 0) {
                        continue;
                    }
                    let (vertices, count) = polygon(values, gradient, &|| false).unwrap();
                    let vertices = &vertices[..count as usize];
                    for p in vertices {
                        assert_exact_plane(*p, base, gradient);
                        assert!(
                            p.iter()
                                .filter(|v| v.numerator() == 0
                                    || v.numerator() as u64 == v.denominator())
                                .count()
                                >= 2
                        );
                    }
                    let axis = (0..3).max_by_key(|a| gradient[*a].abs()).unwrap();
                    let axes = match axis {
                        0 => [1, 2],
                        1 => [0, 2],
                        _ => [0, 1],
                    };
                    let orientation = turn(vertices[0], vertices[1], vertices[2], axes).signum();
                    assert_ne!(orientation, 0);
                    assert_eq!(
                        orientation
                            * (if axis == 1 { -1 } else { 1 })
                            * i128::from(gradient[axis].signum()),
                        -1
                    );
                    for a in 0..3 {
                        for i in 0usize..8 {
                            if i & (1 << a) != 0 {
                                continue;
                            }
                            let j = i | (1 << a);
                            let av = values[i];
                            let bv = values[j];
                            let mut positions = Vec::new();
                            if av == 0 {
                                positions.push(0.);
                            }
                            if bv == 0 {
                                positions.push(1.);
                            }
                            if av.signum() * bv.signum() == -1 {
                                positions.push(f64::from(av) / f64::from(av - bv));
                            }
                            for position in positions {
                                let mut p =
                                    std::array::from_fn::<_, 3, _>(|k| ((i >> k) & 1) as f64);
                                p[a] = position;
                                for edge in 0..vertices.len() {
                                    let v = vertices[edge].map(Rational::as_f64);
                                    let w =
                                        vertices[(edge + 1) % vertices.len()].map(Rational::as_f64);
                                    let area = (w[axes[0]] - v[axes[0]])
                                        * (p[axes[1]] - v[axes[1]])
                                        - (w[axes[1]] - v[axes[1]]) * (p[axes[0]] - v[axes[0]]);
                                    assert!(
                                        area * orientation as f64 >= -1e-12,
                                        "base {base}, gradient {gradient:?}, point {p:?}"
                                    );
                                }
                            }
                        }
                    }
                    checked += 1;
                }
            }
        }
    }
    assert!(checked > 1000);
}
#[test]
fn unknown_invalid_non_affine_and_non_crossing_cells_remain_explicit() {
    let values = [-4, -2, 0, 2, 2, 4, 6, 8];
    let missing = field(source(false, true), values, &[(7, 0), (7, 1)])
        .affine_cell([0; 3], || false)
        .unwrap();
    assert_eq!(
        missing.disposition(),
        AffineDisposition::Unavailable {
            invalid_corners: 2,
            unknown_corners: 0
        }
    );
    let unknown = field(source(false, false), values, &[(7, 0)])
        .affine_cell([0; 3], || false)
        .unwrap();
    assert_eq!(
        unknown.disposition(),
        AffineDisposition::Unavailable {
            invalid_corners: 0,
            unknown_corners: 0xf0
        }
    );
    let nonaffine = field(
        source(false, false),
        [-4, -2, 0, 2, 2, 4, 6, 10],
        &[(7, 0), (7, 1)],
    )
    .affine_cell([0; 3], || false)
    .unwrap();
    assert_eq!(nonaffine.disposition(), AffineDisposition::NonAffine);
    assert_eq!(nonaffine.gradient(), None);
    let empty = field(source(false, false), [-2; 8], &[(7, 0), (7, 1)])
        .affine_cell([0; 3], || false)
        .unwrap();
    assert_eq!(
        empty.disposition(),
        AffineDisposition::NoInteriorCrossing(CellPhase::Unselected)
    );
    assert_eq!(empty.vertices().len(), 0);
    assert!(empty.physical_normal().is_none());
    let f = field(source(false, false), values, &[(7, 0), (7, 1)]);
    assert!(matches!(
        f.affine_cell([0; 3], || true),
        Err(FieldError::Cancelled)
    ));
    assert!(matches!(
        f.affine_cell([1, 0, 0], || false),
        Err(FieldError::Bounds)
    ));
}

fn neighboring_field(axis: usize, invalid: bool, shape: u8) -> MaskField {
    let template = demo_study().series.remove(0).frames.remove(0);
    let frames = (0..3)
        .map(|z| {
            let mut f = (*template).clone();
            f.width = 3;
            f.height = 3;
            f.pixels = vec![1.; 9].into();
            let mut valid = vec![1; 9];
            let mut missing = [0; 3];
            missing[axis] = 2;
            if invalid && z == missing[2] {
                valid[missing[1] * 3 + missing[0]] = 0;
            }
            f.validity = invalid.then(|| valid.into());
            f.geometry = Some(Geometry {
                position: [z as f64 * 0.2, 0., z as f64 * 2.],
                orientation: [1., 0., 0., 0., 1., 0.],
                spacing: [2., 0.75],
                frame_of_reference: Some("generated adjacent affine".into()),
                nifti_transform: None,
            });
            Arc::new(f)
        })
        .collect();
    let source = Arc::new(Volume::new(frames).unwrap());
    let transverse = match axis {
        0 => [1, 2],
        1 => [0, 2],
        _ => [0, 1],
    };
    let mut payload = Vec::new();
    for z in 0..3 {
        for y in 0..3 {
            for x in 0..3 {
                let p = [x, y, z];
                let value = match shape {
                    0 => {
                        -4 + 4 * p[transverse[0]] as i16
                            + 4 * p[transverse[1]] as i16
                            + [-2, 0, 4][p[axis]]
                    }
                    1 => [-2, 2, -2][p[axis]],
                    _ => 2 * (p[transverse[0]] + p[transverse[1]]) as i16 + [-2, 0, -2][p[axis]],
                };
                payload.push(((value + 200) / 2) as u8);
            }
        }
    }
    payload.push(0);
    let masks = Arc::new(
        Segments::from_fractional(
            source,
            &[7],
            &[(7, 0), (7, 1), (7, 2)],
            &payload,
            FractionalEncoding {
                kind: FractionalKind::Probability,
                maximum: 200,
            },
            Some(true),
            || false,
        )
        .unwrap(),
    );
    MaskField::new(MaskOwner::Supplied(masks), 7).unwrap()
}
#[test]
fn adjacent_affine_cells_agree_on_exact_shared_face_traces_for_each_axis() {
    let zero = Rational::new(0, 1).unwrap();
    let one = Rational::new(1, 1).unwrap();
    for axis in 0..3 {
        let field = neighboring_field(axis, false, 0);
        let a = field.affine_cell([0; 3], || false).unwrap();
        let mut origin = [0; 3];
        origin[axis] = 1;
        let b = field.affine_cell(origin, || false).unwrap();
        assert_eq!(a.disposition(), AffineDisposition::Plane);
        assert_eq!(b.disposition(), AffineDisposition::Plane);
        assert_ne!(a.gradient(), b.gradient());
        let face = a.shared_face(&b, axis, || false).unwrap();
        assert_eq!(
            face.key(),
            FaceKey {
                origin: origin.map(|v| v as u32),
                axis: axis as u8
            }
        );
        assert_eq!(
            face.state(),
            AffineFaceState::Matched(AffineFaceTrace::Segment([[zero, one], [one, zero]]))
        );
        assert_eq!(
            b.shared_face(&a, axis, || false).unwrap().state(),
            face.state()
        );
        assert_eq!(face.field().revision(), field.revision());
        assert!(matches!(
            a.shared_face(&b, axis, || true),
            Err(FieldError::Cancelled)
        ));
        assert!(matches!(
            a.shared_face(&b, (axis + 1) % 3, || false),
            Err(FieldError::Bounds)
        ));
        let other = neighboring_field(axis, false, 0)
            .affine_cell(origin, || false)
            .unwrap();
        assert!(matches!(
            a.shared_face(&other, axis, || false),
            Err(FieldError::Selection)
        ));
        let field = neighboring_field(axis, true, 0);
        let a = field.affine_cell([0; 3], || false).unwrap();
        let b = field.affine_cell(origin, || false).unwrap();
        assert!(matches!(
            a.shared_face(&b, axis, || false).unwrap().state(),
            AffineFaceState::Unresolved {
                positive: AffineDisposition::Unavailable { .. },
                ..
            }
        ));
    }
}

#[test]
fn shared_affine_faces_distinguish_empty_and_single_point_contacts() {
    let zero = Rational::new(0, 1).unwrap();
    for axis in 0..3 {
        for (shape, expected) in [
            (1, AffineFaceTrace::Empty),
            (2, AffineFaceTrace::Point([zero, zero])),
        ] {
            let field = neighboring_field(axis, false, shape);
            let first = field.affine_cell([0; 3], || false).unwrap();
            let mut next = [0; 3];
            next[axis] = 1;
            let second = field.affine_cell(next, || false).unwrap();
            assert_eq!(first.disposition(), AffineDisposition::Plane);
            assert_eq!(second.disposition(), AffineDisposition::Plane);
            assert_eq!(
                first.shared_face(&second, axis, || false).unwrap().state(),
                AffineFaceState::Matched(expected)
            );
        }
    }
}

#[test]
fn adjacent_faces_reject_different_objects_and_stale_revisions_on_the_same_source() {
    use crate::labels::Labels;
    let source = neighboring_field(0, false, 0).source().clone();
    let mut labels = Labels::empty(source.clone()).unwrap();
    labels.paint(0, &[[0., 0.]], 1, 7, || false).unwrap();
    let old = Arc::new(labels.try_clone(|| false).unwrap());
    labels.paint(0, &[[2., 2.]], 1, 7, || false).unwrap();
    let new = Arc::new(labels);
    assert_eq!(old.object_id(), new.object_id());
    assert_ne!(old.revision(), new.revision());
    let old_field = MaskField::new(MaskOwner::Manual(old), 7).unwrap();
    let new_field = MaskField::new(MaskOwner::Manual(new), 7).unwrap();
    let a = old_field.affine_cell([0, 0, 0], || false).unwrap();
    let b = new_field.affine_cell([1, 0, 0], || false).unwrap();
    assert!(matches!(
        a.shared_face(&b, 0, || false),
        Err(FieldError::Selection)
    ));
    let different = MaskField::new(
        MaskOwner::Manual(Arc::new(Labels::empty(source).unwrap())),
        7,
    )
    .unwrap();
    let b = different.affine_cell([1, 0, 0], || false).unwrap();
    assert!(matches!(
        a.shared_face(&b, 0, || false),
        Err(FieldError::Selection)
    ));
}
