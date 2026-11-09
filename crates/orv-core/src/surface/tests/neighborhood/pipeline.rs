// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

fn limits() -> CandidateGridLimits {
    CandidateGridLimits {
        max_cells: 256,
        max_directory_bytes: 1024 * 1024,
        graph: CellGraphLimits::default(),
        planar_refinement: PlanarRefinementLimits {
            curves: RefinementLimits {
                precision_bits: 6,
                max_vertices: 4096,
                ..RefinementLimits::default()
            },
            max_vertices: 4096,
            ..PlanarRefinementLimits::default()
        },
        triangulation: PlanarTriangulationLimits::default(),
        enclosure: SurfaceEnclosureLimits::default(),
        surface: PhysicalConformingLimits {
            max_error_mm: 0.02,
            max_depth: 4,
            max_vertices: 2048,
            max_triangles: 2048,
            max_operations: 2_000_000,
            max_bytes: ConformingSurfaceLimits::default().max_bytes,
        },
        seams: CellSeamLimits {
            partition: FacePartitionLimits {
                max_edges_per_cell: 1024,
                max_points: 2048,
                ..FacePartitionLimits::default()
            },
            ..CellSeamLimits::default()
        },
    }
}
fn grid_field() -> MaskField {
    fractional(source([3, 3, 2], true, false), |[x, y, z]| {
        if (x % 2 == 0 && y % 2 == 0 && z == 0) || (x == 1 && y == 1 && z == 1) {
            26
        } else {
            -6
        }
    })
}

fn assert_same_grid(actual: &CandidateGrid, expected: &CandidateGrid) {
    assert_eq!(actual.origin(), expected.origin());
    assert_eq!(actual.size(), expected.size());
    assert_eq!(actual.cells(), expected.cells());
    assert_eq!(actual.coordination(), expected.coordination());
    assert_eq!(actual.faces(), expected.faces());
    assert_eq!(actual.meshes().len(), expected.meshes().len());
    for (a, b) in actual.meshes().iter().zip(expected.meshes()) {
        assert_eq!(a.vertices(), b.vertices());
        assert_eq!(a.triangles(), b.triangles());
        assert_eq!(a.physical_tolerance_mm(), b.physical_tolerance_mm());
        for (id, vertex) in a.vertices().iter().enumerate() {
            if let ConformingVertex::FaceSplit { point, .. } = vertex {
                let (face_a, position_a) = a.face_split_source(*point).unwrap();
                let (face_b, position_b) = b.face_split_source(*point).unwrap();
                assert_eq!(face_a, face_b, "vertex {id}");
                assert_eq!(
                    position_a.compare(position_b).unwrap(),
                    std::cmp::Ordering::Equal
                );
            }
        }
    }
}

fn assembly_limits() -> CandidateAssemblyLimits {
    CandidateAssemblyLimits {
        max_cells: 256,
        max_directory_bytes: 1024 * 1024,
        seams: limits().seams,
    }
}

fn assert_assembly_matches_grid(actual: &CandidateAssembly, expected: &CandidateGrid) {
    assert_eq!(actual.cells(), expected.cells());
    assert_eq!(actual.coordination(), expected.coordination());
    assert_eq!(actual.faces(), expected.faces());
    assert_eq!(actual.meshes().len(), expected.meshes().len());
    for (a, b) in actual.meshes().iter().zip(expected.meshes()) {
        assert_eq!(a.vertices(), b.vertices());
        assert_eq!(a.triangles(), b.triangles());
        assert_eq!(a.physical_tolerance_mm(), b.physical_tolerance_mm());
        for vertex in a.vertices() {
            if let ConformingVertex::FaceSplit { point, .. } = vertex {
                let (face_a, coordinate_a) = a.face_split_source(*point).unwrap();
                let (face_b, coordinate_b) = b.face_split_source(*point).unwrap();
                assert_eq!(face_a, face_b);
                assert_eq!(
                    coordinate_a.compare(coordinate_b).unwrap(),
                    std::cmp::Ordering::Equal
                );
            }
        }
    }
}

#[test]
fn separate_boxes_coordinate_cross_box_faces_like_a_single_grid() {
    let field = grid_field();
    let budget = MemoryBudget::new(512 * 1024 * 1024);
    let boxes: Vec<_> = [[0, 0, 0], [1, 0, 0], [0, 1, 0], [1, 1, 0]]
        .into_iter()
        .map(|origin| {
            field
                .candidate_grid_with_budget(origin, [1; 3], 2, limits(), budget.clone(), || false)
                .unwrap()
        })
        .collect();
    assert!(
        boxes
            .iter()
            .all(|grid| grid.faces().is_empty() && grid.meshes().len() == 1)
    );
    let assembly = CandidateAssembly::build_with_budget(
        &[&boxes[3], &boxes[1], &boxes[2], &boxes[0]],
        assembly_limits(),
        budget.clone(),
        || false,
    )
    .unwrap();
    let single = field
        .candidate_grid_with_budget([0; 3], [2, 2, 1], 2, limits(), budget.clone(), || false)
        .unwrap();
    assert_eq!(assembly.faces().len(), 4);
    assert_assembly_matches_grid(&assembly, &single);
    drop((boxes, single));
    assert!(budget.used() > 0);
    assert_eq!(assembly.meshes().len(), 4);
    drop(assembly);
    assert_eq!(budget.used(), 0);
}

#[test]
fn edited_boxes_recoordinate_with_reused_and_rebuilt_neighbors() {
    let volume = source([8, 3, 2], true, false);
    let map = labels(volume, |p| u8::from(p == [3, 0, 0]));
    let before = MaskField::new(MaskOwner::Manual(map.clone()), 1).unwrap();
    let budget = MemoryBudget::new(512 * 1024 * 1024);
    let origins = [[0, 0, 0], [3, 0, 0]];
    let old: Vec<_> = origins
        .into_iter()
        .map(|origin| {
            before
                .candidate_grid_with_budget(origin, [3, 2, 1], 2, limits(), budget.clone(), || {
                    false
                })
                .unwrap()
        })
        .collect();
    let mut edited = map.try_clone(|| false).unwrap();
    edited.paint(1, &[[6., 1.]], 1, 1, || false).unwrap();
    let after = MaskField::new(MaskOwner::Manual(Arc::new(edited)), 1).unwrap();
    let updates: Vec<_> = origins
        .into_iter()
        .zip(&old)
        .map(|(origin, prior)| {
            after
                .candidate_grid_reusing_with_budget(
                    origin,
                    [3, 2, 1],
                    2,
                    limits(),
                    (Some(prior), budget.clone()),
                    || false,
                )
                .unwrap()
        })
        .collect();
    assert!(updates.iter().any(|(_, stats)| stats.reused_cells > 0));
    assert!(updates.iter().any(|(_, stats)| stats.rebuilt_cells > 0));
    assert!(matches!(
        CandidateAssembly::build_with_budget(
            &[&old[0], &updates[1].0],
            assembly_limits(),
            budget.clone(),
            || false,
        ),
        Err(SurfaceError::Geometry)
    ));
    let assembly = CandidateAssembly::build_with_budget(
        &[&updates[1].0, &updates[0].0],
        assembly_limits(),
        budget.clone(),
        || false,
    )
    .unwrap();
    let clean = after
        .candidate_grid_with_budget([0; 3], [6, 2, 1], 2, limits(), budget.clone(), || false)
        .unwrap();
    assert!(
        assembly
            .faces()
            .iter()
            .any(|face| face.face.axis == 0 && face.face.origin[0] == 3)
    );
    assert_assembly_matches_grid(&assembly, &clean);
    for mesh in assembly.meshes() {
        let field = mesh
            .input()
            .input()
            .input()
            .input()
            .input()
            .edges()
            .chart()
            .graph()
            .field();
        assert_eq!(field.revision(), after.revision());
    }
    drop((old, updates, assembly, clean));
    assert_eq!(budget.used(), 0);
}

#[test]
fn assembly_rejects_overlaps_and_incompatible_sources_or_settings() {
    let field = grid_field();
    let other = grid_field();
    let budget = MemoryBudget::new(512 * 1024 * 1024);
    let first = field
        .candidate_grid_with_budget([0; 3], [1; 3], 2, limits(), budget.clone(), || false)
        .unwrap();
    let second = field
        .candidate_grid_with_budget([1, 0, 0], [1; 3], 2, limits(), budget.clone(), || false)
        .unwrap();
    let other_source = other
        .candidate_grid_with_budget([1, 0, 0], [1; 3], 2, limits(), budget.clone(), || false)
        .unwrap();
    let other_axis = field
        .candidate_grid_with_budget([1, 0, 0], [1; 3], 1, limits(), budget.clone(), || false)
        .unwrap();
    let mut different = limits();
    different.surface.max_error_mm = 0.03;
    let other_settings = field
        .candidate_grid_with_budget([1, 0, 0], [1; 3], 2, different, budget.clone(), || false)
        .unwrap();
    for pair in [
        [&first, &first],
        [&first, &other_source],
        [&first, &other_axis],
        [&first, &other_settings],
    ] {
        assert!(matches!(
            CandidateAssembly::build_with_budget(&pair, assembly_limits(), budget.clone(), || {
                false
            }),
            Err(SurfaceError::Geometry)
        ));
    }
    let other_budget = MemoryBudget::new(512 * 1024 * 1024);
    assert!(matches!(
        CandidateAssembly::build_with_budget(
            &[&first, &second],
            assembly_limits(),
            other_budget.clone(),
            || false,
        ),
        Err(SurfaceError::Geometry)
    ));
    assert!(matches!(
        CandidateAssembly::build_with_budget(&[], assembly_limits(), budget.clone(), || false,),
        Err(SurfaceError::Limit)
    ));
    let mut small = assembly_limits();
    small.max_cells = 1;
    assert!(matches!(
        CandidateAssembly::build_with_budget(&[&first, &second], small, budget.clone(), || false,),
        Err(SurfaceError::Limit)
    ));
    small = assembly_limits();
    small.max_directory_bytes = 0;
    assert!(matches!(
        CandidateAssembly::build_with_budget(&[&first, &second], small, budget.clone(), || false,),
        Err(SurfaceError::Limit)
    ));
    drop((first, second, other_source, other_axis, other_settings));
    assert_eq!(budget.used(), 0);
    assert_eq!(other_budget.used(), 0);
}

#[test]
fn assembly_is_atomic_under_limits_cancellation_and_memory_pressure() {
    let field = grid_field();
    let budget = MemoryBudget::new(512 * 1024 * 1024);
    let left = field
        .candidate_grid_with_budget([0; 3], [1, 2, 1], 2, limits(), budget.clone(), || false)
        .unwrap();
    let right = field
        .candidate_grid_with_budget([1, 0, 0], [1, 2, 1], 2, limits(), budget.clone(), || false)
        .unwrap();
    let used = budget.used();
    let mut constrained = assembly_limits();
    constrained.seams.max_faces = 0;
    let incomplete =
        CandidateAssembly::build_with_budget(&[&left, &right], constrained, budget.clone(), || {
            false
        })
        .unwrap();
    assert_eq!(
        incomplete.coordination(),
        GridCoordination::Unresolved {
            reason: SurfaceError::Limit
        }
    );
    assert_eq!(incomplete.meshes().len(), 4);
    assert!(incomplete.faces().is_empty());
    drop(incomplete);
    assert_eq!(budget.used(), used);
    let calls = Cell::new(0usize);
    let assembly = CandidateAssembly::build_with_budget(
        &[&left, &right],
        assembly_limits(),
        budget.clone(),
        || {
            calls.set(calls.get() + 1);
            false
        },
    )
    .unwrap();
    let total = calls.get();
    drop(assembly);
    assert_eq!(budget.used(), used);
    for stop in [1, total / 2, total] {
        let calls = Cell::new(0usize);
        assert!(matches!(
            CandidateAssembly::build_with_budget(
                &[&left, &right],
                assembly_limits(),
                budget.clone(),
                || {
                    calls.set(calls.get() + 1);
                    calls.get() >= stop
                }
            ),
            Err(SurfaceError::Cancelled)
        ));
        assert_eq!(budget.used(), used);
    }
    budget.set_limit(used);
    assert!(matches!(
        CandidateAssembly::build_with_budget(
            &[&left, &right],
            assembly_limits(),
            budget.clone(),
            || false,
        ),
        Err(SurfaceError::Memory)
    ));
    assert_eq!(budget.used(), used);
    assert_eq!(left.meshes().len(), 2);
    assert_eq!(right.meshes().len(), 2);
    drop((left, right));
    assert_eq!(budget.used(), 0);
}

