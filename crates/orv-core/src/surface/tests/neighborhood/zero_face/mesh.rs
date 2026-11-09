// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

fn face(axis: usize, values: [[i16; 4]; 2]) -> SourceFace {
    let mut dimensions = [2; 3];
    dimensions[axis] = 3;
    let [u, v] = transverse(axis);
    let field = fractional(source(dimensions, true, false), |p| {
        if p[axis] == 1 {
            0
        } else {
            values[usize::from(p[axis] == 2)][[0, 1, 3, 2][p[u] + 2 * p[v]]]
        }
    });
    let mut origin = [0; 3];
    origin[axis] = 1;
    field
        .source_face(
            FaceKey {
                origin,
                axis: axis as u8,
            },
            || false,
        )
        .unwrap()
}
fn mesh(face: &SourceFace, budget: Arc<MemoryBudget>, bits: u8) -> ZeroFaceMesh {
    face.zero_mesh_with_budget(
        RefinementLimits {
            precision_bits: bits,
            ..Default::default()
        },
        TriangulationLimits::default(),
        budget,
        || false,
    )
    .unwrap()
    .unwrap()
}
fn lattice_point(mesh: &ZeroFaceMesh, id: u32) -> [f64; 3] {
    let [u, v] = transverse(usize::from(mesh.key().axis));
    let uv =
        mesh.geometry().subdivision().vertices()[id as usize].map(|c| c.approximate().unwrap());
    let mut p = mesh.key().origin.map(f64::from);
    p[u] += uv[0];
    p[v] += uv[1];
    p
}
fn check_patches(mesh: &ZeroFaceMesh) {
    let mut count = 0;
    for patch in mesh.patches() {
        let original = mesh.geometry().triangles()[patch.triangle as usize];
        assert_eq!(original.region, patch.region);
        assert_eq!(mesh.region_phase(patch.region), Some(patch.phase));
        let mut actual_ids = patch.vertices;
        let mut original_ids = original.vertices;
        actual_ids.sort();
        original_ids.sort();
        assert_eq!(actual_ids, original_ids);
        let [a, b, c] = patch.vertices.map(|id| lattice_point(mesh, id));
        let normal = cross(sub(b, a), sub(c, a));
        let sign = match patch.phase {
            ZeroFacePhase::Interface { normal } => f64::from(normal),
            ZeroFacePhase::Sheet => 1.,
            other => panic!("Unexpected drawable phase {other:?}"),
        };
        for (axis, n) in normal.into_iter().enumerate() {
            if axis == usize::from(mesh.key().axis) {
                assert!(n * sign > 0.);
            } else {
                assert_eq!(n, 0.);
            }
        }
        count += 1;
    }
    assert_eq!(count, mesh.patch_count());
    assert_eq!(
        count,
        mesh.geometry()
            .triangles()
            .iter()
            .filter(|t| { mesh.region_phase(t.region) != Some(ZeroFacePhase::Interior) })
            .count()
    );
}

