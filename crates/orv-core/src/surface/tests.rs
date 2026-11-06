// SPDX-License-Identifier: GPL-3.0-or-later
mod field;
mod neighborhood;
mod ray;
use super::*;
use crate::{
    labels::Labels,
    mask::MaskView,
    segments::{FractionalEncoding, FractionalKind, Segments},
    study::{Geometry, NiftiMethod, NiftiTransform, demo_study},
    vector::*,
};
use std::collections::BTreeSet;

#[test]
fn shared_header_admission_includes_value_alignment() {
    #[repr(C)]
    struct SharedLayout<T> {
        strong: std::sync::atomic::AtomicUsize,
        weak: std::sync::atomic::AtomicUsize,
        value: T,
    }
    #[repr(align(32))]
    struct Aligned;
    assert_eq!(
        shared_header_bytes::<u8>(),
        std::mem::offset_of!(SharedLayout<u8>, value)
    );
    assert_eq!(
        shared_header_bytes::<u128>(),
        std::mem::offset_of!(SharedLayout<u128>, value)
    );
    assert_eq!(
        shared_header_bytes::<Aligned>(),
        std::mem::offset_of!(SharedLayout<Aligned>, value)
    );
}
fn source(dimensions: [usize; 3], shear: bool, missing: bool) -> Arc<Volume> {
    let template = demo_study().series.remove(0).frames.remove(0);
    let [w, h, d] = dimensions;
    Arc::new(
        Volume::new(
            (0..d)
                .map(|z| {
                    let mut f = (*template).clone();
                    f.width = w as u32;
                    f.height = h as u32;
                    f.pixels = vec![10.; w * h].into();
                    f.validity = missing.then(|| {
                        (0..w * h)
                            .map(|i| u8::from([i % w, i / w, z] != [1, 1, 1]))
                            .collect::<Vec<_>>()
                            .into()
                    });
                    f.geometry = Some(Geometry {
                        position: if shear {
                            [
                                500000. + z as f64 * 0.25,
                                -200000. + z as f64 * 0.5,
                                300000. + z as f64 * 2.,
                            ]
                        } else {
                            [0., 0., z as f64]
                        },
                        orientation: if shear {
                            [-1., 0., 0., 0., 1., 0.]
                        } else {
                            [1., 0., 0., 0., 1., 0.]
                        },
                        spacing: if shear { [2., 0.75] } else { [1., 1.] },
                        frame_of_reference: Some("generated cell surface".into()),
                        nifti_transform: None,
                    });
                    Arc::new(f)
                })
                .collect(),
        )
        .unwrap(),
    )
}
fn labels(source: Arc<Volume>, value: impl Fn([usize; 3]) -> u8) -> Arc<Labels> {
    let [w, h, _] = source.dimensions();
    let frames = source
        .frames()
        .iter()
        .enumerate()
        .map(|(z, f)| {
            let mut f = (**f).clone();
            f.validity = None;
            f.pixels = (0..w * h)
                .map(|i| f32::from(value([i % w, i / w, z])))
                .collect::<Vec<_>>()
                .into();
            f.geometry.as_mut().unwrap().nifti_transform = Some(NiftiTransform {
                method: NiftiMethod::Sform,
                code: 1,
            });
            Arc::new(f)
        })
        .collect();
    Arc::new(Labels::import(source, &Volume::new(frames).unwrap(), || false).unwrap())
}
fn build(
    map: MaskOwner,
    number: u16,
    previous: Option<&Arc<CellSurface>>,
) -> (Arc<CellSurface>, SurfaceUpdate) {
    CellSurface::build(map, number, previous, SurfaceLimits::default(), || false).unwrap()
}
fn faces(surface: &CellSurface) -> Vec<CellFace> {
    let mut result: Vec<_> = surface
        .chunks()
        .flat_map(|c| c.faces().iter().copied())
        .collect();
    result.sort();
    result
}
fn euler(surface: &CellSurface) -> isize {
    let mut vertices = BTreeSet::new();
    let mut edges = BTreeSet::new();
    for face in faces(surface) {
        let corners = face.corners_twice();
        for (a, b) in corners
            .into_iter()
            .zip(corners.into_iter().cycle().skip(1))
            .take(4)
        {
            vertices.insert(a);
            edges.insert(if a < b { [a, b] } else { [b, a] });
        }
    }
    vertices.len() as isize - edges.len() as isize + surface.face_count() as isize
}