#[test]
fn assembly_keeps_noncandidate_cells_and_their_uncertainty() {
    let volume = source([4, 2, 2], false, true);
    let field = fractional(volume, |_| -6);
    let budget = MemoryBudget::new(512 * 1024 * 1024);
    let left = field
        .candidate_grid_with_budget([0; 3], [1; 3], 2, limits(), budget.clone(), || false)
        .unwrap();
    let right = field
        .candidate_grid_with_budget([1, 0, 0], [2, 1, 1], 2, limits(), budget.clone(), || false)
        .unwrap();
    let assembly = CandidateAssembly::build_with_budget(
        &[&right, &left],
        assembly_limits(),
        budget.clone(),
        || false,
    )
    .unwrap();
    let clean = field
        .candidate_grid_with_budget([0; 3], [3, 1, 1], 2, limits(), budget.clone(), || false)
        .unwrap();
    assert_eq!(assembly.coordination(), GridCoordination::NoCandidates);
    assert!(assembly.cells().iter().any(|cell| matches!(
        cell.state,
        CandidateCellState::Graph(CellGraphState::Unavailable { .. })
    )));
    assert_assembly_matches_grid(&assembly, &clean);
    drop((left, right, assembly, clean));
    assert_eq!(budget.used(), 0);
}

fn same_neighbor_samples(old: &MaskField, new: &MaskField, lower: [usize; 3]) -> bool {
    let dimensions = old.source().dimensions();
    (-1isize..=2).all(|z| {
        (-1isize..=2).all(|y| {
            (-1isize..=2).all(|x| {
                let mut position = [0usize; 3];
                for (axis, delta) in [x, y, z].into_iter().enumerate() {
                    let Some(value) = lower[axis].checked_add_signed(delta) else {
                        return true;
                    };
                    if value >= dimensions[axis] {
                        return true;
                    }
                    position[axis] = value;
                }
                old.sample(position) == new.sample(position)
            })
        })
    })
}

#[test]
fn source_graph_reads_stay_within_the_checked_neighborhood() {
    let mut saw_incident_neighbor = false;
    for missing in [false, true] {
        let volume = source([5; 3], true, missing);
        for seed in 0u32..128 {
            let field = fractional(volume.clone(), |[x, y, z]| {
                if seed == 0 {
                    return if [x, y, z] == [2; 3] { 26 } else { -6 };
                }
                let mut hash = (x as u32).wrapping_mul(0x9e37_79b9)
                    ^ (y as u32).wrapping_mul(0x85eb_ca6b)
                    ^ (z as u32).wrapping_mul(0xc2b2_ae35)
                    ^ seed.wrapping_mul(0x27d4_eb2d);
                hash ^= hash >> 16;
                hash = hash.wrapping_mul(0x7feb_352d);
                hash ^= hash >> 15;
                hash = hash.wrapping_mul(0x846c_a68b);
                hash ^= hash >> 16;
                if hash & 1 == 0 { 26 } else { -6 }
            });
            let (_, reads) = crate::surface::field::capture_field_reads(|| {
                field.cell_graph([2; 3], CellGraphLimits::default(), || false)
            });
            assert!(!reads.is_empty());
            for point in reads {
                assert!(
                    point.into_iter().all(|v| (1..=4).contains(&v)),
                    "seed {seed}: {point:?}"
                );
                saw_incident_neighbor |= point.into_iter().any(|v| v == 1 || v == 4);
            }
        }
    }
    assert!(
        saw_incident_neighbor,
        "fixture must exercise face and incident-edge reads"
    );
}

#[test]
fn single_voxel_edits_only_reuse_candidates_outside_the_source_neighborhood() {
    use crate::labels::{BrushAction, BrushShape, PhysicalBrush};
    let volume = source([6; 3], true, false);
    let map = labels(volume.clone(), |p| u8::from(p == [2, 2, 2]));
    let original = MaskField::new(MaskOwner::Manual(map.clone()), 1).unwrap();
    let budget = MemoryBudget::new(512 * 1024 * 1024);
    let old = original
        .candidate_grid_with_budget([2; 3], [1; 3], 2, limits(), budget.clone(), || false)
        .unwrap();
    assert_eq!(old.meshes().len(), 1);
    for z in 0..6 {
        for y in 0..6 {
            for x in 0..6 {
                let voxel = [x, y, z];
                let settings = PhysicalBrush {
                    label: 1,
                    action: if voxel == [2; 3] {
                        BrushAction::Erase
                    } else {
                        BrushAction::Add
                    },
                    radius_mm: 0.05,
                    shape: BrushShape::Sphere,
                    clip: None,
                    protected_ids: [false; 256],
                };
                let world = volume.voxel_to_world(voxel.map(|v| v as f64));
                let stroke = map
                    .propose_physical_stroke(&settings, &[world], || false)
                    .unwrap();
                assert_eq!(stroke.summary().changed_voxels, 1, "{voxel:?}");
                let changed =
                    MaskField::new(MaskOwner::Manual(stroke.preview().clone()), 1).unwrap();
                let (updated, stats) = changed
                    .candidate_grid_reusing_with_budget(
                        [2; 3],
                        [1; 3],
                        2,
                        limits(),
                        (Some(&old), budget.clone()),
                        || false,
                    )
                    .unwrap();
                let inside = voxel.into_iter().all(|v| (1..=4).contains(&v));
                assert_eq!(stats.reused_cells, usize::from(!inside), "{voxel:?}");
                let clean = changed
                    .candidate_grid_with_budget([2; 3], [1; 3], 2, limits(), budget.clone(), || {
                        false
                    })
                    .unwrap();
                assert_same_grid(&updated, &clean);
                for mesh in updated.meshes() {
                    let field = mesh
                        .input()
                        .input()
                        .input()
                        .input()
                        .input()
                        .edges()
                        .chart()
                        .graph()
                        .field();
                    assert_eq!(field.revision(), changed.revision());
                }
            }
        }
    }
    drop(old);
    assert_eq!(budget.used(), 0);
}

#[test]
fn rebound_candidate_is_atomic_and_does_not_retain_the_old_mask_owner() {
    let volume = source([8, 3, 2], true, false);
    let map = labels(volume, |p| u8::from(p == [0; 3]));
    let weak_old = Arc::downgrade(&map);
    let original = MaskField::new(MaskOwner::Manual(map.clone()), 1).unwrap();
    let mut edited = map.try_clone(|| false).unwrap();
    edited.paint(1, &[[6., 1.]], 1, 1, || false).unwrap();
    let changed = MaskField::new(MaskOwner::Manual(Arc::new(edited)), 1).unwrap();
    let budget = MemoryBudget::new(512 * 1024 * 1024);
    let old = original
        .candidate_grid_with_budget([0; 3], [1; 3], 2, limits(), budget.clone(), || false)
        .unwrap();
    assert_eq!(old.meshes().len(), 1);
    let used = budget.used();
    let calls = Cell::new(0usize);
    let (sample, stats) = changed
        .candidate_grid_reusing_with_budget(
            [0; 3],
            [1; 3],
            2,
            limits(),
            (Some(&old), budget.clone()),
            || {
                calls.set(calls.get() + 1);
                false
            },
        )
        .unwrap();
    assert_eq!(stats.reused_cells, 1);
    let total = calls.get();
    drop(sample);
    assert_eq!(budget.used(), used);
    for stop in [1, total / 2, total] {
        let calls = Cell::new(0usize);
        assert!(matches!(
            changed.candidate_grid_reusing_with_budget(
                [0; 3],
                [1; 3],
                2,
                limits(),
                (Some(&old), budget.clone()),
                || {
                    calls.set(calls.get() + 1);
                    calls.get() >= stop
                }
            ),
            Err(SurfaceError::Cancelled)
        ));
        assert_eq!(budget.used(), used);
    }
    budget.set_limit(used + old.referenced_bytes() + 1);
    assert!(matches!(
        changed.candidate_grid_reusing_with_budget(
            [0; 3],
            [1; 3],
            2,
            limits(),
            (Some(&old), budget.clone()),
            || false,
        ),
        Err(SurfaceError::Memory)
    ));
    assert_eq!(budget.used(), used);
    budget.set_limit(512 * 1024 * 1024);
    let (updated, stats) = changed
        .candidate_grid_reusing_with_budget(
            [0; 3],
            [1; 3],
            2,
            limits(),
            (Some(&old), budget.clone()),
            || false,
        )
        .unwrap();
    assert_eq!(stats.reused_cells, 1);
    drop((old, original, map));
    assert!(weak_old.upgrade().is_none());
    assert!(budget.used() > 0);
    assert_eq!(updated.meshes().len(), 1);
    drop(updated);
    assert_eq!(budget.used(), 0);
}

#[test]
fn overlapping_grid_reuses_independent_cells_and_recoordinates_like_a_clean_build() {
    let field = fractional(source([4, 3, 2], true, false), |[x, y, z]| {
        if (x % 2 == 0 && y % 2 == 0 && z == 0) || (x % 2 == 1 && y == 1 && z == 1) {
            26
        } else {
            -6
        }
    });
    let budget = MemoryBudget::new(512 * 1024 * 1024);
    let old = field
        .candidate_grid_with_budget([0; 3], [2, 2, 1], 2, limits(), budget.clone(), || false)
        .unwrap();
    assert_eq!(old.meshes().len(), 4);
    assert_eq!(old.faces().len(), 4);
    let (shifted, update) = field
        .candidate_grid_reusing_with_budget(
            [1, 0, 0],
            [2, 2, 1],
            2,
            limits(),
            (Some(&old), budget.clone()),
            || false,
        )
        .unwrap();
    assert_eq!(
        update,
        CandidateGridUpdate {
            reused_cells: 2,
            rebuilt_cells: 2
        }
    );
    let clean = field
        .candidate_grid_with_budget([1, 0, 0], [2, 2, 1], 2, limits(), budget.clone(), || false)
        .unwrap();
    assert_same_grid(&shifted, &clean);
    let (again, update) = field
        .candidate_grid_reusing_with_budget(
            [1, 0, 0],
            [2, 2, 1],
            2,
            limits(),
            (Some(&shifted), budget.clone()),
            || false,
        )
        .unwrap();
    assert_eq!(
        update,
        CandidateGridUpdate {
            reused_cells: 4,
            rebuilt_cells: 0
        }
    );
    assert_same_grid(&again, &clean);
    // A single-cell box needs no face promotion, so its published mesh is the
    // independent retained candidate and can be checked for actual sharing.
    let single = field
        .candidate_grid_with_budget([1, 0, 0], [1; 3], 2, limits(), budget.clone(), || false)
        .unwrap();
    let (same, update) = field
        .candidate_grid_reusing_with_budget(
            [1, 0, 0],
            [1; 3],
            2,
            limits(),
            (Some(&single), budget.clone()),
            || false,
        )
        .unwrap();
    assert_eq!(update.reused_cells, 1);
    assert!(Arc::ptr_eq(&single.meshes()[0], &same.meshes()[0]));
    drop((old, shifted, clean, again, single, same));
    assert_eq!(budget.used(), 0);
}

#[test]
fn reuse_rejects_different_selection_source_settings_and_budget() {
    let volume = source([3, 3, 2], true, false);
    let map = labels(
        volume.clone(),
        |[x, y, z]| if (x + y + z) % 2 == 0 { 1 } else { 2 },
    );
    let other_map = labels(volume, |[x, y, z]| if (x + y + z) % 2 == 0 { 1 } else { 2 });
    let field = MaskField::new(MaskOwner::Manual(map.clone()), 1).unwrap();
    let budget = MemoryBudget::new(512 * 1024 * 1024);
    let old = field
        .candidate_grid_with_budget([0; 3], [2, 2, 1], 2, limits(), budget.clone(), || false)
        .unwrap();
    let other_id = MaskField::new(MaskOwner::Manual(map), 2).unwrap();
    let other_object = MaskField::new(MaskOwner::Manual(other_map), 1).unwrap();
    let other_source = fractional(source([3, 3, 2], true, false), |_| -6);
    let other_budget = MemoryBudget::new(512 * 1024 * 1024);
    let mut other_limits = limits();
    other_limits.surface.max_error_mm = 0.03;
    for (new_field, axis, options, allocation) in [
        (&other_id, 2, limits(), budget.clone()),
        (&other_object, 2, limits(), budget.clone()),
        (&other_source, 2, limits(), budget.clone()),
        (&field, 1, limits(), budget.clone()),
        (&field, 2, other_limits, budget.clone()),
        (&field, 2, limits(), other_budget.clone()),
    ] {
        let (actual, update) = new_field
            .candidate_grid_reusing_with_budget(
                [0; 3],
                [2, 2, 1],
                axis,
                options,
                (Some(&old), allocation.clone()),
                || false,
            )
            .unwrap();
        assert_eq!(
            update,
            CandidateGridUpdate {
                reused_cells: 0,
                rebuilt_cells: 4
            }
        );
        let clean = new_field
            .candidate_grid_with_budget([0; 3], [2, 2, 1], axis, options, allocation, || false)
            .unwrap();
        assert_same_grid(&actual, &clean);
    }
    drop(old);
    assert_eq!(budget.used(), 0);
    assert_eq!(other_budget.used(), 0);
}