#[test]
fn zero_face_mesh_constant_phases_keep_ties_and_signed_winding_on_every_axis() {
    for axis in 0..3 {
        let mut dimensions = [4; 3];
        dimensions[axis] = 3;
        let source = source(dimensions, true, false);
        let mut origin = [1; 3];
        origin[axis] = 1;
        let key = FaceKey {
            origin,
            axis: axis as u8,
        };
        for a in [-2i16, 0, 2] {
            for b in [-2i16, 0, 2] {
                let field = fractional(source.clone(), |p| match p[axis] {
                    0 => a,
                    1 => 0,
                    _ => b,
                });
                let face = field.source_face(key, || false).unwrap();
                let budget = MemoryBudget::new(32 * 1024 * 1024);
                let mesh = mesh(&face, budget.clone(), 4);
                assert!(Arc::ptr_eq(mesh.field().source(), &source));
                assert_eq!(mesh.field().key(), field.key());
                assert_eq!(mesh.field().revision(), field.revision());
                assert_eq!(mesh.key(), key);
                assert!(
                    mesh.cells()
                        .iter()
                        .all(|c| c.state() == CellNeighborhood::Known)
                );
                let phase = expected([a.signum() as i8, b.signum() as i8], false);
                assert_eq!(mesh.region_phase(0), Some(phase));
                assert_eq!(mesh.region_phase(255), None);
                assert_eq!(mesh.geometry().triangles().len(), 2);
                assert_eq!(
                    mesh.patch_count(),
                    if phase == ZeroFacePhase::Interior {
                        0
                    } else {
                        2
                    }
                );
                check_patches(&mesh);
                let MaskOwner::Supplied(map) = mesh.field().owner() else {
                    panic!("Supplied owner required")
                };
                assert_eq!(
                    map.mask(65535)
                        .unwrap()
                        .raw_value(origin.map(|v| v as usize)),
                    Some(100)
                );
                assert_eq!(
                    map.mask(65535).unwrap().value(origin.map(|v| v as usize)),
                    Some(true)
                );
                assert_eq!(budget.used(), mesh.referenced_bytes());
                drop(mesh);
                assert_eq!(budget.used(), 0);
            }
        }
    }
}

#[test]
fn zero_face_mesh_curves_keep_constraints_regions_and_independent_source_phases() {
    for axis in 0..3 {
        for values in [
            [[0, -2, 0, 2], [6, -58, 6, 38]],
            [[0, -2, 0, 2], [2, -14, 2, 10]],
            [[2, -4, 8, -4], [8, -4, 2, -4]],
            [[-2, -2, 4, 2], [2, -6, 2, -6]],
        ] {
            let face = face(axis, values);
            let budget = MemoryBudget::new(32 * 1024 * 1024);
            let mesh = mesh(&face, budget.clone(), 8);
            let refined = mesh.geometry().subdivision();
            let graph = refined.subdivision();
            assert_eq!(graph.fields().map(Bilinear::values), values);
            for (_, original) in face.contour().unwrap().nodes() {
                let id = graph
                    .nodes()
                    .find(|(_, n)| {
                        n.coordinates()
                            .iter()
                            .zip(original.coordinates())
                            .all(|(a, b)| a.compare(*b).unwrap() == std::cmp::Ordering::Equal)
                    })
                    .unwrap()
                    .0;
                assert!(
                    mesh.geometry()
                        .triangles()
                        .iter()
                        .any(|t| t.vertices.contains(&u32::from(id)))
                );
            }
            for (id, region) in graph.regions() {
                assert_eq!(mesh.region_phase(id), Some(expected(region.signs, false)));
                assert!(mesh.geometry().triangles().iter().any(|t| t.region == id));
            }
            check_patches(&mesh);
            let mut area = 0.;
            let mut stable = 0;
            for triangle in mesh.geometry().triangles() {
                let p = triangle
                    .vertices
                    .map(|id| refined.vertices()[id as usize].map(|c| c.approximate().unwrap()));
                let twice = (p[1][0] - p[0][0]) * (p[2][1] - p[0][1])
                    - (p[1][1] - p[0][1]) * (p[2][0] - p[0][0]);
                assert!(twice > 0.);
                area += twice / 2.;
                let [u, v] = std::array::from_fn::<_, 2, _>(|a| (p[0][a] + p[1][a] + p[2][a]) / 3.);
                let samples = values.map(|[a, b, c, d]| {
                    let [a, b, c, d] = [a, b, c, d].map(f64::from);
                    let value =
                        a * (1. - u) * (1. - v) + b * u * (1. - v) + c * u * v + d * (1. - u) * v;
                    let band = ((b - a).abs().max((c - d).abs())
                        + (d - a).abs().max((c - b).abs()))
                        / 256.;
                    (value.signum() as i8, value.abs() > band + 1e-10)
                });
                if samples.iter().all(|s| s.1) {
                    assert_eq!(
                        mesh.region_phase(triangle.region),
                        Some(expected(samples.map(|s| s.0), false))
                    );
                    stable += 1;
                }
            }
            assert!(stable > 0);
            assert!((area - 1.).abs() < 1e-12);
            assert_eq!(budget.used(), mesh.referenced_bytes());
            drop(mesh);
            assert_eq!(budget.used(), 0);
        }
    }
}