#[test]
fn exact_faces_preserve_islands_diagonals_cavities_and_tunnels() {
    let source = source([5; 3], false, false);
    for (kind, expected_faces, expected_euler) in [
        (0, 6, 2),
        (1, 10, 2),
        (2, 12, 3),
        (3, 54, 2),
        (4, 60, 4),
        (5, 64, 0),
    ] {
        let map = labels(source.clone(), |p| {
            let cube = p.iter().all(|v| (1..=3).contains(v));
            u8::from(match kind {
                0 => p == [2; 3],
                1 => p == [2; 3] || p == [3, 2, 2],
                2 => p == [1; 3] || p == [2; 3],
                3 => cube,
                4 => cube && p != [2; 3],
                _ => cube && !(p[0] == 2 && p[1] == 2),
            })
        });
        let (surface, _) = build(MaskOwner::Manual(map), 1, None);
        assert_eq!(surface.face_count(), expected_faces, "kind {kind}");
        assert_eq!(euler(&surface), expected_euler, "kind {kind}");
        assert!(
            faces(&surface)
                .iter()
                .all(|f| f.boundary() == FaceBoundary::Mask)
        );
        assert_eq!(
            faces(&surface).into_iter().collect::<BTreeSet<_>>().len(),
            expected_faces
        );
    }
}

#[test]
fn physical_winding_normals_and_volume_respect_spacing_reflected_axes_and_shear() {
    for shear in [false, true] {
        let source = source([5; 3], shear, false);
        let map = labels(source.clone(), |p| {
            u8::from(p.iter().all(|v| (1..=3).contains(v)))
        });
        let (surface, _) = build(MaskOwner::Manual(map), 1, None);
        let mut volume = 0.;
        let relative = source.voxel_to_world([2.; 3]);
        for face in faces(&surface) {
            let g = surface.physical_face(&face).unwrap();
            assert!((norm(g.normal) - 1.).abs() < 1e-12);
            let cross = cross(
                sub(g.corners_mm[1], g.corners_mm[0]),
                sub(g.corners_mm[2], g.corners_mm[0]),
            );
            assert!(dot(cross, g.normal) > 0.);
            for p in g.corners_mm {
                let v = source.world_to_voxel(p);
                assert!(
                    (v[face.side().axis()]
                        - (f64::from(face.cell()[face.side().axis()])
                            + if face.side().positive() { 0.5 } else { -0.5 }))
                    .abs()
                        < 1e-8
                );
            }
            let offset = mul(g.normal, 0.01);
            let at = source.world_to_voxel(g.corners_mm[0]);
            let out = source.world_to_voxel(add(g.corners_mm[0], offset));
            assert!(
                (out[face.side().axis()] - at[face.side().axis()])
                    * if face.side().positive() { 1. } else { -1. }
                    > 0.
            );
            volume += dot(sub(g.corners_mm[0], relative), g.normal) * g.area_mm2 / 3.;
        }
        assert!((volume - 27. * source.voxel_volume_mm3()).abs() < 1e-7);
    }
}

#[test]
fn supplied_unknown_and_missing_neighbors_are_open_and_large_overlapping_ids_stay_separate() {
    for missing in [false, true] {
        let source = source([4, 4, 3], false, missing);
        let sparse = Arc::new(
            Segments::from_binary(
                source.clone(),
                &[9, 50000],
                &[(9, 1), (50000, 1)],
                &[255; 4],
                Some(true),
                || false,
            )
            .unwrap(),
        );
        let (a, _) = build(MaskOwner::Supplied(sparse.clone()), 9, None);
        let (b, _) = build(MaskOwner::Supplied(sparse.clone()), 50000, Some(&a));
        assert_ne!(a.key(), b.key());
        assert_eq!(b.key().number, 50000);
        assert_eq!(faces(&a), faces(&b));
        assert_eq!(a.face_count(), 16);
        assert!(
            faces(&a)
                .iter()
                .all(|f| f.boundary() == FaceBoundary::Grid && f.side().axis() != 2)
        );
        let complete = Arc::new(
            Segments::from_binary(
                source.clone(),
                &[9],
                &[(9, 0), (9, 1), (9, 2)],
                &[0, 0, 255, 255, 0, 0],
                None,
                || false,
            )
            .unwrap(),
        );
        let (closed, _) = build(MaskOwner::Supplied(complete), 9, Some(&a));
        assert_eq!(closed.face_count(), if missing { 46 } else { 48 });
        assert!(sparse.mask(9).unwrap().value([1, 1, 0]).is_none());
        assert!(sparse.mask(50000).unwrap().value([1, 1, 1]).unwrap());
    }
}