#[test]
fn changed_revision_reuses_only_identical_cell_dependencies() {
    for (missing, family) in [(false, false), (true, false), (false, true), (true, true)] {
        let volume = source([8, 3, 2], true, missing);
        let map = labels(volume, |[x, y, z]| {
            u8::from(if family {
                x < 3
            } else {
                [x, y, z] == [3, 0, 0]
            })
        });
        let before = MaskField::new(MaskOwner::Manual(map.clone()), 1).unwrap();
        let mut edited = map.try_clone(|| false).unwrap();
        edited.paint(1, &[[6., 1.]], 1, 1, || false).unwrap();
        let after = MaskField::new(MaskOwner::Manual(Arc::new(edited)), 1).unwrap();
        assert_ne!(before.revision(), after.revision());
        let budget = MemoryBudget::new(512 * 1024 * 1024);
        let old = before
            .candidate_grid_with_budget([0; 3], [7, 2, 1], 2, limits(), budget.clone(), || false)
            .unwrap();
        if family {
            assert!(old.cells().iter().any(|cell| matches!(
                cell.state,
                CandidateCellState::Graph(CellGraphState::Family { .. })
            )));
        } else {
            assert!(
                !old.meshes().is_empty(),
                "the fixture needs a candidate mesh"
            );
            assert!(
                !old.faces().is_empty(),
                "the fixture needs adjacent candidates"
            );
            assert!(
                old.cells().iter().any(|cell| matches!(
                    cell.state,
                    CandidateCellState::Candidate { .. }
                ) && !same_neighbor_samples(
                    &before,
                    &after,
                    cell.origin
                )),
                "one adjacent candidate must rebuild"
            );
        }
        if missing {
            assert!(old.cells().iter().any(|cell| matches!(
                cell.state,
                CandidateCellState::Graph(CellGraphState::Unavailable { .. })
            )));
        }
        let expected_reused = old
            .cells()
            .iter()
            .filter(|cell| match cell.state {
                CandidateCellState::Graph(
                    CellGraphState::Unavailable { .. }
                    | CellGraphState::NoInteriorInterface
                    | CellGraphState::Family { .. },
                ) => before.cell(cell.origin).unwrap() == after.cell(cell.origin).unwrap(),
                CandidateCellState::Candidate { .. } => {
                    same_neighbor_samples(&before, &after, cell.origin)
                }
                _ => false,
            })
            .count();
        assert!(expected_reused > 0 && expected_reused < old.cells().len());
        if !family {
            assert!(
                old.cells().iter().any(|cell| matches!(
                    cell.state,
                    CandidateCellState::Candidate { .. }
                ) && same_neighbor_samples(
                    &before,
                    &after,
                    cell.origin
                )),
                "the fixture needs a reusable mesh"
            );
        }
        let (incremental, stats) = after
            .candidate_grid_reusing_with_budget(
                [0; 3],
                [7, 2, 1],
                2,
                limits(),
                (Some(&old), budget.clone()),
                || false,
            )
            .unwrap();
        assert_eq!(stats.reused_cells, expected_reused);
        assert_eq!(stats.rebuilt_cells, old.cells().len() - expected_reused);
        let clean = after
            .candidate_grid_with_budget([0; 3], [7, 2, 1], 2, limits(), budget.clone(), || false)
            .unwrap();
        assert_same_grid(&incremental, &clean);
        for mesh in incremental.meshes() {
            let field = mesh
                .input()
                .input()
                .input()
                .input()
                .input()
                .edges()
                .chart()
                .graph()
                .field();
            assert_eq!(field.revision(), after.revision());
        }
        // Returning to the earlier immutable revision models undo; compare the
        // result with a fresh rebuild and check that its meshes retain that revision.
        let (undone, _) = before
            .candidate_grid_reusing_with_budget(
                [0; 3],
                [7, 2, 1],
                2,
                limits(),
                (Some(&incremental), budget.clone()),
                || false,
            )
            .unwrap();
        let clean_undo = before
            .candidate_grid_with_budget([0; 3], [7, 2, 1], 2, limits(), budget.clone(), || false)
            .unwrap();
        assert_same_grid(&undone, &clean_undo);
        for mesh in undone.meshes() {
            let field = mesh
                .input()
                .input()
                .input()
                .input()
                .input()
                .edges()
                .chart()
                .graph()
                .field();
            assert_eq!(field.revision(), before.revision());
        }
        let (redone, _) = after
            .candidate_grid_reusing_with_budget(
                [0; 3],
                [7, 2, 1],
                2,
                limits(),
                (Some(&undone), budget.clone()),
                || false,
            )
            .unwrap();
        assert_same_grid(&redone, &clean);
        drop((old, incremental, clean, undone, clean_undo, redone));
        assert_eq!(budget.used(), 0);
    }
}

#[test]
fn reused_grid_cancellation_keeps_the_old_generation_and_releases_new_admission() {
    let field = grid_field();
    let budget = MemoryBudget::new(512 * 1024 * 1024);
    let old = field
        .candidate_grid_with_budget([0; 3], [2, 2, 1], 2, limits(), budget.clone(), || false)
        .unwrap();
    let used = budget.used();
    let calls = Cell::new(0);
    assert!(matches!(
        field.candidate_grid_reusing_with_budget(
            [0; 3],
            [2, 2, 1],
            2,
            limits(),
            (Some(&old), budget.clone()),
            || {
                calls.set(calls.get() + 1);
                calls.get() == 8
            }
        ),
        Err(SurfaceError::Cancelled)
    ));
    assert_eq!(budget.used(), used);
    assert_eq!(old.cells().len(), 4);
    budget.set_limit(used);
    assert!(matches!(
        field.candidate_grid_reusing_with_budget(
            [0; 3],
            [2, 2, 1],
            2,
            limits(),
            (Some(&old), budget.clone()),
            || false
        ),
        Err(SurfaceError::Memory)
    ));
    assert_eq!(budget.used(), used);
    assert_eq!(old.cells().len(), 4);
    drop(old);
    assert_eq!(budget.used(), 0);
}

#[test]
fn candidate_grid_builds_source_bound_cells_and_reports_each_shared_face() {
    let field = grid_field();
    let budget = MemoryBudget::new(256 * 1024 * 1024);
    let grid = field
        .candidate_grid_with_budget([0; 3], [2, 2, 1], 2, limits(), budget.clone(), || false)
        .unwrap();
    assert!(Arc::ptr_eq(grid.field().source(), field.source()));
    assert_eq!(grid.field().key(), field.key());
    assert_eq!(grid.field().revision(), field.revision());
    assert_eq!(grid.origin(), [0; 3]);
    assert_eq!(grid.size(), [2, 2, 1]);
    assert_eq!(grid.projection_axis(), 2);
    assert_eq!(grid.cells().len(), 4);
    assert_eq!(grid.coordination(), GridCoordination::Coordinated);
    assert_eq!(grid.faces().len(), 4);
    assert_eq!(grid.meshes().len(), 4);
    for (index, cell) in grid.cells().iter().enumerate() {
        assert_eq!(cell.origin, [index % 2, index / 2, 0]);
        let CandidateCellState::Candidate { mesh } = cell.state else {
            panic!("cell {index} was not a candidate: {:?}", cell.state)
        };
        let mesh = &grid.meshes()[usize::from(mesh)];
        assert_eq!(mesh.physical_tolerance_mm(), Some(0.02));
        let chart = mesh.input().input().input().input().input().edges().chart();
        assert_eq!(chart.graph().cell().origin(), cell.origin.map(|v| v as u32));
        assert!(Arc::ptr_eq(chart.graph().field().source(), field.source()));
        for (id, triangle) in mesh.triangles().iter().enumerate() {
            if matches!(triangle.state, ConformingFacetState::WithinTolerance { .. }) {
                assert!(
                    mesh.triangle_physical_error_upper_mm(id as u32)
                        .unwrap()
                        .is_some_and(|v| v <= 0.02)
                );
            }
        }
    }
    for report in grid.faces() {
        if matches!(report.state, CellFaceState::Unresolved { .. }) {
            continue;
        }
        let [a, b] = report.cells.map(usize::from);
        let partition = grid.meshes()[a]
            .shared_face_partition_with_budget(
                &grid.meshes()[b],
                usize::from(report.face.axis),
                limits().seams.partition,
                budget.clone(),
                || false,
            )
            .unwrap();
        assert!(partition.points().iter().all(|p| {
            p.placements
                .iter()
                .all(|v| !matches!(v, FacePlacement::Split { .. }))
        }));
    }
    let retained = grid.meshes()[0].clone();
    let nested = retained.input().clone();
    drop(grid);
    assert!(budget.used() > 0);
    drop(retained);
    assert!(budget.used() >= nested.referenced_bytes());
    let triangulated = nested.input().clone();
    drop(nested);
    assert!(budget.used() >= triangulated.referenced_bytes());
    let refined = triangulated.input().clone();
    drop(triangulated);
    assert!(budget.used() >= refined.referenced_bytes());
    drop(refined);
    assert_eq!(budget.used(), 0);
}

#[test]
fn candidate_grid_keeps_unavailable_empty_family_and_stage_limits_explicit() {
    let budget = MemoryBudget::new(256 * 1024 * 1024);
    for (values, missing) in [
        ([-2; 8], false),
        ([0; 8], false),
        ([26, -6, -6, -6, -6, -6, -6, 26], true),
    ] {
        let field = fractional(source([2; 3], true, missing), |[x, y, z]| {
            values[x + 2 * y + 4 * z]
        });
        let expected = *field
            .cell_graph_with_budget([0; 3], CellGraphLimits::default(), budget.clone(), || false)
            .unwrap()
            .state();
        assert_ne!(expected, CellGraphState::Finite);
        let grid = field
            .candidate_grid_with_budget([0; 3], [1; 3], 2, limits(), budget.clone(), || false)
            .unwrap();
        assert_eq!(grid.cells()[0].state, CandidateCellState::Graph(expected));
        assert_eq!(grid.coordination(), GridCoordination::NoCandidates);
        assert!(grid.meshes().is_empty());
        assert!(grid.faces().is_empty());
    }
    assert_eq!(budget.used(), 0);
    let field = grid_field();
    for stage in [
        PipelineStage::PlanarRefinement,
        PipelineStage::Triangulation,
        PipelineStage::Enclosure,
        PipelineStage::SurfaceRefinement,
    ] {
        let mut constrained = limits();
        match stage {
            PipelineStage::PlanarRefinement => constrained.planar_refinement.max_vertices = 0,
            PipelineStage::Triangulation => constrained.triangulation.max_triangles = 0,
            PipelineStage::Enclosure => constrained.enclosure.precision_bits = 17,
            PipelineStage::SurfaceRefinement => constrained.surface.max_vertices = 0,
            _ => unreachable!(),
        }
        let grid = field
            .candidate_grid_with_budget([0; 3], [1; 3], 2, constrained, budget.clone(), || false)
            .unwrap();
        assert_eq!(
            grid.cells()[0].state,
            CandidateCellState::Unresolved {
                stage,
                reason: SurfaceError::Limit
            }
        );
        assert_eq!(grid.coordination(), GridCoordination::NoCandidates);
        drop(grid);
        assert_eq!(budget.used(), 0);
    }
    let mut constrained = limits();
    constrained.graph.max_bytes = 0;
    let grid = field
        .candidate_grid_with_budget([0; 3], [2, 2, 1], 2, constrained, budget.clone(), || false)
        .unwrap();
    assert!(grid.cells().iter().all(|cell| cell.state
        == CandidateCellState::Unresolved {
            stage: PipelineStage::Graph,
            reason: SurfaceError::Limit
        }));
    assert_eq!(grid.coordination(), GridCoordination::NoCandidates);
    drop(grid);
    let mut constrained = limits();
    constrained.seams.max_faces = 0;
    let grid = field
        .candidate_grid_with_budget([0; 3], [2, 2, 1], 2, constrained, budget.clone(), || false)
        .unwrap();
    assert_eq!(
        grid.coordination(),
        GridCoordination::Unresolved {
            reason: SurfaceError::Limit
        }
    );
    assert_eq!(grid.meshes().len(), 4);
    assert!(grid.faces().is_empty());
    drop(grid);
    assert_eq!(budget.used(), 0);
}

