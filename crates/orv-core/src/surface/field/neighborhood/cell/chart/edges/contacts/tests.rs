// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use std::collections::BTreeSet;

fn coordinate(n: i128) -> ExactCoordinate {
    ExactCoordinate::from_rational(Rational::new(n, 4).unwrap())
        .unwrap()
        .unwrap()
}
fn span(a: [i32; 2], b: [i32; 2]) -> Span {
    Span::new([
        a.map(|v| coordinate(i128::from(v))),
        b.map(|v| coordinate(i128::from(v))),
    ])
    .unwrap()
}
fn integer(p: [ExactCoordinate; 2]) -> [i32; 2] {
    p.map(|c| (c.rational_value().unwrap().as_f64() * 4.) as i32)
}
fn points(a: [i32; 2], b: [i32; 2]) -> BTreeSet<[i32; 2]> {
    (0..=4)
        .flat_map(|x| (0..=4).map(move |y| [x, y]))
        .filter(|p| {
            (0..2).all(|axis| p[axis] >= a[axis].min(b[axis]) && p[axis] <= a[axis].max(b[axis]))
        })
        .collect()
}
#[test]
fn axis_aligned_contacts_match_an_independent_integer_grid_oracle() {
    let coordinates: Vec<_> = (0..=4).flat_map(|x| (0..=4).map(move |y| [x, y])).collect();
    let mut spans = Vec::new();
    for &a in &coordinates {
        for &b in &coordinates {
            if a[0] == b[0] || a[1] == b[1] {
                spans.push((a, b, span(a, b), points(a, b)));
            }
        }
    }
    for (_, _, a, pa) in &spans {
        for (_, _, b, pb) in &spans {
            let expected: Vec<_> = pa.intersection(pb).copied().collect();
            let result = intersection(*a, *b).unwrap();
            match (expected.len(), result) {
                (0, None) => {}
                (1, Some(ProjectedContactGeometry::Point(p))) => {
                    assert_eq!(integer(p), expected[0])
                }
                (n, Some(ProjectedContactGeometry::Overlap([a, b]))) if n > 1 => {
                    let a = integer(a);
                    let b = integer(b);
                    assert_eq!(points(a, b), expected.into_iter().collect());
                }
                (n, _) => panic!("unexpected contact for {n} grid points"),
            }
        }
    }
}
#[test]
fn zero_field_slices_keep_point_overlap_and_empty_cases_distinct() {
    let field = Bilinear::new([-1, 1, 1, -1]).unwrap();
    let horizontal = span([0, 2], [4, 2]);
    let vertical = span([2, 0], [2, 4]);
    let disjoint = span([0, 0], [0, 4]);
    assert!(
        matches!(face_contact(horizontal, field).unwrap(), Some(ProjectedContactGeometry::Point(p)) if integer(p) == [2,2])
    );
    assert!(
        matches!(face_contact(vertical, field).unwrap(), Some(ProjectedContactGeometry::Overlap([a,b])) if integer(a)==[2,0] && integer(b)==[2,4])
    );
    assert!(face_contact(disjoint, field).unwrap().is_none());
    assert!(matches!(
        face_contact(span([2, 2], [2, 2]), field).unwrap(),
        Some(ProjectedContactGeometry::Point(_))
    ));
    assert!(face_contact(span([1, 2], [1, 2]), field).unwrap().is_none());
    assert!(matches!(
        face_contact(horizontal, Bilinear::new([0; 4]).unwrap()).unwrap(),
        Some(ProjectedContactGeometry::Overlap(_))
    ));
}