#[test]
fn zero_face_mesh_unproved_or_unavailable_faces_stay_open_without_admission() {
    for case in 0..6 {
        let field = match case {
            0 => fractional(source([2, 2, 2], false, false), |_| 0),
            1 => fractional(source([2, 2, 4], false, true), |p| {
                if p[2] == 2 { 0 } else { 2 }
            }),
            2 => fractional_slices(source([2, 2, 4], false, false), &[0, 2, 3], |p| {
                if p[2] == 2 { 0 } else { 2 }
            }),
            3 => fractional(source([2, 2, 3], false, false), |_| 2),
            4 => fractional(source([2, 2, 2], false, true), |_| 0),
            _ => fractional_slices(source([2, 2, 3], false, false), &[0, 2], |_| 0),
        };
        let key = FaceKey {
            origin: [
                0,
                0,
                if case == 1 || case == 2 {
                    2
                } else if case == 0 {
                    0
                } else {
                    1
                },
            ],
            axis: 2,
        };
        let face = field.source_face(key, || false).unwrap();
        let budget = MemoryBudget::new(0);
        assert!(
            face.zero_mesh_with_budget(
                RefinementLimits::default(),
                TriangulationLimits::default(),
                budget.clone(),
                || false
            )
            .unwrap()
            .is_none()
        );
        assert!(matches!(
            face.zero_mesh_with_budget(
                RefinementLimits::default(),
                TriangulationLimits::default(),
                budget.clone(),
                || true
            ),
            Err(SurfaceError::Cancelled)
        ));
        assert_eq!(budget.used(), 0);
    }
}

#[test]
fn zero_face_mesh_keeps_exact_source_and_revision_until_last_owner_drops() {
    let face = face(2, [[2; 4], [-2; 4]]);
    let budget = MemoryBudget::new(32 * 1024 * 1024);
    let mesh = mesh(&face, budget.clone(), 4);
    let saved_revision = mesh.field().revision().clone();
    let saved_key = mesh.field().key();
    let weak_source = Arc::downgrade(mesh.field().source());
    let MaskOwner::Supplied(map) = mesh.field().owner() else {
        panic!("Supplied owner required")
    };
    let weak_map = Arc::downgrade(map);
    let other = fractional(mesh.field().source().clone(), |p| {
        if p[2] == 1 {
            0
        } else if p[2] == 0 {
            2
        } else {
            -2
        }
    });
    assert_ne!(other.revision(), &saved_revision);
    assert_eq!(mesh.field().revision(), &saved_revision);
    assert_eq!(mesh.field().key(), saved_key);
    drop(other);
    drop(face);
    assert!(weak_source.upgrade().is_some());
    assert!(weak_map.upgrade().is_some());
    check_patches(&mesh);
    drop(mesh);
    assert!(weak_map.upgrade().is_none());
    assert!(weak_source.upgrade().is_none());
    assert_eq!(budget.used(), 0);
}