#[test]
fn candidate_grid_keeps_projection_end_faces_unresolved() {
    let budget = MemoryBudget::new(256 * 1024 * 1024);
    let field = fractional(source([2, 2, 3], true, false), |[x, y, z]| {
        if (z == 0 && x == 0 && y == 0)
            || (z == 1 && x == 1 && y == 1)
            || (z == 2 && x == 0 && y == 0)
        {
            26
        } else {
            -6
        }
    });
    let grid = field
        .candidate_grid_with_budget([0; 3], [1, 1, 2], 2, limits(), budget.clone(), || false)
        .unwrap();
    assert_eq!(grid.meshes().len(), 2);
    assert_eq!(grid.faces().len(), 1);
    assert_eq!(
        grid.faces()[0].state,
        CellFaceState::Unresolved {
            reason: CellSeamStop::ProjectionEndFace
        }
    );
    drop(grid);
    assert_eq!(budget.used(), 0);
}

#[test]
fn candidate_grid_is_atomic_under_cancellation_admission_and_invalid_scopes() {
    let field = grid_field();
    let budget = MemoryBudget::new(256 * 1024 * 1024);
    let calls = Cell::new(0usize);
    let grid = field
        .candidate_grid_with_budget([0; 3], [2, 2, 1], 2, limits(), budget.clone(), || {
            calls.set(calls.get() + 1);
            false
        })
        .unwrap();
    let total = calls.get();
    let completed_bytes = budget.used();
    drop(grid);
    assert_eq!(budget.used(), 0);
    for stop in [1, total / 3, total / 2, total] {
        let calls = Cell::new(0usize);
        assert!(matches!(
            field.candidate_grid_with_budget(
                [0; 3],
                [2, 2, 1],
                2,
                limits(),
                budget.clone(),
                || {
                    calls.set(calls.get() + 1);
                    calls.get() >= stop
                }
            ),
            Err(SurfaceError::Cancelled)
        ));
        assert_eq!(budget.used(), 0);
    }
    let small = MemoryBudget::new(1);
    assert!(matches!(
        field.candidate_grid_with_budget([0; 3], [2, 2, 1], 2, limits(), small.clone(), || false),
        Err(SurfaceError::Memory)
    ));
    assert_eq!(small.used(), 0);
    let late = MemoryBudget::new(completed_bytes - 1);
    assert!(matches!(
        field.candidate_grid_with_budget([0; 3], [2, 2, 1], 2, limits(), late.clone(), || false),
        Err(SurfaceError::Memory)
    ));
    assert_eq!(late.used(), 0);
    assert!(matches!(
        field.candidate_grid_with_budget([0; 3], [0, 1, 1], 2, limits(), budget.clone(), || false),
        Err(SurfaceError::Limit)
    ));
    assert!(matches!(
        field.candidate_grid_with_budget([0; 3], [3, 2, 1], 2, limits(), budget.clone(), || false),
        Err(SurfaceError::Geometry)
    ));
    assert!(matches!(
        field.candidate_grid_with_budget([0; 3], [2, 2, 1], 3, limits(), budget.clone(), || false),
        Err(SurfaceError::Geometry)
    ));
    assert!(matches!(
        field.candidate_grid_with_budget(
            [0; 3],
            [2, 2, 1],
            2,
            CandidateGridLimits {
                max_directory_bytes: 0,
                ..limits()
            },
            budget.clone(),
            || false
        ),
        Err(SurfaceError::Limit)
    ));
    assert_eq!(budget.used(), 0);
}

#[test]
fn assemblies_retain_affine_planes_and_explicit_cross_box_neighbor_traces() {
    for mixed in [false, true] {
        let field = fractional(source([3, 2, 2], true, false), |[x, y, z]| {
            let offset = [-2, 0, 4][x];
            -4 + 4 * y as i16
                + 4 * z as i16
                + offset
                + if mixed && [x, y, z] == [2, 1, 1] {
                    2
                } else {
                    0
                }
        });
        let budget = MemoryBudget::new(512 * 1024 * 1024);
        let left = field
            .candidate_grid_with_budget([0, 0, 0], [1; 3], 2, limits(), budget.clone(), || false)
            .unwrap();
        let right = field
            .candidate_grid_with_budget([1, 0, 0], [1; 3], 2, limits(), budget.clone(), || false)
            .unwrap();
        let assembly = CandidateAssembly::build_with_budget(
            &[&right, &left],
            assembly_limits(),
            budget.clone(),
            || false,
        )
        .unwrap();
        assert_eq!(assembly.cells().len(), 2);
        assert_eq!(assembly.affine_cells().len(), 2);
        assert_eq!(
            assembly.affine_cells()[0].disposition(),
            AffineDisposition::Plane
        );
        assert_eq!(assembly.max_error_mm(), limits().surface.max_error_mm);
        assert_eq!(assembly.affine_faces().len(), 1);
        if mixed {
            assert_eq!(
                assembly.affine_cells()[1].disposition(),
                AffineDisposition::NonAffine
            );
            assert!(matches!(
                assembly.affine_faces()[0].state(),
                AffineFaceState::Unresolved {
                    positive: AffineDisposition::NonAffine,
                    ..
                }
            ));
        } else {
            assert_eq!(
                assembly.affine_cells()[1].disposition(),
                AffineDisposition::Plane
            );
            assert!(matches!(
                assembly.affine_faces()[0].state(),
                AffineFaceState::Matched(AffineFaceTrace::Segment(_))
            ));
            assert!(matches!(
                assembly.mixed_face_partition_with_budget(
                    0,
                    FacePartitionLimits::default(),
                    budget.clone(),
                    || false,
                ),
                Ok(Err(MixedFaceUnsupported::NotMixed))
            ));
        }
        drop((assembly, left, right));
        assert_eq!(budget.used(), 0);
    }
}

#[test]
fn mixed_affine_and_general_face_partition_retains_split_requests_and_uncertified_facets() {
    let budget = MemoryBudget::new(512 * 1024 * 1024);
    let field = fractional(source([3, 2, 2], true, false), |[x, y, z]| {
        -4 + 4 * y as i16
            + 4 * z as i16
            + [-2, 0, 4][x]
            + if [x, y, z] == [2, 1, 1] { -12 } else { 0 }
    });
    let affine = Arc::new(field.affine_cell([0, 0, 0], || false).unwrap());
    assert_eq!(affine.disposition(), AffineDisposition::Plane);
    let grid = field
        .candidate_grid_with_budget([1, 0, 0], [1; 3], 2, limits(), budget.clone(), || false)
        .unwrap();
    let CandidateCellState::Candidate { mesh } = grid.cells()[0].state else {
        panic!("expected a general candidate");
    };
    let candidate = grid.meshes()[usize::from(mesh)].clone();
    let partition_limits = FacePartitionLimits {
        max_edges_per_cell: 1024,
        max_points: 2048,
        ..Default::default()
    };
    let partition = candidate
        .mixed_face_partition_with_budget(&affine, 0, partition_limits, budget.clone(), || false)
        .unwrap()
        .expect("expected a transverse segment");
    assert!(
        partition
            .spans()
            .iter()
            .any(|span| matches!(span.candidate, FaceSpanCoverage::Present { .. }))
    );
    assert_eq!(
        partition.face(),
        FaceKey {
            origin: [1, 0, 0],
            axis: 0
        }
    );
    assert_eq!(partition.projection_axis(), 2);
    assert_eq!(partition.affine().cell().origin(), affine.cell().origin());
    assert_eq!(partition.affine().field().key(), affine.field().key());
    assert!(Arc::ptr_eq(partition.candidate(), &candidate));
    assert_eq!(partition.points().len(), 5);
    assert_eq!(
        partition
            .points()
            .iter()
            .filter(|point| point.affine == MixedAffinePlacement::Split)
            .count(),
        3
    );
    assert!(
        partition
            .spans()
            .iter()
            .all(|span| span.affine && !span.candidate_within_tolerance)
    );
    for (endpoint, numerator) in [(0, 0), (1, 1)] {
        let coordinate = partition
            .points()
            .iter()
            .find(|point| point.affine == MixedAffinePlacement::Endpoint(endpoint))
            .unwrap()
            .coordinate
            .rational_value()
            .unwrap();
        assert_eq!(
            (coordinate.numerator(), coordinate.denominator()),
            (numerator, 1)
        );
    }
    assert!(
        partition
            .points()
            .iter()
            .any(|point| matches!(point.affine, MixedAffinePlacement::Endpoint(0)))
    );
    assert!(
        partition
            .points()
            .iter()
            .any(|point| matches!(point.affine, MixedAffinePlacement::Endpoint(1)))
    );
    for pair in partition.points().windows(2) {
        assert!(
            pair[0]
                .coordinate
                .compare(pair[1].coordinate)
                .unwrap()
                .is_lt()
        );
    }
    for point in partition.points() {
        if let Some(position) = point.affine_position {
            assert_eq!(position[0], ConformingSurfaceMesh::POSITION_DENOMINATOR);
        } else {
            assert_eq!(point.candidate, FacePlacement::Absent);
        }
    }
    for span in partition.spans() {
        if span.candidate_within_tolerance {
            let FaceSpanCoverage::Present { triangle, .. } = span.candidate else {
                panic!("certified span has no candidate")
            };
            assert!(matches!(
                candidate.triangles()[triangle as usize].state,
                ConformingFacetState::WithinTolerance { .. }
            ));
        }
    }
    let mut loose = limits();
    loose.surface.max_error_mm = 100.0;
    let loose_grid = field
        .candidate_grid_with_budget([1, 0, 0], [1; 3], 2, loose, budget.clone(), || false)
        .unwrap();
    let CandidateCellState::Candidate { mesh } = loose_grid.cells()[0].state else {
        panic!("expected a candidate with a relaxed tolerance");
    };
    let loose_partition = loose_grid.meshes()[usize::from(mesh)]
        .mixed_face_partition_with_budget(&affine, 0, partition_limits, budget.clone(), || false)
        .unwrap()
        .expect("expected a transverse segment");
    assert!(
        loose_partition
            .spans()
            .iter()
            .any(|span| span.candidate_within_tolerance)
    );
    drop((loose_partition, loose_grid));
    let left = field
        .candidate_grid_with_budget([0, 0, 0], [1; 3], 2, limits(), budget.clone(), || false)
        .unwrap();
    let assembly = CandidateAssembly::build_with_budget(
        &[&grid, &left],
        assembly_limits(),
        budget.clone(),
        || false,
    )
    .unwrap();
    let assembled = assembly
        .mixed_face_partition_with_budget(0, partition_limits, budget.clone(), || false)
        .unwrap()
        .expect("assembly should expose the mixed face");
    assert_eq!(assembled.face(), partition.face());
    assert!(
        assembled
            .spans()
            .iter()
            .any(|span| matches!(span.candidate, FaceSpanCoverage::Present { .. }))
    );
    assert!(matches!(
        assembly.mixed_face_partition_with_budget(1, partition_limits, budget.clone(), || false),
        Err(SurfaceError::Selection)
    ));
    assert!(matches!(
        assembly.mixed_face_partition_with_budget(0, partition_limits, budget.clone(), || true),
        Err(SurfaceError::Cancelled)
    ));
    drop((assembled, assembly, left));
    let wrong = fractional(field.source().clone(), |_| 2);
    let wrong = Arc::new(wrong.affine_cell([0, 0, 0], || false).unwrap());
    let limits = FacePartitionLimits {
        max_edges_per_cell: 1024,
        max_points: 2048,
        ..Default::default()
    };
    assert!(matches!(
        candidate.mixed_face_partition_with_budget(&wrong, 0, limits, budget.clone(), || false),
        Err(SurfaceError::Geometry)
    ));
    assert!(matches!(
        candidate.mixed_face_partition_with_budget(&affine, 0, limits, budget.clone(), || true),
        Err(SurfaceError::Cancelled)
    ));
    let used_before_refusals = budget.used();
    for bounded in [
        FacePartitionLimits {
            max_operations: 1,
            ..limits
        },
        FacePartitionLimits {
            max_bytes: 0,
            ..limits
        },
        FacePartitionLimits {
            max_points: 0,
            ..limits
        },
    ] {
        assert!(matches!(
            candidate
                .mixed_face_partition_with_budget(&affine, 0, bounded, budget.clone(), || false),
            Err(SurfaceError::Limit)
        ));
        assert_eq!(budget.used(), used_before_refusals);
    }
    let empty = MemoryBudget::new(0);
    assert!(matches!(
        candidate.mixed_face_partition_with_budget(&affine, 0, limits, empty.clone(), || false),
        Err(SurfaceError::Memory)
    ));
    assert_eq!(empty.used(), 0);
    drop((partition, candidate, grid, affine, field));
    assert_eq!(budget.used(), 0);
}

