// SPDX-License-Identifier: GPL-3.0-or-later
//! Bounded reconstruction of one affine polygon from multiple checked faces.
use super::*;
use crate::surface::field::{AffineFaceMergeLimits, AffineFaceMergeRefusal, AffineFaceMesh};

// Boundary points on the same original polygon edge are exactly collinear,
// even when their rounded grid representatives are not. Do not clip an ear
// whose exact reference triangle would therefore collapse.
fn triangulate_ears<F: Fn() -> bool>(
    vertices: &[[u32; 3]],
    edge_masks: &[u8],
    axes: [usize; 2],
    orientation: i128,
    triangles: &mut Vec<[u32; 3]>,
    work: &mut Work<'_, F>,
) -> Result<bool, SurfaceError> {
    let n = vertices.len();
    if n < 3 || edge_masks.len() != n {
        return Err(SurfaceError::Geometry);
    }
    let mut previous = exact(n)?;
    let mut next = exact(n)?;
    for i in 0..n {
        work.charge(1)?;
        previous.push(u32::try_from((i + n - 1) % n).map_err(|_| SurfaceError::Limit)?);
        next.push(u32::try_from((i + 1) % n).map_err(|_| SurfaceError::Limit)?);
    }
    let mut remaining = n;
    let mut current = 0usize;
    while remaining > 3 {
        let mut clipped = false;
        let attempts = remaining;
        for _ in 0..attempts {
            work.charge(1)?;
            let a = previous[current] as usize;
            let b = next[current] as usize;
            if edge_masks[a] & edge_masks[current] & edge_masks[b] == 0
                && turn(vertices[a], vertices[current], vertices[b], axes).signum() == orientation
            {
                let mut scan = next[b] as usize;
                let mut occupied = false;
                let mut remaining_edge = edge_masks[a] & edge_masks[b];
                while scan != a {
                    work.charge(1)?;
                    remaining_edge &= edge_masks[scan];
                    let p = vertices[scan];
                    if turn(vertices[a], vertices[current], p, axes).signum() * orientation >= 0
                        && turn(vertices[current], vertices[b], p, axes).signum() * orientation >= 0
                        && turn(vertices[b], vertices[a], p, axes).signum() * orientation >= 0
                    {
                        occupied = true;
                        break;
                    }
                    scan = next[scan] as usize;
                }
                if !occupied && remaining_edge == 0 {
                    triangles.push([
                        u32::try_from(a).map_err(|_| SurfaceError::Limit)?,
                        u32::try_from(current).map_err(|_| SurfaceError::Limit)?,
                        u32::try_from(b).map_err(|_| SurfaceError::Limit)?,
                    ]);
                    next[a] = u32::try_from(b).map_err(|_| SurfaceError::Limit)?;
                    previous[b] = u32::try_from(a).map_err(|_| SurfaceError::Limit)?;
                    current = b;
                    remaining -= 1;
                    clipped = true;
                    break;
                }
            }
            current = next[current] as usize;
        }
        if !clipped {
            return Ok(false);
        }
    }
    work.charge(1)?;
    let a = current;
    let b = next[a] as usize;
    let c = next[b] as usize;
    if a == b
        || b == c
        || c == a
        || edge_masks[a] & edge_masks[b] & edge_masks[c] != 0
        || turn(vertices[a], vertices[b], vertices[c], axes).signum() != orientation
    {
        return Ok(false);
    }
    triangles.push([
        u32::try_from(a).map_err(|_| SurfaceError::Limit)?,
        u32::try_from(b).map_err(|_| SurfaceError::Limit)?,
        u32::try_from(c).map_err(|_| SurfaceError::Limit)?,
    ]);
    Ok(true)
}

impl AffineFaceMesh {
    /// Rebuild a planar source cell from one or more checked face applications.
    /// A later generation can replace any input and rebuild from the retained
    /// applications. Duplicate polygon edges and inconsistent shared corners
    /// remain explicit refusals; this does not certify unlisted faces or coverage.
    pub fn merge(
        applications: &[Arc<AppliedMixedFace>],
        tolerance_mm: f64,
        limits: AffineFaceMergeLimits,
        cancel: impl Fn() -> bool,
    ) -> Result<Result<Self, AffineFaceMergeRefusal>, SurfaceError> {
        Self::merge_with_budget(
            applications,
            tolerance_mm,
            limits,
            MemoryBudget::volumes(),
            cancel,
        )
    }