#[test]
fn fractional_surface_uses_declared_half_maximum_without_discarding_original_values() {
    let source = source([4, 4, 3], false, false);
    let mut payload = [99u8; 16];
    payload[5] = 100;
    for kind in [FractionalKind::Probability, FractionalKind::Occupancy] {
        let map = Arc::new(
            Segments::from_fractional(
                source.clone(),
                &[65535],
                &[(65535, 1)],
                &payload,
                FractionalEncoding { maximum: 200, kind },
                None,
                || false,
            )
            .unwrap(),
        );
        let (surface, _) = build(MaskOwner::Supplied(map.clone()), 65535, None);
        assert_eq!(
            surface.face_count(),
            4,
            "Unknown adjacent planes cannot cap the positive cell"
        );
        assert!(
            faces(&surface)
                .iter()
                .all(|f| f.cell() == [1, 1, 1] && f.side().axis() != 2)
        );
        assert_eq!(map.mask(65535).unwrap().raw_value([0, 0, 1]), Some(99));
        assert_eq!(map.mask(65535).unwrap().raw_value([1, 1, 1]), Some(100));
    }
}

#[test]
fn chunk_seams_and_incremental_edit_undo_match_a_fresh_full_reference() {
    let source = source([97; 3], false, false);
    let mut map = Labels::empty(source.clone()).unwrap();
    map.threshold(None, [0., 20.], 7, || false).unwrap();
    let original = Arc::new(map);
    let (first, full) = build(MaskOwner::Manual(original.clone()), 7, None);
    assert_eq!(full.rebuilt_chunks, 64);
    assert_eq!(first.face_count(), 6 * 97 * 97);
    let far = first.chunk([3; 3]).unwrap().clone();
    let mut edit = original.try_clone(|| false).unwrap();
    edit.paint(32, &[[31., 16.]], 1, 0, || false).unwrap();
    let edit = Arc::new(edit);
    let (changed, update) = build(MaskOwner::Manual(edit.clone()), 7, Some(&first));
    let (fresh, _) = build(MaskOwner::Manual(edit.clone()), 7, None);
    assert_eq!(update.rebuilt_chunks, 9);
    assert_eq!(update.reused_chunks, 55);
    assert!(update.counted_cells < 97 * 97 * 97 / 2);
    assert!(Arc::ptr_eq(&far, changed.chunk([3; 3]).unwrap()));
    assert_eq!(faces(&changed), faces(&fresh));
    assert_eq!(first.face_count(), 6 * 97 * 97);
    let (undo, update) = build(MaskOwner::Manual(original.clone()), 7, Some(&changed));
    assert_eq!(update.rebuilt_chunks, 9);
    assert!(Arc::ptr_eq(&far, undo.chunk([3; 3]).unwrap()));
    assert_eq!(faces(&undo), faces(&first));
    let (same, update) = build(MaskOwner::Manual(original), 7, Some(&undo));
    assert!(Arc::ptr_eq(&same, &undo));
    assert_eq!(update.allocated_bytes, 0);
    assert_eq!(update.counted_cells, 0);
    let (other_id, update) = build(MaskOwner::Manual(edit), 8, Some(&changed));
    assert_eq!(other_id.face_count(), 0);
    assert_eq!(update.rebuilt_chunks, 64);
}

