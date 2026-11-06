// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use orv_core::surface::field::{AffineDisposition, AffineFaceState, AffineFaceTrace};

fn planar_assembly(source: Arc<Volume>, tolerance: f64) -> Arc<CandidateAssembly> {
    planar_assembly_values(source, tolerance, [-4, 2, 8])
}
fn planar_assembly_values(
    source: Arc<Volume>,
    tolerance: f64,
    values: [i16; 3],
) -> Arc<CandidateAssembly> {
    let [w, h, d] = source.dimensions();
    assert_eq!([w, h, d], [3, 3, 2]);
    let mut payload = Vec::new();
    for _z in 0..d {
        for _y in 0..h {
            for value in values.iter().take(w) {
                payload.push((value + 200) as u8 / 2);
            }
        }
    }
    let mask = Arc::new(
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
    let field = MaskField::new(MaskOwner::Supplied(mask), 1).unwrap();
    let limits = grid_limits(tolerance, 2);
    let grid = field
        .candidate_grid([0; 3], [2, 2, 1], 2, limits, || false)
        .unwrap();
    assembly(&grid, limits.seams)
}
#[test]
fn affine_candidates_project_on_gpu_against_independent_physical_plane() {
    let (device, queue) = device();
    let cpu = volume_geometry([3, 3, 2], 1e6, true, true);
    let source = GpuVolumeSource::new(&device, &queue, cpu.clone(), || false).unwrap();
    let assembly = planar_assembly(cpu.clone(), 0.01);
    assert_eq!(
        assembly
            .affine_cells()
            .iter()
            .filter(|cell| cell.disposition() == AffineDisposition::Plane)
            .count(),
        2
    );
    assert!(assembly.affine_faces().iter().any(|face| matches!(
        face.state(),
        AffineFaceState::Matched(AffineFaceTrace::Segment(_))
    )));
    let (surface, stats) = GpuCandidateSurface::upload(
        source.clone(),
        assembly.clone(),
        None,
        CandidateUploadLimits::default(),
        || false,
    )
    .unwrap();
    assert_eq!(stats.affine_cells, 2);
    assert_eq!(stats.unresolved_affine_cells, 0);
    assert_eq!(stats.represented_triangles, 4);
    assert!(stats.affine_error_upper_mm > 0. && stats.affine_error_upper_mm <= 0.01);
    assert_eq!(surface.affine_error_upper_mm(), stats.affine_error_upper_mm);
    assert_eq!(surface.unresolved_affine_cells(), 0);
    let denominator = f64::from(ConformingSurfaceMesh::POSITION_DENOMINATOR);
    for chunk in surface.chunks() {
        let ChunkGeometry::Affine(geometry) = chunk.geometry() else {
            panic!("Expected affine geometry")
        };
        let cell = assembly
            .affine_cells()
            .iter()
            .find(|cell| cell.cell().origin() == chunk.origin())
            .unwrap();
        for (actual, exact) in geometry.vertices.iter().zip(cell.vertices()) {
            let delta = std::array::from_fn(|axis| {
                f64::from(actual[axis]) / denominator - exact[axis].as_f64()
            });
            let displacement = cpu.voxel_vector_to_world(delta);
            let distance = dot(displacement, displacement).sqrt();
            assert!(distance <= geometry.error_mm && distance <= stats.affine_error_upper_mm);
        }
    }
    let projector = GpuCandidateProjector::new(source.clone()).unwrap();
    let mut camera = Camera::fit(&cpu);
    camera.yaw_degrees = 38.;
    camera.pitch_degrees = -19.;
    let request = CandidateProjection {
        surface: surface.clone(),
        camera,
        display: ViewState::default(),
        clip: None,
        size: [97, 83],
    };
    let image = projector.render(request.clone(), || false).unwrap();
    let (cells, depths, normals) = image_pixels(&image);
    let frame = request
        .camera
        .frame(
            &request.display,
            f64::from(request.size[0]) / f64::from(request.size[1]),
        )
        .unwrap();
    let mut checked = 0;
    let physical = cpu.world_vector_to_voxel([1., 0., 0.]);
    let direction_normal = cpu.world_vector_to_voxel([0., 1., 0.]);
    let third = cpu.world_vector_to_voxel([0., 0., 1.]);
    let normal = [-physical[0], -direction_normal[0], -third[0]];
    let length = dot(normal, normal).sqrt();
    let normal = normal.map(|v| v / length);
    for (i, ((cell, depth), actual_normal)) in cells
        .iter()
        .zip(depths.iter())
        .zip(normals.iter())
        .enumerate()
    {
        let pixel = [(i % 97) as u32, (i / 97) as u32];
        let (world, direction) = frame.ray([
            (f64::from(pixel[0]) + 0.5) / 97.,
            (f64::from(pixel[1]) + 0.5) / 83.,
        ]);
        let origin = cpu.world_to_voxel(world);
        let ray = cpu.world_vector_to_voxel(direction);
        if ray[0].abs() < 1e-10 {
            continue;
        }
        let t = (2. / 3. - origin[0]) / ray[0];
        let point = std::array::from_fn::<_, 3, _>(|a| origin[a] + ray[a] * t);
        let inside = point[1] > 0.01
            && point[1] < 1.99
            && point[2] > 0.01
            && point[2] < 0.99
            && (point[1] - 1.).abs() > 0.01;
        if !inside {
            continue;
        }
        let world_point = cpu.voxel_to_world(point);
        let expected_depth = dot(sub(world_point, cpu.center()), direction);
        assert_eq!(*cell, [0, point[1].floor() as u32, 0, 1], "{pixel:?}");
        assert!(
            (f64::from(*depth) - expected_depth).abs() < 0.002,
            "{pixel:?} {depth} {expected_depth}"
        );
        assert_eq!(actual_normal[3], 1.);
        for axis in 0..3 {
            assert!((actual_normal[axis] - normal[axis]).abs() < 0.004);
        }
        checked += 1;
    }
    assert!(
        checked > 20,
        "fixture must exercise the plane interior: {checked}"
    );
    let mut volume = VolumeView::fit(&cpu);
    volume.projection = Projection::Composite;
    volume.camera = request.camera.clone();
    let scene = CandidateOpaqueScene {
        surface: image,
        navigation: request.display.clone(),
        volume,
        material: candidate_material(0.2, 0.),
        lighting: Lighting::UNLIT,
        surface_lighting: Lighting::UNLIT,
        surface_luminance: 0.8,
    };
    let composed = CandidateOpaqueRenderer::new(source.clone(), wgpu::TextureFormat::Rgba8Unorm)
        .unwrap()
        .render(scene, || false)
        .unwrap();
    for (pixel, cell) in optical_pixels(&composed).iter().zip(&cells) {
        let expected = if cell[3] == 0 { 0.0030352698 } else { 0.8 };
        assert!((pixel[0] - expected).abs() < 0.001);
    }
    let grid = assembly
        .field()
        .candidate_grid([0; 3], [2, 2, 1], 2, grid_limits(0.01, 2), || false)
        .unwrap();
    let rebuilt = super::assembly(&grid, grid_limits(0.01, 2).seams);
    let (related, update) = GpuCandidateSurface::upload(
        source.clone(),
        rebuilt,
        Some(surface.clone()),
        CandidateUploadLimits::default(),
        || false,
    )
    .unwrap();
    assert_eq!(update.kind, UploadKind::Delta);
    assert_eq!(update.reused_chunks, 2);
    for chunk in related.chunks() {
        assert!(Arc::ptr_eq(chunk, surface.chunk(chunk.origin()).unwrap()));
    }
    let (_, again) = GpuCandidateSurface::upload(
        source,
        assembly,
        Some(surface),
        CandidateUploadLimits::default(),
        || false,
    )
    .unwrap();
    assert_eq!(again.kind, UploadKind::Reuse);
    assert_eq!(again.reused_chunks, 2);
}
#[test]
fn affine_quantization_respects_tolerance_and_atomic_cancellation() {
    let (device, queue) = device();
    let cpu = volume();
    let source = GpuVolumeSource::new(&device, &queue, cpu.clone(), || false).unwrap();
    let (surface, stats) = GpuCandidateSurface::upload(
        source.clone(),
        planar_assembly(cpu.clone(), 0.),
        None,
        CandidateUploadLimits::default(),
        || false,
    )
    .unwrap();
    assert_eq!(stats.affine_cells, 0);
    assert_eq!(stats.unresolved_affine_cells, 2);
    assert_eq!(surface.unresolved_affine_cells(), 2);
    assert!(surface.chunks().is_empty());
    let (exact, exact_stats) = GpuCandidateSurface::upload(
        source.clone(),
        planar_assembly_values(cpu.clone(), 0., [-2, 2, 6]),
        None,
        CandidateUploadLimits::default(),
        || false,
    )
    .unwrap();
    assert_eq!(exact_stats.affine_cells, 2);
    assert_eq!(exact_stats.affine_error_upper_mm, 0.);
    assert_eq!(exact.unresolved_affine_cells(), 0);
    let assembly = planar_assembly(cpu, 0.01);
    let cancel = Cell::new(0);
    assert!(
        GpuCandidateSurface::upload(
            source,
            assembly,
            None,
            CandidateUploadLimits::default(),
            || {
                cancel.set(cancel.get() + 1);
                cancel.get() >= 5
            }
        )
        .is_err()
    );
    assert!(surface.chunks().is_empty());
}

#[test]
fn oblique_affine_cells_project_as_one_physical_plane() {
    let (device, queue) = device();
    let cpu = volume_geometry([3, 3, 2], 1000., true, true);
    let source = GpuVolumeSource::new(&device, &queue, cpu.clone(), || false).unwrap();
    let mut payload = Vec::new();
    for z in 0..2 {
        for y in 0..3 {
            for x in 0..3 {
                let field = -6 + 2 * x + 4 * y + 6 * z;
                payload.push(((field + 200) / 2) as u8);
            }
        }
    }
    let mask = Arc::new(
        Segments::from_fractional(
            cpu.clone(),
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
    let field = MaskField::new(MaskOwner::Supplied(mask), 1).unwrap();
    let limits = grid_limits(0.01, 2);
    let grid = field
        .candidate_grid([0; 3], [2, 2, 1], 2, limits, || false)
        .unwrap();
    let assembly = super::assembly(&grid, limits.seams);
    let (surface, stats) = GpuCandidateSurface::upload(
        source.clone(),
        assembly,
        None,
        CandidateUploadLimits::default(),
        || false,
    )
    .unwrap();
    assert!(stats.affine_cells >= 2);
    assert_eq!(stats.unresolved_affine_cells, 0);
    let mut camera = Camera::fit(&cpu);
    camera.yaw_degrees = 38.;
    camera.pitch_degrees = -19.;
    let request = CandidateProjection {
        surface,
        camera,
        display: ViewState::default(),
        clip: None,
        size: [97, 83],
    };
    let image = GpuCandidateProjector::new(source)
        .unwrap()
        .render(request.clone(), || false)
        .unwrap();
    let (cells, depths, normals) = image_pixels(&image);
    let frame = request.camera.frame(&request.display, 97. / 83.).unwrap();
    let gradient = [2., 4., 6.];
    let physical: [f64; 3] = std::array::from_fn(|axis| {
        let mut unit = [0.; 3];
        unit[axis] = 1.;
        -dot(gradient, cpu.world_vector_to_voxel(unit))
    });
    let norm = dot(physical, physical).sqrt();
    let expected_normal = physical.map(|v| v / norm);
    let mut checked = 0;
    for (i, ((cell, depth), normal)) in cells.iter().zip(&depths).zip(&normals).enumerate() {
        let pixel = [(i % 97) as u32, (i / 97) as u32];
        let (world, direction) = frame.ray([
            (f64::from(pixel[0]) + 0.5) / 97.,
            (f64::from(pixel[1]) + 0.5) / 83.,
        ]);
        let origin = cpu.world_to_voxel(world);
        let ray = cpu.world_vector_to_voxel(direction);
        let denominator = dot(gradient, ray);
        if denominator.abs() < 1e-10 {
            continue;
        }
        let t = (6. - dot(gradient, origin)) / denominator;
        let point = std::array::from_fn::<_, 3, _>(|axis| origin[axis] + ray[axis] * t);
        if point[0] <= 0.01
            || point[0] >= 1.99
            || point[1] <= 0.01
            || point[1] >= 1.99
            || point[2] <= 0.01
            || point[2] >= 0.99
            || (point[0] - 1.).abs() < 0.01
            || (point[1] - 1.).abs() < 0.01
        {
            continue;
        }
        let expected_depth = dot(sub(cpu.voxel_to_world(point), cpu.center()), direction);
        assert_eq!(
            *cell,
            [point[0].floor() as u32, point[1].floor() as u32, 0, 1],
            "pixel {pixel:?}"
        );
        assert!((f64::from(*depth) - expected_depth).abs() < 0.002);
        assert_eq!(normal[3], 1.);
        for axis in 0..3 {
            assert!((normal[axis] - expected_normal[axis]).abs() < 0.004);
        }
        checked += 1;
    }
    assert!(
        checked > 20,
        "fixture must exercise the oblique plane: {checked}"
    );
}