    pub(in crate::surface) fn merge_with_budget(
        applications: &[Arc<AppliedMixedFace>],
        tolerance_mm: f64,
        limits: AffineFaceMergeLimits,
        budget: Arc<MemoryBudget>,
        cancel: impl Fn() -> bool,
    ) -> Result<Result<Self, AffineFaceMergeRefusal>, SurfaceError> {
        check(&cancel)?;
        if applications.is_empty()
            || applications.len() > limits.max_faces
            || limits.max_faces > 6
            || limits.max_vertices > MAX_CELL_VERTICES
            || limits.max_operations == 0
            || limits.max_operations > MAX_WORK
            || limits.max_bytes > MAX_OUTPUT
            || !tolerance_mm.is_finite()
            || tolerance_mm < 0.0
        {
            return Err(SurfaceError::Limit);
        }
        let affine = applications[0].input().affine();
        let original = affine.vertices();
        if !(3..=6).contains(&original.len()) {
            return Err(SurfaceError::Geometry);
        }
        let mut work = Work {
            used: 0,
            limit: limits.max_operations,
            cancel: &cancel,
        };
        let mut edges: [Option<usize>; 6] = [None; 6];
        let mut split_counts = [0usize; 6];
        let mut corners: [Option<[u32; 3]>; 6] = [None; 6];
        let mut offsets = [0usize; 7];
        for (application_id, application) in applications.iter().enumerate() {
            work.charge(1)?;
            let input = application.input();
            offsets[application_id + 1] = offsets[application_id]
                .checked_add(input.points().len())
                .ok_or(SurfaceError::Limit)?;
            let other = input.affine();
            if !Arc::ptr_eq(affine.field().source(), other.field().source())
                || affine.field().key() != other.field().key()
                || affine.field().revision() != other.field().revision()
                || affine.cell() != other.cell()
                || original != other.vertices()
            {
                return Err(SurfaceError::Geometry);
            }
            let face = input.face();
            let axis = usize::from(face.axis);
            let projection = usize::from(input.projection_axis());
            if axis >= 3 || projection >= 3 || axis == projection {
                return Err(SurfaceError::Geometry);
            }
            let origin = affine.cell().origin();
            let upper = if face.origin[axis] == origin[axis] {
                false
            } else if origin[axis].checked_add(1) == Some(face.origin[axis]) {
                true
            } else {
                return Err(SurfaceError::Geometry);
            };
            if affine
                .cell()
                .face(axis, upper)
                .ok_or(SurfaceError::Geometry)?
                .0
                != face
            {
                return Err(SurfaceError::Geometry);
            }
            let face_value = i64::from(upper);
            let on_face = |vertex: [Rational; 3]| {
                vertex[axis].numerator() == face_value * vertex[axis].denominator() as i64
            };
            let mut edge = None;
            for i in 0..original.len() {
                work.charge(1)?;
                if on_face(original[i])
                    && on_face(original[(i + 1) % original.len()])
                    && edge.replace(i).is_some()
                {
                    return Err(SurfaceError::Geometry);
                }
            }
            let edge = edge.ok_or(SurfaceError::Geometry)?;
            if edges[edge].replace(application_id).is_some() {
                return Ok(Err(AffineFaceMergeRefusal::DuplicateEdge));
            }
            let along = (0..3)
                .find(|value| *value != axis && *value != projection)
                .ok_or(SurfaceError::Geometry)?;
            for (point_id, point) in input.points().iter().enumerate() {
                work.charge(1)?;
                if point.affine == MixedAffinePlacement::Outside {
                    continue;
                }
                let Some(position) = point.affine_position else {
                    continue;
                };
                let mapping = application
                    .point_vertices()
                    .get(point_id)
                    .ok_or(SurfaceError::Geometry)?;
                if mapping[0].is_none() {
                    return Err(SurfaceError::Geometry);
                }
                let output = application
                    .affine_vertices()
                    .get(mapping[1].ok_or(SurfaceError::Geometry)? as usize)
                    .ok_or(SurfaceError::Geometry)?;
                if *output != position {
                    return Err(SurfaceError::Geometry);
                }
                match point.affine {
                    MixedAffinePlacement::Outside => {}
                    MixedAffinePlacement::Split => {
                        split_counts[edge] = split_counts[edge]
                            .checked_add(1)
                            .ok_or(SurfaceError::Limit)?;
                    }
                    MixedAffinePlacement::Endpoint(_) => {
                        let mut matched = None;
                        for index in [edge, (edge + 1) % original.len()] {
                            let coordinate = ExactCoordinate::from_rational(original[index][along])
                                .map_err(error)?
                                .ok_or(SurfaceError::Geometry)?;
                            if work.compare(coordinate, point.coordinate)? == Ordering::Equal
                                && matched.replace(index).is_some()
                            {
                                return Err(SurfaceError::Geometry);
                            }
                        }
                        let index = matched.ok_or(SurfaceError::Geometry)?;
                        if corners[index].is_some_and(|prior| prior != position) {
                            return Ok(Err(AffineFaceMergeRefusal::ConflictingCorner));
                        }
                        corners[index] = Some(position);
                    }
                }
            }
        }
        let boundary = split_counts[..original.len()]
            .iter()
            .try_fold(original.len(), |sum, count| sum.checked_add(*count))
            .ok_or(SurfaceError::Limit)?;
        let anchor = (0..original.len()).find(|i| {
            split_counts[*i] == 0 && split_counts[(*i + original.len() - 1) % original.len()] == 0
        });
        let needs_ears = anchor.is_none();
        let vertex_count = boundary;
        let triangle_count = boundary.checked_sub(2).ok_or(SurfaceError::Geometry)?;
        if vertex_count > limits.max_vertices || triangle_count > MAX_CELL_TRIANGLES {
            return Err(SurfaceError::Limit);
        }
        let bytes = size(
            &[
                (
                    applications.len(),
                    std::mem::size_of::<Arc<AppliedMixedFace>>(),
                ),
                (vertex_count, std::mem::size_of::<[u32; 3]>()),
                (triangle_count, std::mem::size_of::<[u32; 3]>()),
                (applications.len() + 1, std::mem::size_of::<usize>()),
                (
                    offsets[applications.len()],
                    std::mem::size_of::<Option<u32>>(),
                ),
            ],
            std::mem::size_of::<Self>() + crate::surface::shared_header_bytes::<Self>(),
        )?;
        if bytes > limits.max_bytes {
            return Err(SurfaceError::Limit);
        }
        let scratch = if needs_ears {
            size(
                &[(
                    boundary,
                    std::mem::size_of::<u8>() + 2 * std::mem::size_of::<u32>(),
                )],
                3 * std::mem::size_of::<Vec<u32>>(),
            )?
        } else {
            0
        };
        let reservation = budget.reserve(bytes).ok_or(SurfaceError::Memory)?;
        let _scratch = budget.reserve(scratch).ok_or(SurfaceError::Memory)?;
        let mut edge_masks = exact(if needs_ears { boundary } else { 0 })?;
        let mut retained = exact(applications.len())?;
        let mut vertices = exact(vertex_count)?;
        let mut triangles = exact(triangle_count)?;
        let mut point_offsets = exact(applications.len() + 1)?;
        let mut point_vertices = exact(offsets[applications.len()])?;
        point_vertices.resize(offsets[applications.len()], None);
        point_offsets.extend_from_slice(&offsets[..=applications.len()]);
        let basis = physical_basis(applications[0].input())?;
        let mut corner_ids = [0u32; 6];
        let mut error_mm = 0_f64;
        for i in 0..original.len() {
            work.charge(1)?;
            let mut position = [0; 3];
            for axis in 0..3 {
                position[axis] = rounded(original[i][axis])?;
            }
            if let Some(binding) = corners[i] {
                position = binding;
            }
            for axis in 0..3 {
                let coordinate = original[i][axis];
                if (coordinate.numerator() == 0 && position[axis] != 0)
                    || (coordinate.numerator() == coordinate.denominator() as i64
                        && position[axis] != REFINED_GRID)
                {
                    return Ok(Err(AffineFaceMergeRefusal::Topology));
                }
            }
            let bound = vertex_error(position, original[i], basis)?;
            error_mm = error_mm.max(bound);
            corner_ids[i] = u32::try_from(vertices.len()).map_err(|_| SurfaceError::Limit)?;
            vertices.push(position);
            if needs_ears {
                edge_masks.push((1u8 << i) | (1u8 << ((i + original.len() - 1) % original.len())));
            }
            if let Some(application_id) = edges[i] {
                let input = applications[application_id].input();
                let face_axis = usize::from(input.face().axis);
                let projection = usize::from(input.projection_axis());
                let along = (0..3)
                    .find(|axis| *axis != face_axis && *axis != projection)
                    .ok_or(SurfaceError::Geometry)?;
                let forward = difference(
                    original[(i + 1) % original.len()][along],
                    original[i][along],
                )
                .0 > 0;
                for step in 0..input.points().len() {
                    work.charge(1)?;
                    let id = if forward {
                        step
                    } else {
                        input.points().len() - 1 - step
                    };
                    let point = &input.points()[id];
                    if point.affine != MixedAffinePlacement::Split {
                        continue;
                    }
                    if let Some(position) = point.affine_position {
                        error_mm =
                            error_mm.max(split_error(input, point, position, basis, &mut work)?);
                        point_vertices[offsets[application_id] + id] =
                            Some(u32::try_from(vertices.len()).map_err(|_| SurfaceError::Limit)?);
                        vertices.push(position);
                        if needs_ears {
                            edge_masks.push(1u8 << i);
                        }
                    }
                }
            }
        }
        if vertices.len() != boundary {
            return Err(SurfaceError::Geometry);
        }
        for (application_id, application) in applications.iter().enumerate() {
            let edge = edges[..original.len()]
                .iter()
                .position(|entry| *entry == Some(application_id))
                .ok_or(SurfaceError::Geometry)?;
            let input = application.input();
            let face_axis = usize::from(input.face().axis);
            let projection = usize::from(input.projection_axis());
            let along = (0..3)
                .find(|axis| *axis != face_axis && *axis != projection)
                .ok_or(SurfaceError::Geometry)?;
            for (id, point) in input.points().iter().enumerate() {
                work.charge(1)?;
                if !matches!(point.affine, MixedAffinePlacement::Endpoint(_)) {
                    continue;
                }
                let mut matched = None;
                for index in [edge, (edge + 1) % original.len()] {
                    let coordinate = ExactCoordinate::from_rational(original[index][along])
                        .map_err(error)?
                        .ok_or(SurfaceError::Geometry)?;
                    if work.compare(coordinate, point.coordinate)? == Ordering::Equal
                        && matched.replace(index).is_some()
                    {
                        return Err(SurfaceError::Geometry);
                    }
                }
                point_vertices[offsets[application_id] + id] =
                    Some(corner_ids[matched.ok_or(SurfaceError::Geometry)?]);
            }
        }
        if !error_mm.is_finite() {
            return Err(SurfaceError::Geometry);
        }
        if error_mm > tolerance_mm {
            return Ok(Err(AffineFaceMergeRefusal::PhysicalTolerance));
        }
        let gradient = affine.gradient().ok_or(SurfaceError::Geometry)?;
        let dominant = (0..3)
            .max_by_key(|axis| gradient[*axis].abs())
            .ok_or(SurfaceError::Geometry)?;
        let axes = match dominant {
            0 => [1, 2],
            1 => [0, 2],
            _ => [0, 1],
        };
        let orientation = exact_turn(original[0], original[1], original[2], axes).signum();
        if orientation == 0 {
            return Err(SurfaceError::Geometry);
        }
        for i in 0..boundary {
            for j in i + 1..boundary {
                work.charge(1)?;
                if (i + 1) % boundary == j || (j + 1) % boundary == i {
                    continue;
                }
                if intersects(
                    vertices[i],
                    vertices[(i + 1) % boundary],
                    vertices[j],
                    vertices[(j + 1) % boundary],
                    axes,
                ) {
                    return Ok(Err(AffineFaceMergeRefusal::Topology));
                }
            }
        }
        if let Some(anchor) = anchor {
            let root = corner_ids[anchor] as usize;
            for offset in 1..boundary - 1 {
                work.charge(1)?;
                let a = (root + offset) % boundary;
                let b = (root + offset + 1) % boundary;
                if turn(vertices[root], vertices[a], vertices[b], axes).signum() != orientation {
                    return Ok(Err(AffineFaceMergeRefusal::Topology));
                }
                triangles.push([
                    u32::try_from(root).map_err(|_| SurfaceError::Limit)?,
                    u32::try_from(a).map_err(|_| SurfaceError::Limit)?,
                    u32::try_from(b).map_err(|_| SurfaceError::Limit)?,
                ]);
            }
        } else if !triangulate_ears(
            &vertices,
            &edge_masks,
            axes,
            orientation,
            &mut triangles,
            &mut work,
        )? {
            return Ok(Err(AffineFaceMergeRefusal::Topology));
        }
        if triangles.len() != triangle_count {
            return Err(SurfaceError::Geometry);
        }
        for application in applications {
            work.charge(1)?;
            retained.push(Arc::clone(application));
        }
        check(&cancel)?;
        Ok(Ok(Self {
            affine: affine.clone(),
            applications: retained,
            vertices,
            boundary_vertices: boundary,
            triangles,
            point_offsets,
            point_vertices,
            error_mm,
            bytes,
            _reservation: reservation,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn ear_triangulation_retains_split_edges_without_degenerate_reference_triangles() {
        let original = [
            [0, 0, 0],
            [2, 0, 0],
            [4, 0, 0],
            [4, 2, 0],
            [4, 4, 0],
            [2, 4, 0],
            [0, 4, 0],
            [0, 2, 0],
        ];
        let masks = [9u8, 1, 3, 2, 6, 4, 12, 8];
        for reverse in [false, true] {
            for perturbed in [false, true] {
                let mut vertices = original.to_vec();
                let mut masks = masks.to_vec();
                if perturbed {
                    vertices[1][1] = 1;
                }
                if reverse {
                    vertices.reverse();
                    masks.reverse();
                }
                let orientation = if reverse { -1 } else { 1 };
                let mut triangles = Vec::new();
                let cancel = || false;
                let mut work = Work {
                    used: 0,
                    limit: 10_000,
                    cancel: &cancel,
                };
                assert!(
                    triangulate_ears(
                        &vertices,
                        &masks,
                        [0, 1],
                        orientation,
                        &mut triangles,
                        &mut work
                    )
                    .unwrap()
                );
                assert_eq!(triangles.len(), vertices.len() - 2);
                let mut edges = BTreeMap::<(u32, u32), usize>::new();
                for triangle in triangles {
                    let [a, b, c] = triangle.map(|v| v as usize);
                    assert_eq!(masks[a] & masks[b] & masks[c], 0);
                    assert_eq!(
                        turn(vertices[a], vertices[b], vertices[c], [0, 1]).signum(),
                        orientation
                    );
                    for [u, v] in [[a, b], [b, c], [c, a]] {
                        *edges.entry((u.min(v) as u32, u.max(v) as u32)).or_default() += 1;
                    }
                }
                for i in 0..vertices.len() {
                    let j = (i + 1) % vertices.len();
                    assert_eq!(edges.remove(&(i.min(j) as u32, i.max(j) as u32)), Some(1));
                }
                assert!(edges.values().all(|count| *count == 2));
            }
        }
    }

    #[test]
    fn ear_triangulation_covers_all_small_edge_split_patterns() {
        let corners = [[0u32, 0, 0], [16, 0, 0], [16, 16, 0], [0, 16, 0]];
        for pattern in 0usize..256 {
            let counts = std::array::from_fn::<_, 4, _>(|edge| (pattern >> (2 * edge)) & 3);
            let mut vertices = Vec::new();
            let mut masks = Vec::new();
            for edge in 0..4 {
                let a = corners[edge];
                let b = corners[(edge + 1) % 4];
                vertices.push(a);
                masks.push((1u8 << edge) | (1u8 << ((edge + 3) % 4)));
                let denominator = u32::try_from(counts[edge] + 1).unwrap();
                for split in 1..=counts[edge] {
                    let split = u32::try_from(split).unwrap();
                    vertices.push(std::array::from_fn(|axis| {
                        (a[axis] * (denominator - split) + b[axis] * split) / denominator
                    }));
                    masks.push(1u8 << edge);
                }
            }
            for reverse in [false, true] {
                let mut vertices = vertices.clone();
                let mut masks = masks.clone();
                if reverse {
                    vertices.reverse();
                    masks.reverse();
                }
                let orientation = if reverse { -1 } else { 1 };
                let cancel = || false;
                let mut work = Work {
                    used: 0,
                    limit: 100_000,
                    cancel: &cancel,
                };
                let mut triangles = Vec::new();
                assert!(
                    triangulate_ears(
                        &vertices,
                        &masks,
                        [0, 1],
                        orientation,
                        &mut triangles,
                        &mut work
                    )
                    .unwrap(),
                    "pattern {pattern}, reversed {reverse}"
                );
                assert_eq!(triangles.len(), vertices.len() - 2);
                let area: i128 = triangles
                    .iter()
                    .map(|triangle| {
                        let [a, b, c] = triangle.map(|v| v as usize);
                        assert_eq!(masks[a] & masks[b] & masks[c], 0);
                        turn(vertices[a], vertices[b], vertices[c], [0, 1])
                    })
                    .sum();
                assert_eq!(area, 512 * orientation);
            }
        }
    }
}