#[test]
fn mixed_face_partition_handles_the_affine_cell_on_the_positive_side() {
    let budget = MemoryBudget::new(512 * 1024 * 1024);
    let field = fractional(source([3, 2, 2], true, false), |[x, y, z]| {
        let reflected_x = 2 - x;
        -4 + 4 * y as i16
            + 4 * z as i16
            + [-2, 0, 4][reflected_x]
            + if [reflected_x, y, z] == [2, 1, 1] {
                -12
            } else {
                0
            }
    });
    let left = field
        .candidate_grid_with_budget([0, 0, 0], [1; 3], 2, limits(), budget.clone(), || false)
        .unwrap();
    let right = field
        .candidate_grid_with_budget([1, 0, 0], [1; 3], 2, limits(), budget.clone(), || false)
        .unwrap();
    let assembly = CandidateAssembly::build_with_budget(
        &[&left, &right],
        assembly_limits(),
        budget.clone(),
        || false,
    )
    .unwrap();
    let partition = assembly
        .mixed_face_partition_with_budget(0, limits().seams.partition, budget.clone(), || false)
        .unwrap()
        .expect("expected a mixed face on the positive side");
    assert_eq!(partition.affine().cell().origin(), [1, 0, 0]);
    for (endpoint, numerator) in [(0, 0), (1, 1)] {
        let coordinate = partition
            .points()
            .iter()
            .find(|point| point.affine == MixedAffinePlacement::Endpoint(endpoint))
            .unwrap()
            .coordinate
            .rational_value()
            .unwrap();
        assert_eq!(
            (coordinate.numerator(), coordinate.denominator()),
            (numerator, 1)
        );
    }
    assert!(
        partition
            .spans()
            .iter()
            .any(|span| matches!(span.candidate, FaceSpanCoverage::Present { .. }))
    );
    for point in partition.points() {
        if let Some(position) = point.affine_position {
            assert_eq!(position[0], 0);
        }
    }
    drop((partition, assembly, right, left, field));
    assert_eq!(budget.used(), 0);
}

#[test]
fn mixed_face_application_joins_represented_edges_and_preserves_refusals() {
    for reflected in [false, true] {
        let budget = MemoryBudget::new(512 * 1024 * 1024);
        let field = fractional(source([3, 2, 2], true, false), |[x, y, z]| {
            let x = if reflected { 2 - x } else { x };
            -4 + 4 * y as i16
                + 4 * z as i16
                + [-2, 0, 4][x]
                + if [x, y, z] == [2, 1, 1] { -12 } else { 0 }
        });
        let affine_x = usize::from(reflected);
        let candidate_x = 1 - affine_x;
        let affine = field.affine_cell([affine_x, 0, 0], || false).unwrap();
        let grid = field
            .candidate_grid_with_budget(
                [candidate_x, 0, 0],
                [1; 3],
                2,
                limits(),
                budget.clone(),
                || false,
            )
            .unwrap();
        let CandidateCellState::Candidate { mesh } = grid.cells()[0].state else {
            panic!("expected a general candidate");
        };
        let candidate = grid.meshes()[usize::from(mesh)].clone();
        let make_partition = || {
            candidate
                .mixed_face_partition_with_budget(
                    &affine,
                    0,
                    limits().seams.partition,
                    budget.clone(),
                    || false,
                )
                .unwrap()
                .unwrap()
        };
        let partition = Arc::new(make_partition());
        let applied = partition
            .apply_mixed_with_budget(0.02, AppliedFaceLimits::default(), budget.clone(), || false)
            .unwrap()
            .expect("mixed face must pass physical and topology checks");
        assert!(applied.affine_error_mm() <= 0.02);
        assert_eq!(
            applied.affine_triangles().len(),
            applied.affine_vertices().len() - 2
        );
        assert!(applied.affine_vertices().len() > affine.vertices().len());
        let denominator = f64::from(ConformingSurfaceMesh::POSITION_DENOMINATOR);
        for (id, point) in partition.points().iter().enumerate() {
            let Some([candidate_id, affine_id]) = applied.point_vertices()[id][0]
                .zip(applied.point_vertices()[id][1])
                .map(|(a, b)| [a, b])
            else {
                continue;
            };
            let from_candidate = applied.candidate().vertices()[candidate_id as usize]
                .position()
                .unwrap();
            let from_affine = applied.affine_vertices()[affine_id as usize];
            assert_eq!(from_candidate[1..], from_affine[1..]);
            assert_eq!(
                from_candidate[0],
                if reflected {
                    ConformingSurfaceMesh::POSITION_DENOMINATOR
                } else {
                    0
                }
            );
            assert_eq!(
                from_affine[0],
                if reflected {
                    0
                } else {
                    ConformingSurfaceMesh::POSITION_DENOMINATOR
                }
            );
            let along = point.coordinate.approximate().unwrap();
            let expected = [f64::from(!reflected), along, 1.0 - along];
            let actual = from_affine.map(|v| f64::from(v) / denominator);
            let delta = std::array::from_fn(|axis| actual[axis] - expected[axis]);
            let distance: f64 = field
                .source()
                .voxel_vector_to_world(delta)
                .into_iter()
                .map(f64::abs)
                .sum();
            assert!(distance <= applied.affine_error_mm() + 1e-12);
        }
        assert!(
            applied
                .spans()
                .iter()
                .any(|span| span.segments.iter().all(Option::is_some))
        );
        assert!(
            applied
                .spans()
                .iter()
                .all(|span| !span.candidate_within_tolerance)
        );
        let used = budget.used();
        let calls = Cell::new(0usize);
        let counted = partition
            .apply_mixed_with_budget(0.02, AppliedFaceLimits::default(), budget.clone(), || {
                calls.set(calls.get() + 1);
                false
            })
            .unwrap()
            .unwrap();
        let total_calls = calls.get();
        drop(counted);
        for stop in [total_calls / 3, total_calls / 2, total_calls] {
            let calls = Cell::new(0usize);
            assert!(matches!(
                partition.apply_mixed_with_budget(
                    0.02,
                    AppliedFaceLimits::default(),
                    budget.clone(),
                    || {
                        calls.set(calls.get() + 1);
                        calls.get() >= stop
                    }
                ),
                Err(SurfaceError::Cancelled)
            ));
            assert_eq!(budget.used(), used);
        }
        assert!(matches!(
            partition.apply_mixed_with_budget(
                0.02,
                AppliedFaceLimits::default(),
                budget.clone(),
                || true,
            ),
            Err(SurfaceError::Cancelled)
        ));
        assert!(matches!(
            partition.apply_mixed_with_budget(
                -1.0,
                AppliedFaceLimits::default(),
                budget.clone(),
                || false,
            ),
            Err(SurfaceError::Limit)
        ));
        assert!(matches!(
            partition.apply_mixed_with_budget(
                0.02,
                AppliedFaceLimits {
                    max_operations: 1,
                    ..Default::default()
                },
                budget.clone(),
                || false,
            ),
            Err(SurfaceError::Limit)
        ));
        assert_eq!(budget.used(), used);
        let empty = MemoryBudget::new(0);
        assert!(matches!(
            partition.apply_mixed_with_budget(
                0.02,
                AppliedFaceLimits::default(),
                empty.clone(),
                || false,
            ),
            Err(SurfaceError::Memory)
        ));
        assert_eq!(empty.used(), 0);
        let scratch_refused = MemoryBudget::new(applied.referenced_bytes());
        assert!(matches!(
            partition.apply_mixed_with_budget(
                0.02,
                AppliedFaceLimits::default(),
                scratch_refused.clone(),
                || false,
            ),
            Err(SurfaceError::Memory)
        ));
        assert_eq!(scratch_refused.used(), 0);
        let mut collapsed = make_partition();
        let endpoint = collapsed
            .points
            .iter()
            .find(|point| point.affine == MixedAffinePlacement::Endpoint(0))
            .unwrap()
            .affine_position
            .unwrap();
        let split = collapsed
            .points
            .iter_mut()
            .find(|point| point.affine == MixedAffinePlacement::Split)
            .unwrap();
        split.affine_position = Some(endpoint);
        assert!(matches!(
            Arc::new(collapsed).apply_mixed_with_budget(
                100.0,
                AppliedFaceLimits::default(),
                budget.clone(),
                || false,
            ),
            Ok(Err(MixedFaceApplyRefusal::Topology))
        ));
        drop((applied, partition, candidate, grid, affine, field));
        assert_eq!(budget.used(), 0);
    }
}

#[test]
fn mixed_face_application_rejects_quantization_beyond_physical_tolerance() {
    let budget = MemoryBudget::new(512 * 1024 * 1024);
    let field = fractional(source([3, 2, 2], true, false), |[x, y, z]| {
        -10 + 14 * y as i16
            + 14 * z as i16
            + [-4, 0, 8][x]
            + if [x, y, z] == [2, 1, 1] { -24 } else { 0 }
    });
    let affine = field.affine_cell([0, 0, 0], || false).unwrap();
    let grid = field
        .candidate_grid_with_budget([1, 0, 0], [1; 3], 2, limits(), budget.clone(), || false)
        .unwrap();
    let CandidateCellState::Candidate { mesh } = grid.cells()[0].state else {
        panic!("expected a general candidate");
    };
    let partition = Arc::new(
        grid.meshes()[usize::from(mesh)]
            .mixed_face_partition_with_budget(
                &affine,
                0,
                limits().seams.partition,
                budget.clone(),
                || false,
            )
            .unwrap()
            .unwrap(),
    );
    let before = budget.used();
    assert!(matches!(
        partition.apply_mixed_with_budget(
            0.0,
            AppliedFaceLimits::default(),
            budget.clone(),
            || false,
        ),
        Ok(Err(MixedFaceApplyRefusal::PhysicalTolerance))
    ));
    assert_eq!(budget.used(), before);
    let applied = partition
        .apply_mixed_with_budget(0.02, AppliedFaceLimits::default(), budget.clone(), || false)
        .unwrap()
        .expect("non-dyadic face should meet the relaxed physical tolerance");
    assert!(applied.affine_error_mm() > 0.0 && applied.affine_error_mm() <= 0.02);
    let denominator = f64::from(ConformingSurfaceMesh::POSITION_DENOMINATOR);
    for (id, point) in partition.points().iter().enumerate() {
        if let Some(vertex) = applied.point_vertices()[id][1] {
            let along = point.coordinate.approximate().unwrap();
            let expected = [1.0, along, 5.0 / 7.0 - along];
            let actual =
                applied.affine_vertices()[vertex as usize].map(|v| f64::from(v) / denominator);
            let delta = std::array::from_fn(|axis| actual[axis] - expected[axis]);
            let distance: f64 = field
                .source()
                .voxel_vector_to_world(delta)
                .into_iter()
                .map(f64::abs)
                .sum();
            assert!(distance <= applied.affine_error_mm() + 1e-12);
        }
    }
    drop((applied, partition, grid, affine, field));
    assert_eq!(budget.used(), 0);
}

