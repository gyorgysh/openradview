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
