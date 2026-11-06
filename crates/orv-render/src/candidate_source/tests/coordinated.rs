// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use orv_core::surface::field::{MixedCoordination, MixedCoordinationLimits, MixedFaceState};
use std::collections::BTreeMap;

fn fixture(
    source: Arc<Volume>,
    tolerance: f64,
) -> (Arc<CandidateAssembly>, Arc<MixedCoordination>) {
    assert_eq!(source.dimensions(), [3, 3, 2]);
    let mut payload = Vec::new();
    for z in 0..2 {
        for y in 0..3 {
            for x in 0..3 {
                let value = -10
                    + 4 * x as i16
                    + 4 * y as i16
                    + 8 * z as i16
                    + if [x, y, z] == [2, 1, 1] { -24 } else { 0 }
                    + if [x, y, z] == [1, 2, 1] { -24 } else { 0 };
                payload.push(((value + 200) / 2) as u8);
            }
        }
    }
    let masks = Arc::new(
        Segments::from_fractional(
            source,
            &[1],
            &[(1, 0), (1, 1)],
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
    let field = MaskField::new(MaskOwner::Supplied(masks), 1).unwrap();
    let limits = grid_limits(tolerance, 4);
    let grids: Vec<_> = [[0, 0, 0], [1, 0, 0], [0, 1, 0]]
        .into_iter()
        .map(|origin| {
            field
                .candidate_grid(origin, [1; 3], 2, limits, || false)
                .unwrap()
        })
        .collect();
    let assembly = Arc::new(
        CandidateAssembly::build(
            &[&grids[0], &grids[1], &grids[2]],
            CandidateAssemblyLimits {
                max_cells: 256,
                max_directory_bytes: 1024 * 1024,
                seams: limits.seams,
            },
            || false,
        )
        .unwrap(),
    );
    let coordinated = Arc::new(
        MixedCoordination::coordinate(&assembly, MixedCoordinationLimits::default(), || false)
            .unwrap(),
    );
    (assembly, coordinated)
}

fn cpu_records(coordination: &MixedCoordination) -> BTreeMap<[u32; 3], Vec<[u32; 16]>> {
    let assembly = coordination.assembly();
    let mut records = BTreeMap::new();
    for (index, cell) in assembly.cells().iter().enumerate() {
        let origin = cell.origin.map(|v| v as u32);
        let source_cell = assembly.field().cell(cell.origin).unwrap();
        let samples: [i16; 8] = source_cell.corners().map(|sample| {
            assert!(sample.source_valid);
            sample.value.unwrap()
        });
        let encode = |positions: [[u32; 3]; 3]| {
            let mut words = [0u32; 16];
            words[..3].copy_from_slice(&origin);
            for (vertex, position) in positions.iter().enumerate() {
                words[3 + 3 * vertex..6 + 3 * vertex].copy_from_slice(position);
            }
            for i in 0..4 {
                words[12 + i] =
                    u32::from(samples[2 * i] as u16) | (u32::from(samples[2 * i + 1] as u16) << 16);
            }
            words
        };
        let mut triangles = Vec::new();
        if let Some(affine) = &coordination.affine_meshes()[index] {
            for triangle in affine.triangles() {
                triangles.push(encode(triangle.map(|id| affine.vertices()[id as usize])));
            }
        } else if let CandidateCellState::Candidate { mesh } = cell.state {
            let mesh = &coordination.meshes()[usize::from(mesh)];
            for triangle in mesh.triangles() {
                if matches!(triangle.state, ConformingFacetState::WithinTolerance { .. }) {
                    triangles.push(encode(
                        triangle
                            .vertices
                            .map(|id| mesh.vertices()[id as usize].position().unwrap()),
                    ));
                }
            }
        }
        if !triangles.is_empty() {
            records.insert(origin, triangles);
        }
    }
    records
}

fn contains_edge(records: &[[u32; 16]], edge: [[u32; 3]; 2]) -> bool {
    records.iter().any(|record| {
        let positions = [
            [record[3], record[4], record[5]],
            [record[6], record[7], record[8]],
            [record[9], record[10], record[11]],
        ];
        positions.contains(&edge[0]) && positions.contains(&edge[1])
    })
}

#[derive(Clone, Copy)]
struct CpuTriangle {
    cell: [u32; 3],
    points: [[f64; 3]; 3],
}
#[derive(Clone, Copy)]
struct CpuHit {
    cell: [u32; 3],
    depth: f64,
    margin: f64,
    voxel: [f64; 3],
}
fn cpu_triangles(records: &BTreeMap<[u32; 3], Vec<[u32; 16]>>) -> Vec<CpuTriangle> {
    let denominator = f64::from(ConformingSurfaceMesh::POSITION_DENOMINATOR);
    records
        .iter()
        .flat_map(|(cell, records)| {
            records.iter().map(move |record| CpuTriangle {
                cell: *cell,
                points: std::array::from_fn(|vertex| {
                    std::array::from_fn(|axis| {
                        f64::from(cell[axis])
                            + f64::from(record[3 + 3 * vertex + axis]) / denominator
                    })
                }),
            })
        })
        .collect()
}
fn cpu_hit(
    request: &CandidateProjection,
    triangles: &[CpuTriangle],
    pixel: [u32; 2],
    slack: f64,
) -> Option<CpuHit> {
    let source = request.surface.source().volume();
    let frame = request
        .camera
        .frame(
            &request.display,
            f64::from(request.size[0]) / f64::from(request.size[1]),
        )
        .unwrap();
    let (world, direction) = frame.ray(std::array::from_fn(|axis| {
        (f64::from(pixel[axis]) + 0.5) / f64::from(request.size[axis])
    }));
    let origin = source.world_to_voxel(world);
    let vector = source.world_vector_to_voxel(direction);
    let mut best: Option<(f64, CpuHit)> = None;
    for triangle in triangles {
        let e1 = sub(triangle.points[1], triangle.points[0]);
        let e2 = sub(triangle.points[2], triangle.points[0]);
        let p = cross(vector, e2);
        let determinant = dot(e1, p);
        if determinant.abs() < 1e-12 {
            continue;
        }
        let delta = sub(origin, triangle.points[0]);
        let u = dot(delta, p) / determinant;
        let q = cross(delta, e1);
        let v = dot(vector, q) / determinant;
        let margin = u.min(v).min(1.0 - u - v);
        if margin < -slack {
            continue;
        }
        let t = dot(e2, q) / determinant;
        let voxel = std::array::from_fn(|axis| origin[axis] + vector[axis] * t);
        let depth = dot(
            sub(source.voxel_to_world(voxel), source.center()),
            direction,
        );
        if best.as_ref().is_none_or(|(prior, _)| t < *prior) {
            best = Some((
                t,
                CpuHit {
                    cell: triangle.cell,
                    depth,
                    margin,
                    voxel,
                },
            ));
        }
    }
    best.map(|(_, hit)| hit)
}

fn edge_distance(point: [f64; 3], edge: [[f64; 3]; 2]) -> f64 {
    let direction = sub(edge[1], edge[0]);
    let relative = sub(point, edge[0]);
    let length = dot(direction, direction);
    let t = if length == 0.0 {
        0.0
    } else {
        (dot(relative, direction) / length).clamp(0.0, 1.0)
    };
    let difference = std::array::from_fn(|axis| relative[axis] - direction[axis] * t);
    dot(difference, difference).sqrt()
}

#[test]
fn coordinated_geometry_upload_and_gpu_projection_keep_current_shared_edges() {
    let (device, queue) = device();
    let volume = volume_geometry([3, 3, 2], 1e6, true, true);
    let source = GpuVolumeSource::new(&device, &queue, volume.clone(), || false).unwrap();
    let (assembly, coordination) = fixture(volume.clone(), 100.0);
    assert_eq!(coordination.faces().len(), 2);
    assert!(
        coordination
            .faces()
            .iter()
            .all(|face| matches!(face.state(), MixedFaceState::Applied { .. }))
    );
    let (baseline, _) = GpuCandidateSurface::upload(
        source.clone(),
        assembly.clone(),
        None,
        CandidateUploadLimits::default(),
        || false,
    )
    .unwrap();
    let (surface, stats) = GpuCandidateSurface::upload_coordinated(
        source.clone(),
        coordination.clone(),
        Some(baseline.clone()),
        CandidateUploadLimits::default(),
        || false,
    )
    .unwrap();
    assert!(!Arc::ptr_eq(&surface, &baseline));
    assert!(Arc::ptr_eq(surface.coordination().unwrap(), &coordination));
    assert!(Arc::ptr_eq(surface.assembly(), &assembly));
    assert_eq!(stats.coordinated_affine_cells, 1);
    let records = cpu_records(&coordination);
    assert_eq!(surface.chunks().len(), records.len());
    for chunk in surface.chunks() {
        let expected = records.get(&chunk.origin()).unwrap();
        assert_eq!(&read(&source, chunk), expected);
        vertex_probe(&source, chunk, expected);
    }
    let mut certified_edges = Vec::new();
    for face in coordination.faces() {
        let application = face.application().unwrap();
        let affine_cell = usize::from(face.cells()[0]);
        let candidate_cell = usize::from(face.cells()[1]);
        let affine = coordination.affine_meshes()[affine_cell].as_ref().unwrap();
        let app_index = affine
            .applications()
            .iter()
            .position(|item| Arc::ptr_eq(item, application))
            .unwrap();
        let mapping = affine.face_point_vertices(app_index).unwrap();
        let candidate = &coordination.meshes()[usize::from(face.candidate_mesh().unwrap())];
        let affine_origin = assembly.cells()[affine_cell].origin.map(|v| v as u32);
        let candidate_origin = assembly.cells()[candidate_cell].origin.map(|v| v as u32);
        for span in application.spans() {
            if !span.candidate_within_tolerance {
                continue;
            }
            let Some(candidate_edge) = span.segments[0] else {
                continue;
            };
            let partition_span = &application.input().spans()[span.partition_span as usize];
            let affine_edge = partition_span
                .points
                .map(|id| affine.vertices()[mapping[id as usize].unwrap() as usize]);
            let candidate_edge =
                candidate_edge.map(|id| candidate.vertices()[id as usize].position().unwrap());
            let global = |origin: [u32; 3], local: [u32; 3]| -> [u64; 3] {
                std::array::from_fn(|axis| {
                    u64::from(origin[axis]) * u64::from(ConformingSurfaceMesh::POSITION_DENOMINATOR)
                        + u64::from(local[axis])
                })
            };
            assert_eq!(
                candidate_edge.map(|point| global(candidate_origin, point)),
                affine_edge.map(|point| global(affine_origin, point))
            );
            assert!(contains_edge(
                records.get(&candidate_origin).unwrap(),
                candidate_edge
            ));
            assert!(contains_edge(
                records.get(&affine_origin).unwrap(),
                affine_edge
            ));
            let denominator = f64::from(ConformingSurfaceMesh::POSITION_DENOMINATOR);
            certified_edges.push(affine_edge.map(|point| {
                let absolute = global(affine_origin, point);
                absolute.map(|value| value as f64 / denominator)
            }));
        }
    }
    assert!(
        !certified_edges.is_empty(),
        "fixture needs a represented certified shared edge"
    );
    let triangles = cpu_triangles(&records);
    let projector = GpuCandidateProjector::new(source.clone()).unwrap();
    let mut checked = 0usize;
    let mut planar = 0usize;
    let mut curved = 0usize;
    let mut near_planar = 0usize;
    let mut near_curved = 0usize;
    for (yaw, pitch) in [(27., -17.), (-35., 24.), (0., 0.)] {
        let mut camera = Camera::fit(&volume);
        camera.yaw_degrees = yaw;
        camera.pitch_degrees = pitch;
        let request = CandidateProjection {
            surface: surface.clone(),
            camera,
            display: ViewState::default(),
            clip: None,
            size: [97, 83],
        };
        let image = projector.render(request.clone(), || false).unwrap();
        let (cells, depths, _) = image_pixels(&image);
        for (index, (cell, depth)) in cells.iter().zip(&depths).enumerate() {
            let pixel = [(index % 97) as u32, (index / 97) as u32];
            if cell[3] != 0 {
                assert!(
                    cpu_hit(&request, &triangles, pixel, 0.003).is_some(),
                    "unexpected GPU hit at {pixel:?}"
                );
            }
            if let Some(hit) = cpu_hit(&request, &triangles, pixel, 0.0)
                && hit.margin > 0.003
            {
                assert_eq!(cell[..3], hit.cell, "pixel {pixel:?}");
                assert_eq!(cell[3], 1);
                assert!(
                    (f64::from(*depth) - hit.depth).abs() < 0.002,
                    "pixel {pixel:?}"
                );
                checked += 1;
                let near_edge = certified_edges
                    .iter()
                    .any(|edge| edge_distance(hit.voxel, *edge) < 0.12);
                if hit.cell == [0, 0, 0] {
                    planar += 1;
                    near_planar += usize::from(near_edge);
                } else {
                    curved += 1;
                    near_curved += usize::from(near_edge);
                }
            }
        }
    }
    assert!(
        checked > 30 && planar > 5 && curved > 5,
        "reference coverage: {checked}, {planar}, {curved}"
    );
    assert!(
        near_planar > 0 && near_curved > 0,
        "near-edge coverage: {near_planar}, {near_curved}"
    );
    let mut exact_edge_pixels = 0usize;
    for edge in certified_edges.iter().take(8) {
        let midpoint = std::array::from_fn(|axis| (edge[0][axis] + edge[1][axis]) * 0.5);
        for (yaw, pitch) in [(27., -17.), (-35., 24.), (0., 0.)] {
            let mut camera = Camera::fit(&volume);
            camera.target_mm = volume.voxel_to_world(midpoint);
            camera.yaw_degrees = yaw;
            camera.pitch_degrees = pitch;
            let request = CandidateProjection {
                surface: surface.clone(),
                camera,
                display: ViewState::default(),
                clip: None,
                size: [1, 1],
            };
            let Some(hit) = cpu_hit(&request, &triangles, [0, 0], 1e-8) else {
                continue;
            };
            let difference = sub(hit.voxel, midpoint);
            if dot(difference, difference).sqrt() > 1e-5 {
                continue;
            }
            let image = projector.render(request, || false).unwrap();
            let (cells, depths, _) = image_pixels(&image);
            assert_eq!(cells[0][3], 1, "missing GPU hit on represented shared edge");
            assert!(
                (f64::from(depths[0]) - hit.depth).abs() < 0.002,
                "edge {midpoint:?}, camera {yaw}/{pitch}, GPU {:?} depth {}, CPU {:?} depth {} margin {} voxel {:?}",
                cells[0],
                depths[0],
                hit.cell,
                hit.depth,
                hit.margin,
                hit.voxel,
            );
            exact_edge_pixels += 1;
        }
    }
    assert!(
        exact_edge_pixels > 0,
        "fixture needs a visible exact-edge pixel"
    );
    let (same, reuse) = GpuCandidateSurface::upload_coordinated(
        source.clone(),
        coordination.clone(),
        Some(surface.clone()),
        CandidateUploadLimits::default(),
        || false,
    )
    .unwrap();
    assert!(Arc::ptr_eq(&same, &surface));
    assert_eq!(reuse.kind, UploadKind::Reuse);
    let (back_to_assembly, back_stats) = GpuCandidateSurface::upload(
        source.clone(),
        assembly.clone(),
        Some(surface.clone()),
        CandidateUploadLimits::default(),
        || false,
    )
    .unwrap();
    assert!(!Arc::ptr_eq(&back_to_assembly, &surface));
    assert!(back_to_assembly.coordination().is_none());
    assert_eq!(back_stats.kind, UploadKind::Delta);
    let rebuilt = Arc::new(
        MixedCoordination::coordinate(&assembly, MixedCoordinationLimits::default(), || false)
            .unwrap(),
    );
    let (rebuilt_surface, delta) = GpuCandidateSurface::upload_coordinated(
        source.clone(),
        rebuilt,
        Some(surface.clone()),
        CandidateUploadLimits::default(),
        || false,
    )
    .unwrap();
    assert!(!Arc::ptr_eq(&rebuilt_surface, &surface));
    assert_eq!(delta.kind, UploadKind::Delta);
    assert_eq!(delta.reused_chunks, surface.chunks().len());
    let incomplete = Arc::new(
        MixedCoordination::coordinate(
            &assembly,
            MixedCoordinationLimits {
                max_face_visits: 0,
                ..Default::default()
            },
            || false,
        )
        .unwrap(),
    );
    let (partial, partial_stats) = GpuCandidateSurface::upload_coordinated(
        source.clone(),
        incomplete,
        Some(surface.clone()),
        CandidateUploadLimits::default(),
        || false,
    )
    .unwrap();
    assert!(!Arc::ptr_eq(&partial, &surface));
    assert!(partial.chunk([0, 0, 0]).is_none());
    assert!(partial_stats.unresolved_affine_cells >= 1);
    assert_eq!(partial_stats.incomplete_mixed_faces, 2);
    assert!(
        GpuCandidateSurface::upload_coordinated(
            source.clone(),
            coordination.clone(),
            Some(surface.clone()),
            CandidateUploadLimits {
                max_triangles: 0,
                ..Default::default()
            },
            || false,
        )
        .is_err()
    );
    assert!(
        GpuCandidateSurface::upload_coordinated(
            source.clone(),
            coordination.clone(),
            Some(surface.clone()),
            CandidateUploadLimits::default(),
            || true,
        )
        .is_err()
    );
    let other = GpuVolumeSource::new(
        &device,
        &queue,
        volume_geometry([3, 3, 2], 0., false, false),
        || false,
    )
    .unwrap();
    assert!(
        GpuCandidateSurface::upload_coordinated(
            other,
            coordination,
            None,
            CandidateUploadLimits::default(),
            || false,
        )
        .is_err()
    );
}

#[test]
fn coordinated_upload_late_cancellation_releases_admission() {
    let (device, queue) = device();
    let volume = volume();
    let source = GpuVolumeSource::new(&device, &queue, volume.clone(), || false).unwrap();
    let (assembly, coordination) = fixture(volume, 100.0);
    let cpu_before = MemoryBudget::volumes().used();
    let gpu_before = MemoryBudget::gpu().used();
    let max_directory = (assembly.cells().len() * std::mem::size_of::<Arc<GpuCandidateChunk>>()
        + std::mem::size_of::<GpuCandidateSurface>()
        + 2 * std::mem::size_of::<usize>()) as u64;
    let entered = AtomicBool::new(false);
    let result = GpuCandidateSurface::upload_coordinated(
        source.clone(),
        coordination.clone(),
        None,
        CandidateUploadLimits::default(),
        || {
            let hit = MemoryBudget::volumes().used() > cpu_before + max_directory;
            entered.fetch_or(hit, Ordering::AcqRel);
            hit
        },
    );
    assert!(matches!(result, Err("Cancelled")));
    assert!(entered.load(Ordering::Acquire));
    fence(&device);
    assert_eq!(MemoryBudget::volumes().used(), cpu_before);
    assert_eq!(MemoryBudget::gpu().used(), gpu_before);
}

#[test]
fn coordinated_upload_reports_uncertified_face_spans() {
    let (device, queue) = device();
    let volume = volume();
    let source = GpuVolumeSource::new(&device, &queue, volume.clone(), || false).unwrap();
    let (_assembly, coordination) = fixture(volume, 0.02);
    let uncertified: usize = coordination
        .faces()
        .iter()
        .map(|face| match face.state() {
            MixedFaceState::Applied {
                uncertified_spans, ..
            } => uncertified_spans as usize,
            MixedFaceState::Unresolved { .. } => 0,
        })
        .sum();
    assert!(uncertified > 0, "fixture needs uncertified shared spans");
    let (surface, stats) = GpuCandidateSurface::upload_coordinated(
        source.clone(),
        coordination.clone(),
        None,
        CandidateUploadLimits::default(),
        || false,
    )
    .unwrap();
    assert!(stats.incomplete_mixed_faces > 0);
    assert!(Arc::ptr_eq(surface.coordination().unwrap(), &coordination));
    let records = cpu_records(&coordination);
    for chunk in surface.chunks() {
        assert_eq!(&read(&source, chunk), records.get(&chunk.origin()).unwrap());
    }
}