#[test]
fn mixed_face_application_preserves_shared_coordinates_on_every_axis() {
    for face_axis in 0..3 {
        for projection in 0..3 {
            if projection == face_axis {
                continue;
            }
            let along = (0..3)
                .find(|axis| *axis != face_axis && *axis != projection)
                .unwrap();
            for reflected in [false, true] {
                let mut dimensions = [2; 3];
                dimensions[face_axis] = 3;
                let budget = MemoryBudget::new(512 * 1024 * 1024);
                let field = fractional(source(dimensions, true, false), |position| {
                    let normal = if reflected {
                        2 - position[face_axis]
                    } else {
                        position[face_axis]
                    };
                    let u = position[along];
                    let v = position[projection];
                    -4 + 4 * u as i16
                        + 4 * v as i16
                        + [-2, 0, 4][normal]
                        + if [normal, u, v] == [2, 1, 1] { -12 } else { 0 }
                });
                let mut affine_origin = [0; 3];
                affine_origin[face_axis] = usize::from(reflected);
                let mut candidate_origin = [0; 3];
                candidate_origin[face_axis] = usize::from(!reflected);
                let affine = field.affine_cell(affine_origin, || false).unwrap();
                let grid = field
                    .candidate_grid_with_budget(
                        candidate_origin,
                        [1; 3],
                        projection,
                        limits(),
                        budget.clone(),
                        || false,
                    )
                    .unwrap();
                let CandidateCellState::Candidate { mesh } = grid.cells()[0].state else {
                    panic!(
                        "missing candidate for face {face_axis}, projection {projection}, reflected {reflected}"
                    );
                };
                let partition = Arc::new(
                    grid.meshes()[usize::from(mesh)]
                        .mixed_face_partition_with_budget(
                            &affine,
                            face_axis,
                            limits().seams.partition,
                            budget.clone(),
                            || false,
                        )
                        .unwrap()
                        .unwrap(),
                );
                let applied = partition
                    .apply_mixed_with_budget(
                        0.02,
                        AppliedFaceLimits::default(),
                        budget.clone(),
                        || false,
                    )
                    .unwrap()
                    .expect("expected a valid mixed face");
                for mapping in applied.point_vertices() {
                    if let Some((candidate, affine)) = mapping[0].zip(mapping[1]) {
                        let p = applied.candidate().vertices()[candidate as usize]
                            .position()
                            .unwrap();
                        let q = applied.affine_vertices()[affine as usize];
                        for axis in 0..3 {
                            if axis != face_axis {
                                assert_eq!(p[axis], q[axis]);
                            }
                        }
                        assert_eq!(
                            p[face_axis],
                            if reflected {
                                ConformingSurfaceMesh::POSITION_DENOMINATOR
                            } else {
                                0
                            }
                        );
                        assert_eq!(
                            q[face_axis],
                            if reflected {
                                0
                            } else {
                                ConformingSurfaceMesh::POSITION_DENOMINATOR
                            }
                        );
                    }
                }
                assert!(
                    applied
                        .spans()
                        .iter()
                        .any(|span| span.segments.iter().all(Option::is_some))
                );
                drop((applied, partition, grid, affine, field));
                assert_eq!(budget.used(), 0);
            }
        }
    }
}

#[test]
fn promoted_mixed_candidate_retains_exact_split_and_can_be_repartitioned() {
    let budget = MemoryBudget::new(512 * 1024 * 1024);
    let field = fractional(source([3, 3, 2], true, false), |[x, y, z]| {
        let y = if y == 2 { 0 } else { y };
        -2 + 4 * y as i16
            + 4 * z as i16
            + [-2, 0, 4][x]
            + if [x, y, z] == [2, 1, 1] { -12 } else { 0 }
    });
    let affine = field.affine_cell([0, 0, 0], || false).unwrap();
    let grid = field
        .candidate_grid_with_budget([1, 0, 0], [1; 3], 2, limits(), budget.clone(), || false)
        .unwrap();
    let CandidateCellState::Candidate { mesh } = grid.cells()[0].state else {
        panic!("expected a candidate");
    };
    let candidate = grid.meshes()[usize::from(mesh)].clone();
    let mut partition = candidate
        .mixed_face_partition_with_budget(
            &affine,
            0,
            limits().seams.partition,
            budget.clone(),
            || false,
        )
        .unwrap()
        .unwrap();
    // Insert a valid point strictly inside an existing represented candidate edge,
    // as would happen when a neighboring face contributes an additional split.
    let (fraction, coordinate, span_index) = (1i16..32)
        .find_map(|fraction| {
            let EdgeZero::Point(ratio) =
                Trilinear::new([-fraction, 64 - fraction, 1, 1, 1, 1, 1, 1])
                    .unwrap()
                    .edge(0, 0)
                    .unwrap()
            else {
                return None;
            };
            let coordinate = ExactCoordinate::from_rational(ratio).unwrap().unwrap();
            partition
                .spans
                .iter()
                .position(|span| {
                    partition.points[span.points[0] as usize]
                        .coordinate
                        .compare(coordinate)
                        .unwrap()
                        .is_lt()
                        && partition.points[span.points[1] as usize]
                            .coordinate
                            .compare(coordinate)
                            .unwrap()
                            .is_gt()
                        && matches!(span.candidate, FaceSpanCoverage::Present { .. })
                })
                .map(|span_index| (fraction, coordinate, span_index))
        })
        .expect("fixture should contain a split in a represented edge");
    let original_span = partition.spans[span_index];
    let FaceSpanCoverage::Present { triangle, edge } = original_span.candidate else {
        unreachable!()
    };
    let insert = original_span.points[1] as usize;
    let denominator = ConformingSurfaceMesh::POSITION_DENOMINATOR;
    let along = denominator * u32::try_from(fraction).unwrap() / 64;
    let candidate_position = [0, along, denominator / 2 - along];
    let affine_position = [denominator, candidate_position[1], candidate_position[2]];
    partition.points.insert(
        insert,
        MixedFacePoint {
            coordinate,
            affine: MixedAffinePlacement::Split,
            candidate: FacePlacement::Split {
                triangle,
                edge,
                position: candidate_position,
            },
            affine_position: Some(affine_position),
        },
    );
    for span in &mut partition.spans {
        for point in &mut span.points {
            if *point >= insert as u32 {
                *point += 1;
            }
        }
    }
    let adjusted = partition.spans[span_index];
    partition.spans[span_index].points = [adjusted.points[0], insert as u32];
    partition.spans.insert(
        span_index + 1,
        MixedFaceSpan {
            points: [insert as u32, adjusted.points[1]],
            ..adjusted
        },
    );
    let partition = Arc::new(partition);
    let applied = partition
        .apply_mixed_with_budget(0.02, AppliedFaceLimits::default(), budget.clone(), || false)
        .unwrap()
        .unwrap();
    let vertex_id = applied.point_vertices()[insert][0].unwrap();
    assert!(matches!(
        applied.candidate().vertices()[vertex_id as usize],
        AppliedFaceVertex::Split { .. }
    ));
    let promoted = applied
        .promote_candidate_compacted_with_budget(
            FacePromotionLimits::default(),
            budget.clone(),
            || false,
        )
        .unwrap();
    assert_eq!(
        promoted.physical_tolerance_mm(),
        candidate.physical_tolerance_mm()
    );
    assert_eq!(
        promoted.triangles().len(),
        applied.candidate().triangles().len()
    );
    let ConformingVertex::FaceSplit {
        point, position, ..
    } = promoted.vertices()[vertex_id as usize]
    else {
        panic!("candidate split was not retained");
    };
    assert_eq!(position, candidate_position);
    let (face, retained) = promoted.face_split_source(point).unwrap();
    assert_eq!(face, partition.face());
    assert!(retained.compare(coordinate).unwrap().is_eq());
    let again = promoted
        .mixed_face_partition_with_budget(
            &affine,
            0,
            limits().seams.partition,
            budget.clone(),
            || false,
        )
        .unwrap()
        .unwrap();
    let point = again
        .points()
        .iter()
        .find(|point| point.coordinate.compare(coordinate).unwrap().is_eq())
        .unwrap();
    assert_eq!(
        point.candidate,
        FacePlacement::Existing { vertex: vertex_id }
    );
    let neighbor_grid = field
        .candidate_grid_with_budget([1, 1, 0], [1; 3], 2, limits(), budget.clone(), || false)
        .unwrap();
    let CandidateCellState::Candidate { mesh } = neighbor_grid.cells()[0].state else {
        panic!("expected a second candidate for later face coordination");
    };
    let neighbor = neighbor_grid.meshes()[usize::from(mesh)].clone();
    let before_face = candidate
        .shared_face_partition_with_budget(
            &neighbor,
            1,
            limits().seams.partition,
            budget.clone(),
            || false,
        )
        .unwrap();
    let next_face = promoted
        .shared_face_partition_with_budget(
            &neighbor,
            1,
            limits().seams.partition,
            budget.clone(),
            || false,
        )
        .unwrap();
    assert_eq!(next_face.face().axis, 1);
    let batch = CellSeamBatch::coordinate_with_budget(
        &[promoted.clone(), neighbor.clone()],
        limits().seams,
        budget.clone(),
        || false,
    )
    .unwrap();
    assert_eq!(batch.faces().len(), 1);
    assert!(!matches!(
        batch.faces()[0].state,
        CellFaceState::Unresolved { .. }
    ));
    let repeated_partition = Arc::new(
        batch.cells()[0]
            .mixed_face_partition_with_budget(
                &affine,
                0,
                limits().seams.partition,
                budget.clone(),
                || false,
            )
            .unwrap()
            .unwrap(),
    );
    let repeated_application = repeated_partition
        .apply_mixed_with_budget(0.02, AppliedFaceLimits::default(), budget.clone(), || false)
        .unwrap()
        .unwrap();
    let repeated = repeated_application
        .promote_candidate_compacted_with_budget(
            FacePromotionLimits::default(),
            budget.clone(),
            || false,
        )
        .unwrap();
    assert!(repeated.vertices().iter().any(|vertex| {
        match vertex {
            ConformingVertex::FaceSplit { point, .. } => repeated
                .face_split_source(*point)
                .is_some_and(|(face, exact)| {
                    face == partition.face() && exact.compare(coordinate).unwrap().is_eq()
                }),
            _ => false,
        }
    }));
    for (expected, actual) in applied
        .candidate()
        .triangles()
        .iter()
        .zip(promoted.triangles())
    {
        assert_eq!(expected.vertices, actual.vertices);
        assert_eq!(expected.source_facet, actual.source_facet);
        match (expected.state, actual.state) {
            (
                AppliedTriangleState::WithinTolerance {
                    numerator: a,
                    denominator: b,
                },
                ConformingFacetState::WithinTolerance {
                    numerator: c,
                    denominator: d,
                },
            ) => assert_eq!((a, b), (c, d)),
            (
                AppliedTriangleState::Unresolved { certificate: a },
                ConformingFacetState::Unresolved {
                    certificate: Some(b),
                    ..
                },
            ) => assert_eq!(a, b),
            _ => panic!("candidate certification changed during promotion"),
        }
    }
    let retained_bytes = budget.used();
    let calls = Cell::new(0usize);
    let counted = applied
        .promote_candidate_compacted_with_budget(
            FacePromotionLimits::default(),
            budget.clone(),
            || {
                calls.set(calls.get() + 1);
                false
            },
        )
        .unwrap();
    let total_calls = calls.get();
    drop(counted);
    for stop in [1, total_calls / 2, total_calls] {
        let calls = Cell::new(0usize);
        assert!(matches!(
            applied.promote_candidate_compacted_with_budget(
                FacePromotionLimits::default(),
                budget.clone(),
                || {
                    calls.set(calls.get() + 1);
                    calls.get() >= stop
                },
            ),
            Err(SurfaceError::Cancelled)
        ));
        assert_eq!(budget.used(), retained_bytes);
    }
    let empty = MemoryBudget::new(0);
    assert!(matches!(
        applied.promote_candidate_compacted_with_budget(
            FacePromotionLimits::default(),
            empty.clone(),
            || false,
        ),
        Err(SurfaceError::Memory)
    ));
    assert_eq!(empty.used(), 0);
    assert!(matches!(
        applied.promote_candidate_compacted_with_budget(
            FacePromotionLimits {
                max_seam_points_per_cell: 0,
                ..Default::default()
            },
            budget.clone(),
            || false,
        ),
        Err(SurfaceError::Limit)
    ));
    assert_eq!(budget.used(), retained_bytes);
    let axial = Arc::new(
        candidate
            .input()
            .refine_conforming_with_budget(
                ConformingSurfaceLimits {
                    tolerance_bits: 6,
                    max_depth: 4,
                    max_vertices: 2048,
                    max_triangles: 2048,
                    max_operations: 2_000_000,
                    ..Default::default()
                },
                budget.clone(),
                || false,
            )
            .unwrap(),
    );
    assert_eq!(axial.axial_tolerance_bits(), Some(6));
    let axial_partition = Arc::new(
        axial
            .mixed_face_partition_with_budget(
                &affine,
                0,
                limits().seams.partition,
                budget.clone(),
                || false,
            )
            .unwrap()
            .unwrap(),
    );
    let axial_application = axial_partition
        .apply_mixed_with_budget(
            0.02,
            AppliedFaceLimits {
                tolerance_bits: [12, 8],
                ..Default::default()
            },
            budget.clone(),
            || false,
        )
        .unwrap()
        .unwrap();
    let axial_promoted = axial_application
        .promote_candidate_compacted_with_budget(
            FacePromotionLimits::default(),
            budget.clone(),
            || false,
        )
        .unwrap();
    assert_eq!(axial_promoted.axial_tolerance_bits(), Some(12));
    drop((axial_promoted, axial_application, axial_partition, axial));
    assert_eq!(budget.used(), retained_bytes);
    drop((
        batch,
        repeated,
        repeated_application,
        repeated_partition,
        before_face,
        next_face,
        neighbor,
        neighbor_grid,
        again,
        applied,
        partition,
        candidate,
        grid,
    ));
    assert_eq!(promoted.face_lineage_depth(), 0);
    assert!(promoted.face_application().is_none());
    let after_release = promoted
        .mixed_face_partition_with_budget(
            &affine,
            0,
            limits().seams.partition,
            budget.clone(),
            || false,
        )
        .unwrap()
        .unwrap();
    assert!(after_release.points().iter().any(|point| {
        point.coordinate.compare(coordinate).unwrap().is_eq()
            && point.candidate == FacePlacement::Existing { vertex: vertex_id }
    }));
    drop((after_release, promoted, affine, field));
    assert_eq!(budget.used(), 0);
}