#[test]
fn source_replacement_selection_limits_and_cancellation_preserve_previous_generations_and_admission()
 {
    let source = source([8, 8, 4], false, false);
    let map = labels(source.clone(), |p| u8::from((p[0] + p[1] + p[2]) % 2 == 0));
    let budget = MemoryBudget::new(MAX_BYTES);
    let build = |map, number, previous, limits, cancelled: &dyn Fn() -> bool| {
        CellSurface::with_budget(map, number, previous, limits, budget.clone(), cancelled)
    };
    let (first, _) = build(
        MaskOwner::Manual(map.clone()),
        1,
        None,
        SurfaceLimits::default(),
        &|| false,
    )
    .unwrap();
    let baseline = budget.used();
    assert_eq!(first.face_count(), 768);
    let limited = SurfaceLimits {
        max_faces: 5,
        ..Default::default()
    };
    assert!(matches!(
        build(
            MaskOwner::Manual(map.clone()),
            1,
            Some(&first),
            limited,
            &|| false
        ),
        Err(SurfaceError::Limit)
    ));
    assert!(matches!(
        build(
            MaskOwner::Manual(map.clone()),
            0,
            None,
            SurfaceLimits::default(),
            &|| false
        ),
        Err(SurfaceError::Selection)
    ));
    assert!(matches!(
        build(
            MaskOwner::Manual(map.clone()),
            256,
            None,
            SurfaceLimits::default(),
            &|| false
        ),
        Err(SurfaceError::Selection)
    ));
    let mut next = map.try_clone(|| false).unwrap();
    next.paint(1, &[[2., 2.]], 1, 9, || false).unwrap();
    let next = Arc::new(next);
    assert!(matches!(
        build(
            MaskOwner::Manual(next.clone()),
            1,
            Some(&first),
            limited,
            &|| false
        ),
        Err(SurfaceError::Limit)
    ));
    let scratch = (std::mem::size_of::<usize>() + 1 + walk::HALO_CELLS) as u64;
    budget.set_limit(baseline + scratch);
    assert!(
        matches!(
            build(
                MaskOwner::Manual(next.clone()),
                1,
                Some(&first),
                limited,
                &|| false
            ),
            Err(SurfaceError::Limit)
        ),
        "Counting must reject excess geometry before any output admission"
    );
    assert_eq!(budget.used(), baseline);
    budget.set_limit(MAX_BYTES);
    let directory = first.bytes - first.chunks().map(|c| c.bytes).sum::<u64>();
    let cancel_after_output_admission = || budget.used() > baseline + scratch + directory;
    assert!(matches!(
        build(
            MaskOwner::Manual(next.clone()),
            1,
            Some(&first),
            SurfaceLimits::default(),
            &cancel_after_output_admission
        ),
        Err(SurfaceError::Cancelled)
    ));
    assert_eq!(budget.used(), baseline);
    budget.set_limit(baseline + scratch + directory);
    assert!(matches!(
        build(
            MaskOwner::Manual(next.clone()),
            1,
            Some(&first),
            SurfaceLimits::default(),
            &|| false
        ),
        Err(SurfaceError::Memory)
    ));
    assert_eq!(budget.used(), baseline);
    budget.set_limit(0);
    let (same, _) = build(
        MaskOwner::Manual(map.clone()),
        1,
        Some(&first),
        SurfaceLimits::default(),
        &|| false,
    )
    .unwrap();
    assert!(Arc::ptr_eq(&same, &first));
    drop(same);
    assert!(matches!(
        build(
            MaskOwner::Manual(next),
            1,
            Some(&first),
            SurfaceLimits::default(),
            &|| false
        ),
        Err(SurfaceError::Memory)
    ));
    budget.set_limit(MAX_BYTES);
    let copied_source = Arc::new(Volume::new(source.frames().to_vec()).unwrap());
    let copied = labels(copied_source, |_| 1);
    let (replacement, update) = build(
        MaskOwner::Manual(copied),
        1,
        Some(&first),
        SurfaceLimits::default(),
        &|| false,
    )
    .unwrap();
    assert_eq!(update.reused_chunks, 0);
    assert_ne!(replacement.key(), first.key());
    assert!(!Arc::ptr_eq(replacement.source(), first.source()));
    drop(replacement);
    drop(first);
    assert_eq!(budget.used(), 0);
    assert_eq!(
        Layout::new([usize::MAX, 2, 2]).err(),
        Some(SurfaceError::Limit)
    );
    assert_eq!(
        Layout::new([2, 2, crate::labels::MAX_VOXELS / 4]).err(),
        Some(SurfaceError::Limit)
    );
    assert!(MaskOwner::Manual(map).view(0).is_err());
    assert_eq!(std::mem::size_of::<CellFace>(), 16);
}

#[test]
fn cell_face_set_agrees_with_an_independent_neighbor_scan_on_a_ragged_grid() {
    let source = source([67, 35, 34], false, true);
    let map = labels(source.clone(), |p| {
        u8::from((p[0] * 7 + p[1] * 11 + p[2] * 13) % 17 < 3)
    });
    let view = MaskView::manual(&map, 1).unwrap();
    let (surface, _) = build(MaskOwner::Manual(map.clone()), 1, None);
    let mut expected = BTreeSet::new();
    let dimensions = source.dimensions();
    for z in 0..dimensions[2] {
        for y in 0..dimensions[1] {
            for x in 0..dimensions[0] {
                let cell = [x, y, z];
                if !view.sample(cell).unwrap().eligible() {
                    continue;
                }
                for side in FaceSide::ALL {
                    let mut neighbor = cell;
                    let a = side.axis();
                    let n = neighbor[a].checked_add_signed(if side.positive() { 1 } else { -1 });
                    let boundary = if let Some(n) = n.filter(|n| *n < dimensions[a]) {
                        neighbor[a] = n;
                        let sample = view.sample(neighbor).unwrap();
                        if !sample.known || !sample.source_valid || sample.present {
                            continue;
                        }
                        FaceBoundary::Mask
                    } else {
                        FaceBoundary::Grid
                    };
                    expected.insert(CellFace {
                        cell: cell.map(|c| c as u32),
                        side,
                        boundary,
                    });
                }
            }
        }
    }
    assert_eq!(
        faces(&surface).into_iter().collect::<BTreeSet<_>>(),
        expected
    );
}