#[test]
fn zero_face_mesh_cancels_at_every_callback_without_replacing_prior_or_leaking() {
    let face = face(1, [[2; 4], [-2; 4]]);
    let budget = MemoryBudget::new(32 * 1024 * 1024);
    let calls = Cell::new(0usize);
    let prior = face
        .zero_mesh_with_budget(
            RefinementLimits::default(),
            TriangulationLimits::default(),
            budget.clone(),
            || {
                calls.set(calls.get() + 1);
                false
            },
        )
        .unwrap()
        .unwrap();
    let count = calls.get();
    let baseline = budget.used();
    let patches: Vec<_> = prior.patches().collect();
    let MaskOwner::Supplied(map) = prior.field().owner() else {
        panic!("Supplied owner required")
    };
    let owners = Arc::strong_count(map);
    for stop in 1..=count {
        calls.set(0);
        assert!(
            matches!(
                face.zero_mesh_with_budget(
                    RefinementLimits::default(),
                    TriangulationLimits::default(),
                    budget.clone(),
                    || {
                        calls.set(calls.get() + 1);
                        calls.get() == stop
                    }
                ),
                Err(SurfaceError::Cancelled)
            ),
            "stop {stop}/{count}"
        );
        assert_eq!(budget.used(), baseline);
        assert_eq!(prior.patches().collect::<Vec<_>>(), patches);
        assert_eq!(Arc::strong_count(map), owners);
    }
    assert_eq!(baseline, prior.referenced_bytes());
    drop(prior);
    assert_eq!(budget.used(), 0);
}

#[test]
fn zero_face_mesh_enforces_geometry_caps_and_all_admission_stages() {
    let face = face(2, [[0, -2, 0, 2], [6, -58, 6, 38]]);
    let budget = MemoryBudget::new(32 * 1024 * 1024);
    let prior = mesh(&face, budget.clone(), 4);
    let baseline = budget.used();
    let refinement = RefinementLimits {
        precision_bits: 4,
        max_vertices: prior.geometry().subdivision().vertices().len(),
        max_bytes: prior.geometry().subdivision().referenced_bytes(),
    };
    let triangulation = TriangulationLimits {
        max_triangles: prior.geometry().triangles().len(),
        max_bytes: prior.geometry().referenced_bytes(),
    };
    let exact = face
        .zero_mesh_with_budget(refinement, triangulation, budget.clone(), || false)
        .unwrap()
        .unwrap();
    assert_eq!(
        exact.patches().collect::<Vec<_>>(),
        prior.patches().collect::<Vec<_>>()
    );
    assert_eq!(budget.used(), baseline + exact.referenced_bytes());
    drop(exact);
    for (r, t) in [
        (
            RefinementLimits {
                max_vertices: refinement.max_vertices - 1,
                ..refinement
            },
            triangulation,
        ),
        (
            RefinementLimits {
                max_bytes: refinement.max_bytes - 1,
                ..refinement
            },
            triangulation,
        ),
        (
            RefinementLimits {
                precision_bits: 25,
                ..refinement
            },
            triangulation,
        ),
        (
            refinement,
            TriangulationLimits {
                max_triangles: triangulation.max_triangles - 1,
                ..triangulation
            },
        ),
        (
            refinement,
            TriangulationLimits {
                max_bytes: triangulation.max_bytes - 1,
                ..triangulation
            },
        ),
    ] {
        assert!(matches!(
            face.zero_mesh_with_budget(r, t, budget.clone(), || false),
            Err(SurfaceError::Limit)
        ));
        assert_eq!(budget.used(), baseline);
    }
    let metadata = (std::mem::size_of::<ZeroFaceMesh>()
        - std::mem::size_of::<TriangulatedJointSubdivision>()) as u64;
    let prep = (std::mem::size_of::<WholeZeroSubdivision>()
        + std::mem::size_of::<[[ExactCoordinate; 2]; JointSubdivision::MAX_INSERTIONS]>())
        as u64;
    for available in [
        0,
        metadata - 1,
        metadata,
        metadata + prep - 1,
        metadata + prep,
        prior.referenced_bytes() - 1,
    ] {
        budget.set_limit(baseline + available);
        assert!(matches!(
            face.zero_mesh_with_budget(refinement, triangulation, budget.clone(), || false),
            Err(SurfaceError::Memory)
        ));
        assert_eq!(budget.used(), baseline);
        check_patches(&prior);
    }
    drop(prior);
    assert_eq!(budget.used(), 0);
}