#[test]
fn affine_face_merge_keeps_neighbor_boundaries_and_rejects_duplicates() {
    let budget = MemoryBudget::new(512 * 1024 * 1024);
    fn sample([x, y, z]: [usize; 3]) -> i16 {
        -10 + 4 * x as i16
            + 4 * y as i16
            + 8 * z as i16
            + if [x, y, z] == [2, 1, 1] { -24 } else { 0 }
            + if [x, y, z] == [1, 2, 1] { -24 } else { 0 }
            + if [x, y, z] == [1, 1, 2] { -24 } else { 0 }
    }
    let field = fractional(source([3, 3, 3], true, false), sample);
    let affine = field.affine_cell([0, 0, 0], || false).unwrap();
    assert_eq!(affine.disposition(), AffineDisposition::Plane);
    let mut grids = Vec::new();
    let mut applications = Vec::new();
    for (origin, axis, projection) in [([1, 0, 0], 0, 2), ([0, 1, 0], 1, 2), ([0, 0, 1], 2, 0)] {
        let grid = field
            .candidate_grid_with_budget(
                origin,
                [1; 3],
                projection,
                limits(),
                budget.clone(),
                || false,
            )
            .unwrap();
        let CandidateCellState::Candidate { mesh } = grid.cells()[0].state else {
            panic!(
                "expected candidate on face {axis}: {:?}",
                grid.cells()[0].state
            );
        };
        let partition = Arc::new(
            grid.meshes()[usize::from(mesh)]
                .mixed_face_partition_with_budget(
                    &affine,
                    axis,
                    limits().seams.partition,
                    budget.clone(),
                    || false,
                )
                .unwrap()
                .expect("expected a transverse segment"),
        );
        let applied = Arc::new(
            partition
                .apply_mixed_with_budget(0.02, AppliedFaceLimits::default(), budget.clone(), || {
                    false
                })
                .unwrap()
                .expect("face application should pass"),
        );
        assert!(
            applied
                .spans()
                .iter()
                .any(|span| span.segments.iter().all(Option::is_some))
        );
        assert!(
            partition
                .points()
                .iter()
                .any(|point| point.affine == MixedAffinePlacement::Split
                    && point.affine_position.is_some())
        );
        grids.push(grid);
        applications.push(applied);
    }
    let merged = AffineFaceMesh::merge_with_budget(
        &applications,
        0.02,
        AffineFaceMergeLimits::default(),
        budget.clone(),
        || false,
    )
    .unwrap()
    .expect("the faces should agree");
    assert_eq!(merged.applications().len(), 3);
    assert_eq!(merged.affine().cell().origin(), [0, 0, 0]);
    assert!(merged.error_mm() <= 0.02);
    let boundary = &merged.vertices()[..merged.boundary_vertices()];
    assert!(boundary.len() > affine.vertices().len());
    assert_eq!(merged.triangles().len(), boundary.len() - 2);
    let corners: Vec<[u32; 3]> = affine
        .vertices()
        .iter()
        .map(|point| {
            point.map(|coordinate| {
                let numerator = u64::try_from(coordinate.numerator()).unwrap();
                let denominator = coordinate.denominator();
                let grid = u64::from(ConformingSurfaceMesh::POSITION_DENOMINATOR);
                u32::try_from((2 * numerator * grid + denominator) / (2 * denominator)).unwrap()
            })
        })
        .collect();
    for corner in &corners {
        let index = boundary
            .iter()
            .position(|position| position == corner)
            .unwrap();
        let previous = boundary[(index + boundary.len() - 1) % boundary.len()];
        let next = boundary[(index + 1) % boundary.len()];
        assert!(!corners.contains(&previous) || !corners.contains(&next));
    }
    let area = |a: [u32; 3], b: [u32; 3], c: [u32; 3]| {
        (i128::from(b[0]) - i128::from(a[0])) * (i128::from(c[1]) - i128::from(a[1]))
            - (i128::from(b[1]) - i128::from(a[1])) * (i128::from(c[0]) - i128::from(a[0]))
    };
    let boundary_area: i128 = (1..boundary.len() - 1)
        .map(|i| area(boundary[0], boundary[i], boundary[i + 1]))
        .sum();
    let mut edges = BTreeMap::<(u32, u32), usize>::new();
    let mut triangle_area = 0i128;
    for triangle in merged.triangles() {
        let [a, b, c] = triangle.map(|id| merged.vertices()[id as usize]);
        let signed = area(a, b, c);
        assert_eq!(signed.signum(), boundary_area.signum());
        triangle_area += signed;
        for [u, v] in [
            [triangle[0], triangle[1]],
            [triangle[1], triangle[2]],
            [triangle[2], triangle[0]],
        ] {
            *edges.entry((u.min(v), u.max(v))).or_default() += 1;
        }
    }
    assert_ne!(boundary_area, 0);
    assert_eq!(triangle_area, boundary_area);
    for i in 0..boundary.len() {
        let j = (i + 1) % boundary.len();
        assert_eq!(edges.remove(&(i.min(j) as u32, i.max(j) as u32)), Some(1));
    }
    assert!(edges.values().all(|count| *count == 2));
    for (application_id, application) in applications.iter().enumerate() {
        let mapping = merged.face_point_vertices(application_id).unwrap();
        assert_eq!(mapping.len(), application.input().points().len());
        for (point, mapped) in application.input().points().iter().zip(mapping) {
            if point.affine != MixedAffinePlacement::Outside
                && let Some(position) = point.affine_position
            {
                assert_eq!(merged.vertices()[mapped.unwrap() as usize], position);
                let t = point.coordinate.approximate().unwrap();
                let exact = match application.input().face().axis {
                    0 => [1.0, t, (6.0 - 4.0 * t) / 8.0],
                    1 => [t, 1.0, (6.0 - 4.0 * t) / 8.0],
                    2 => [(2.0 - 4.0 * t) / 4.0, t, 1.0],
                    _ => panic!("invalid face"),
                };
                let grid = f64::from(ConformingSurfaceMesh::POSITION_DENOMINATOR);
                let actual = position.map(|value| f64::from(value) / grid);
                let difference = std::array::from_fn(|axis| actual[axis] - exact[axis]);
                let physical: f64 = field
                    .source()
                    .voxel_vector_to_world(difference)
                    .into_iter()
                    .map(f64::abs)
                    .sum();
                assert!(physical <= merged.error_mm() + 1e-10);
            }
            if matches!(point.affine, MixedAffinePlacement::Endpoint(_)) {
                assert!(mapped.is_some());
            }
        }
        for span in application.spans() {
            let Some(affine_edge) = span.segments[1] else {
                continue;
            };
            let positions = affine_edge.map(|id| application.affine_vertices()[id as usize]);
            assert!(
                boundary.iter().enumerate().any(|(i, point)| {
                    (*point == positions[0] && boundary[(i + 1) % boundary.len()] == positions[1])
                        || (*point == positions[1]
                            && boundary[(i + 1) % boundary.len()] == positions[0])
                }),
                "a represented face segment disappeared from the merged boundary"
            );
        }
    }
    let reversed = AffineFaceMesh::merge_with_budget(
        &[
            applications[2].clone(),
            applications[1].clone(),
            applications[0].clone(),
        ],
        0.02,
        AffineFaceMergeLimits::default(),
        budget.clone(),
        || false,
    )
    .unwrap()
    .unwrap();
    assert_eq!(reversed.vertices(), merged.vertices());
    assert_eq!(reversed.triangles(), merged.triangles());
    assert_eq!(reversed.error_mm(), merged.error_mm());
    assert_eq!(
        merged.face_point_vertices(0),
        reversed.face_point_vertices(2)
    );
    assert_eq!(
        merged.face_point_vertices(1),
        reversed.face_point_vertices(1)
    );
    assert_eq!(
        merged.face_point_vertices(2),
        reversed.face_point_vertices(0)
    );
    assert!(merged.face_point_vertices(3).is_none());
    let used = budget.used();
    let other_field = fractional(field.source().clone(), sample);
    let other_affine = other_field.affine_cell([0, 0, 0], || false).unwrap();
    let other_grid = other_field
        .candidate_grid_with_budget([1, 0, 0], [1; 3], 2, limits(), budget.clone(), || false)
        .unwrap();
    let CandidateCellState::Candidate { mesh } = other_grid.cells()[0].state else {
        panic!("expected a candidate from the other mask");
    };
    let other_partition = Arc::new(
        other_grid.meshes()[usize::from(mesh)]
            .mixed_face_partition_with_budget(
                &other_affine,
                0,
                limits().seams.partition,
                budget.clone(),
                || false,
            )
            .unwrap()
            .unwrap(),
    );
    let other_application = Arc::new(
        other_partition
            .apply_mixed_with_budget(0.02, AppliedFaceLimits::default(), budget.clone(), || false)
            .unwrap()
            .unwrap(),
    );
    assert!(matches!(
        AffineFaceMesh::merge_with_budget(
            &[applications[1].clone(), other_application.clone()],
            0.02,
            AffineFaceMergeLimits::default(),
            budget.clone(),
            || false,
        ),
        Err(SurfaceError::Geometry)
    ));
    drop((
        other_application,
        other_partition,
        other_grid,
        other_affine,
        other_field,
    ));
    assert_eq!(budget.used(), used);
    if merged.error_mm() > 0.0 {
        assert!(matches!(
            AffineFaceMesh::merge_with_budget(
                &applications,
                0.0,
                AffineFaceMergeLimits::default(),
                budget.clone(),
                || false,
            ),
            Ok(Err(AffineFaceMergeRefusal::PhysicalTolerance))
        ));
        assert_eq!(budget.used(), used);
    }
    let calls = Cell::new(0usize);
    let counted = AffineFaceMesh::merge_with_budget(
        &applications,
        0.02,
        AffineFaceMergeLimits::default(),
        budget.clone(),
        || {
            calls.set(calls.get() + 1);
            false
        },
    )
    .unwrap()
    .unwrap();
    let total_calls = calls.get();
    drop(counted);
    for stop in [1, total_calls / 2, total_calls] {
        let calls = Cell::new(0usize);
        assert!(matches!(
            AffineFaceMesh::merge_with_budget(
                &applications,
                0.02,
                AffineFaceMergeLimits::default(),
                budget.clone(),
                || {
                    calls.set(calls.get() + 1);
                    calls.get() >= stop
                },
            ),
            Err(SurfaceError::Cancelled)
        ));
        assert_eq!(budget.used(), used);
    }
    assert!(matches!(
        AffineFaceMesh::merge_with_budget(
            &[applications[0].clone(), applications[0].clone()],
            0.02,
            AffineFaceMergeLimits::default(),
            budget.clone(),
            || false,
        ),
        Ok(Err(AffineFaceMergeRefusal::DuplicateEdge))
    ));
    assert_eq!(budget.used(), used);
    assert!(matches!(
        AffineFaceMesh::merge_with_budget(
            &applications,
            0.02,
            AffineFaceMergeLimits {
                max_operations: 1,
                ..Default::default()
            },
            budget.clone(),
            || false,
        ),
        Err(SurfaceError::Limit)
    ));
    assert!(matches!(
        AffineFaceMesh::merge_with_budget(
            &applications,
            0.02,
            AffineFaceMergeLimits::default(),
            budget.clone(),
            || true,
        ),
        Err(SurfaceError::Cancelled)
    ));
    let empty = MemoryBudget::new(0);
    assert!(matches!(
        AffineFaceMesh::merge_with_budget(
            &applications,
            0.02,
            AffineFaceMergeLimits::default(),
            empty.clone(),
            || false,
        ),
        Err(SurfaceError::Memory)
    ));
    assert_eq!(empty.used(), 0);
    let scratch_refused = MemoryBudget::new(merged.referenced_bytes());
    assert!(matches!(
        AffineFaceMesh::merge_with_budget(
            &applications,
            0.02,
            AffineFaceMergeLimits::default(),
            scratch_refused.clone(),
            || false,
        ),
        Err(SurfaceError::Memory)
    ));
    assert_eq!(scratch_refused.used(), 0);
    assert_eq!(budget.used(), used);
    drop((reversed, merged, applications, grids, affine, field));
    assert_eq!(budget.used(), 0);
}

