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
fn changed_revision_reuses_only_identical_pre_neighborhood_dispositions() {
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
            .filter(|cell| {
                matches!(
                    cell.state,
                    CandidateCellState::Graph(
                        CellGraphState::Unavailable { .. }
                            | CellGraphState::NoInteriorInterface
                            | CellGraphState::Family { .. }
                    )
                ) && before.cell(cell.origin).unwrap() == after.cell(cell.origin).unwrap()
            })
            .count();
        assert!(expected_reused > 0 && expected_reused < old.cells().len());
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