#[test]
fn mixed_coordination_rechecks_generations_and_merges_current_faces() {
    let budget = MemoryBudget::new(512 * 1024 * 1024);
    let field = fractional(source([3, 3, 2], true, false), |[x, y, z]| {
        -10 + 4 * x as i16
            + 4 * y as i16
            + 8 * z as i16
            + if [x, y, z] == [2, 1, 1] { -24 } else { 0 }
            + if [x, y, z] == [1, 2, 1] { -24 } else { 0 }
    });
    let mut grids = Vec::new();
    for origin in [[0, 0, 0], [1, 0, 0], [0, 1, 0]] {
        grids.push(
            field
                .candidate_grid_with_budget(origin, [1; 3], 2, limits(), budget.clone(), || false)
                .unwrap(),
        );
    }
    let assembly = Arc::new(
        CandidateAssembly::build_with_budget(
            &[&grids[2], &grids[0], &grids[1]],
            assembly_limits(),
            budget.clone(),
            || false,
        )
        .unwrap(),
    );
    let result = MixedCoordination::coordinate_with_budget(
        &assembly,
        MixedCoordinationLimits::default(),
        budget.clone(),
        || false,
    )
    .unwrap();
    assert!(Arc::ptr_eq(result.assembly(), &assembly));
    assert_eq!(result.faces().len(), 2);
    assert_eq!(result.general_state(), MixedGeneralState::Current);
    assert!(result.rounds() <= MixedCoordinationLimits::default().max_rounds);
    assert!(result.candidate_updates() > 0);
    assert!(result.rounds() > 1);
    let cell_index = assembly
        .cells()
        .iter()
        .position(|cell| cell.origin == [0, 0, 0])
        .unwrap();
    assert_eq!(
        result.affine_states()[cell_index],
        MixedAffineState::Ready {
            applied: 2,
            listed: 2
        }
    );
    let merged = result.affine_meshes()[cell_index]
        .as_ref()
        .expect("expected merged planar geometry");
    assert_eq!(merged.applications().len(), 2);
    for face in result.faces() {
        let MixedFaceState::Applied {
            points,
            spans,
            missing_points,
            missing_spans,
            uncertified_spans,
        } = face.state()
        else {
            panic!(
                "face {:?} did not converge: {:?}",
                face.face(),
                face.state()
            );
        };
        assert!(points >= 2 && spans >= 1);
        let application = face
            .application()
            .expect("applied face must retain its source");
        assert_eq!(
            missing_points as usize,
            application
                .point_vertices()
                .iter()
                .filter(|point| point.contains(&None))
                .count()
        );
        assert_eq!(
            missing_spans as usize,
            application
                .spans()
                .iter()
                .filter(|span| span.segments.contains(&None))
                .count()
        );
        assert_eq!(
            uncertified_spans as usize,
            application
                .spans()
                .iter()
                .filter(|span| span.segments[0].is_some() && !span.candidate_within_tolerance)
                .count()
        );
        let mesh = usize::from(face.candidate_mesh().unwrap());
        assert!(Arc::ptr_eq(
            application.input().candidate(),
            &result.meshes()[mesh]
        ));
        assert_eq!(application.input().face(), face.face());
        assert!(
            merged
                .applications()
                .iter()
                .any(|other| Arc::ptr_eq(other, application))
        );
        let output = application.candidate();
        assert_eq!(
            output.triangles().len(),
            result.meshes()[mesh].triangles().len()
        );
        for (expected, actual) in output
            .triangles()
            .iter()
            .zip(result.meshes()[mesh].triangles())
        {
            assert_eq!(expected.vertices, actual.vertices);
            assert_eq!(expected.source_facet, actual.source_facet);
            assert_eq!(
                matches!(expected.state, AppliedTriangleState::WithinTolerance { .. }),
                matches!(actual.state, ConformingFacetState::WithinTolerance { .. })
            );
        }
    }
    let baseline = budget.used();
    let limited = MixedCoordination::coordinate_with_budget(
        &assembly,
        MixedCoordinationLimits {
            max_face_visits: 0,
            ..Default::default()
        },
        budget.clone(),
        || false,
    )
    .unwrap();
    assert!(limited.faces().iter().all(|face| matches!(
        face.state(),
        MixedFaceState::Unresolved {
            reason: MixedFaceStop::NotRechecked
        }
    )));
    assert_eq!(
        limited.affine_states()[cell_index],
        MixedAffineState::NoReadyFaces { listed: 2 }
    );
    assert!(limited.affine_meshes()[cell_index].is_none());
    drop(limited);
    assert_eq!(budget.used(), baseline);
    let one_round = MixedCoordination::coordinate_with_budget(
        &assembly,
        MixedCoordinationLimits {
            max_rounds: 1,
            ..Default::default()
        },
        budget.clone(),
        || false,
    )
    .unwrap();
    assert_eq!(one_round.general_state(), MixedGeneralState::NotRechecked);
    assert!(one_round.faces().iter().any(|face| matches!(
        face.state(),
        MixedFaceState::Unresolved {
            reason: MixedFaceStop::NotRechecked
        }
    )));
    for face in one_round.faces() {
        if let Some(application) = face.application() {
            let id = usize::from(face.candidate_mesh().unwrap());
            assert!(Arc::ptr_eq(
                application.input().candidate(),
                &one_round.meshes()[id]
            ));
        }
    }
    drop(one_round);
    assert_eq!(budget.used(), baseline);
    let no_updates = MixedCoordination::coordinate_with_budget(
        &assembly,
        MixedCoordinationLimits {
            max_candidate_updates: 0,
            ..Default::default()
        },
        budget.clone(),
        || false,
    )
    .unwrap();
    assert_eq!(no_updates.candidate_updates(), 0);
    assert!(
        no_updates
            .meshes()
            .iter()
            .zip(assembly.meshes())
            .all(|(a, b)| Arc::ptr_eq(a, b))
    );
    drop(no_updates);
    assert_eq!(budget.used(), baseline);
    let mut general_limited = MixedCoordinationLimits::default();
    general_limited.general.max_faces = 0;
    let general_limited = MixedCoordination::coordinate_with_budget(
        &assembly,
        general_limited,
        budget.clone(),
        || false,
    )
    .unwrap();
    assert_eq!(
        general_limited.general_state(),
        MixedGeneralState::Unresolved(SurfaceError::Limit)
    );
    assert!(general_limited.general_faces().is_empty());
    drop(general_limited);
    assert_eq!(budget.used(), baseline);
    let calls = Cell::new(0usize);
    let counted = MixedCoordination::coordinate_with_budget(
        &assembly,
        MixedCoordinationLimits::default(),
        budget.clone(),
        || {
            calls.set(calls.get() + 1);
            false
        },
    )
    .unwrap();
    let total_calls = calls.get();
    drop(counted);
    for stop in [total_calls / 2, total_calls] {
        let calls = Cell::new(0usize);
        assert!(matches!(
            MixedCoordination::coordinate_with_budget(
                &assembly,
                MixedCoordinationLimits::default(),
                budget.clone(),
                || {
                    calls.set(calls.get() + 1);
                    calls.get() >= stop
                },
            ),
            Err(SurfaceError::Cancelled)
        ));
        assert_eq!(budget.used(), baseline);
    }
    assert!(matches!(
        MixedCoordination::coordinate_with_budget(
            &assembly,
            MixedCoordinationLimits {
                max_rounds: 0,
                ..Default::default()
            },
            budget.clone(),
            || false,
        ),
        Err(SurfaceError::Limit)
    ));
    assert!(matches!(
        MixedCoordination::coordinate_with_budget(
            &assembly,
            MixedCoordinationLimits::default(),
            budget.clone(),
            || true,
        ),
        Err(SurfaceError::Cancelled)
    ));
    let empty = MemoryBudget::new(0);
    assert!(matches!(
        MixedCoordination::coordinate_with_budget(
            &assembly,
            MixedCoordinationLimits::default(),
            empty.clone(),
            || false,
        ),
        Err(SurfaceError::Memory)
    ));
    assert_eq!(empty.used(), 0);
    let scratch_refused = MemoryBudget::new(result.referenced_bytes());
    assert!(matches!(
        MixedCoordination::coordinate_with_budget(
            &assembly,
            MixedCoordinationLimits::default(),
            scratch_refused.clone(),
            || false,
        ),
        Err(SurfaceError::Memory)
    ));
    assert_eq!(scratch_refused.used(), 0);
    assert_eq!(budget.used(), baseline);
    drop((result, assembly, grids, field));
    assert_eq!(budget.used(), 0);
}

#[test]
fn mixed_coordination_keeps_end_face_or_unavailable_neighbor_explicit() {
    let budget = MemoryBudget::new(512 * 1024 * 1024);
    let field = fractional(source([3, 3, 3], true, false), |[x, y, z]| {
        -10 + 4 * x as i16
            + 4 * y as i16
            + 8 * z as i16
            + if [x, y, z] == [2, 1, 1] { -24 } else { 0 }
            + if [x, y, z] == [1, 2, 1] { -24 } else { 0 }
            + if [x, y, z] == [1, 1, 2] { -24 } else { 0 }
    });
    let mut grids = Vec::new();
    for origin in [[0, 0, 0], [1, 0, 0], [0, 1, 0], [0, 0, 1]] {
        grids.push(
            field
                .candidate_grid_with_budget(origin, [1; 3], 2, limits(), budget.clone(), || false)
                .unwrap(),
        );
    }
    let assembly = Arc::new(
        CandidateAssembly::build_with_budget(
            &[&grids[0], &grids[1], &grids[2], &grids[3]],
            assembly_limits(),
            budget.clone(),
            || false,
        )
        .unwrap(),
    );
    let result = MixedCoordination::coordinate_with_budget(
        &assembly,
        MixedCoordinationLimits::default(),
        budget.clone(),
        || false,
    )
    .unwrap();
    let face = result
        .faces()
        .iter()
        .find(|face| face.face().axis == 2)
        .expect("expected the listed end face");
    match face.candidate_mesh() {
        Some(_) => assert_eq!(
            face.state(),
            MixedFaceState::Unresolved {
                reason: MixedFaceStop::Unsupported(MixedFaceUnsupported::ProjectionEndFace)
            }
        ),
        None => assert_eq!(
            face.state(),
            MixedFaceState::Unresolved {
                reason: MixedFaceStop::CandidateUnavailable
            }
        ),
    }
    assert!(face.application().is_none());
    let cell = assembly
        .cells()
        .iter()
        .position(|cell| cell.origin == [0, 0, 0])
        .unwrap();
    assert_eq!(
        result.affine_states()[cell],
        MixedAffineState::Ready {
            applied: 2,
            listed: 3
        }
    );
    assert_eq!(
        result.affine_meshes()[cell]
            .as_ref()
            .unwrap()
            .applications()
            .len(),
        2
    );
    drop((result, assembly, grids, field));
    assert_eq!(budget.used(), 0);
}
