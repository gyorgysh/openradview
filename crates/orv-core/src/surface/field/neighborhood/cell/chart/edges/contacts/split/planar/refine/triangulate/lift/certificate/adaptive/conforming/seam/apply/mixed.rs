// SPDX-License-Identifier: GPL-3.0-or-later
//! Atomic mixed-face application with bounded physical and topology checks.
use super::*;
use crate::surface::field::{
    AppliedMixedFace, AppliedMixedSpan, MixedAffinePlacement, MixedFaceApplyRefusal,
    MixedFacePartition, Rational,
};

fn ratio_upper(num: u128, den: u128) -> Result<f64, SurfaceError> {
    if den == 0 {
        return Err(SurfaceError::Geometry);
    }
    if num == 0 {
        return Ok(0.0);
    }
    let result = ((num as f64).next_up() / (den as f64).next_down()).next_up();
    if !result.is_finite() {
        return Err(SurfaceError::Geometry);
    }
    Ok(result)
}

fn physical_basis(partition: &MixedFacePartition) -> Result<[f64; 3], SurfaceError> {
    let mut basis = [0.0; 3];
    for (axis, bound) in basis.iter_mut().enumerate() {
        let mut unit = [0.0; 3];
        unit[axis] = 1.0;
        for value in partition
            .affine()
            .field()
            .source()
            .voxel_vector_to_world(unit)
        {
            *bound = (*bound + value.abs()).next_up();
        }
        if !bound.is_finite() {
            return Err(SurfaceError::Geometry);
        }
    }
    Ok(basis)
}

fn add_error(total: &mut f64, error: f64, basis: f64) {
    if error != 0.0 {
        *total = (*total + (error * basis).next_up()).next_up();
    }
}

fn vertex_error(
    position: [u32; 3],
    original: [Rational; 3],
    basis: [f64; 3],
) -> Result<f64, SurfaceError> {
    let mut error = 0.0;
    for axis in 0..3 {
        let ratio = original[axis];
        let denominator = i128::from(ratio.denominator());
        let difference = i128::from(position[axis]) * denominator
            - i128::from(ratio.numerator()) * i128::from(REFINED_GRID);
        let divisor = u128::from(ratio.denominator()) * u128::from(REFINED_GRID);
        add_error(
            &mut error,
            ratio_upper(difference.unsigned_abs(), divisor)?,
            basis[axis],
        );
    }
    Ok(error)
}

fn split_error<F: Fn() -> bool>(
    partition: &MixedFacePartition,
    point: &crate::surface::field::MixedFacePoint,
    position: [u32; 3],
    basis: [f64; 3],
    work: &mut Work<'_, F>,
) -> Result<f64, SurfaceError> {
    let face = usize::from(partition.face().axis);
    let projection = usize::from(partition.projection_axis());
    if face >= 3 || projection >= 3 || face == projection {
        return Err(SurfaceError::Geometry);
    }
    let along = (0..3)
        .find(|axis| *axis != face && *axis != projection)
        .ok_or(SurfaceError::Geometry)?;
    let upper = match position[face] {
        0 => 0_i128,
        REFINED_GRID => 1_i128,
        _ => return Err(SurfaceError::Geometry),
    };
    if position.iter().any(|coordinate| *coordinate > REFINED_GRID) {
        return Err(SurfaceError::Geometry);
    }
    // The affine face's projected coordinate is linear in the exact transverse
    // coordinate, so its largest residual over an interval is at an endpoint.
    let (lower, upper_bound, denominator) = if let Some(ratio) = point.coordinate.rational_value() {
        let value = i128::from(ratio.numerator());
        (value, value, i128::from(ratio.denominator()))
    } else {
        work.charge(25)?;
        let bounds = coordinate_bounds(point.coordinate, 24, work.cancel)?;
        (
            i128::from(bounds.lower),
            i128::from(bounds.upper),
            i128::from(bounds.denominator),
        )
    };
    if denominator <= 0 || lower < 0 || upper_bound > denominator || lower > upper_bound {
        return Err(SurfaceError::Geometry);
    }
    let grid = i128::from(REFINED_GRID);
    let along_error = [lower, upper_bound]
        .map(|v| (i128::from(position[along]) * denominator - v * grid).unsigned_abs())
        .into_iter()
        .max()
        .ok_or(SurfaceError::Geometry)?;
    let gradient = partition
        .affine()
        .gradient()
        .ok_or(SurfaceError::Geometry)?
        .map(i128::from);
    if gradient[projection] == 0 {
        return Err(SurfaceError::Geometry);
    }
    let base = i128::from(
        partition.affine().cell().corners()[0]
            .value
            .ok_or(SurfaceError::Geometry)?,
    );
    let constant = base + gradient[face] * upper;
    let projection_error = [lower, upper_bound]
        .map(|v| {
            (i128::from(position[projection]) * gradient[projection] * denominator
                + grid * (constant * denominator + gradient[along] * v))
                .unsigned_abs()
        })
        .into_iter()
        .max()
        .ok_or(SurfaceError::Geometry)?;
    let den = u128::try_from(grid * denominator).map_err(|_| SurfaceError::Geometry)?;
    let mut error = 0.0;
    add_error(&mut error, ratio_upper(along_error, den)?, basis[along]);
    add_error(
        &mut error,
        ratio_upper(projection_error, den * gradient[projection].unsigned_abs())?,
        basis[projection],
    );
    Ok(error)
}

fn rounded(value: Rational) -> Result<u32, SurfaceError> {
    let numerator = u64::try_from(value.numerator()).map_err(|_| SurfaceError::Geometry)?;
    let denominator = value.denominator();
    if denominator == 0 || numerator > denominator {
        return Err(SurfaceError::Geometry);
    }
    let scaled = numerator
        .checked_mul(u64::from(REFINED_GRID))
        .ok_or(SurfaceError::Limit)?;
    let value = scaled
        .checked_mul(2)
        .and_then(|v| v.checked_add(denominator))
        .map(|v| v / (2 * denominator))
        .ok_or(SurfaceError::Limit)?;
    u32::try_from(value).map_err(|_| SurfaceError::Limit)
}

fn difference(a: Rational, b: Rational) -> (i128, i128) {
    (
        i128::from(a.numerator()) * i128::from(b.denominator())
            - i128::from(b.numerator()) * i128::from(a.denominator()),
        i128::from(a.denominator()) * i128::from(b.denominator()),
    )
}
// Original cube-edge ratios have denominators at most 510, so these exact
// cross-products fit in i128.
fn exact_turn(a: [Rational; 3], b: [Rational; 3], c: [Rational; 3], axes: [usize; 2]) -> i128 {
    let (x, xd) = difference(b[axes[0]], a[axes[0]]);
    let (y, yd) = difference(b[axes[1]], a[axes[1]]);
    let (u, ud) = difference(c[axes[0]], a[axes[0]]);
    let (v, vd) = difference(c[axes[1]], a[axes[1]]);
    x * v * yd * ud - y * u * xd * vd
}
fn turn(a: [u32; 3], b: [u32; 3], c: [u32; 3], axes: [usize; 2]) -> i128 {
    let x = i128::from(b[axes[0]]) - i128::from(a[axes[0]]);
    let y = i128::from(b[axes[1]]) - i128::from(a[axes[1]]);
    let u = i128::from(c[axes[0]]) - i128::from(a[axes[0]]);
    let v = i128::from(c[axes[1]]) - i128::from(a[axes[1]]);
    x * v - y * u
}
fn intersects(a: [u32; 3], b: [u32; 3], c: [u32; 3], d: [u32; 3], axes: [usize; 2]) -> bool {
    let (ab_c, ab_d, cd_a, cd_b) = (
        turn(a, b, c, axes),
        turn(a, b, d, axes),
        turn(c, d, a, axes),
        turn(c, d, b, axes),
    );
    axes.iter().all(|axis| {
        a[*axis].min(b[*axis]) <= c[*axis].max(d[*axis])
            && c[*axis].min(d[*axis]) <= a[*axis].max(b[*axis])
    }) && ab_c.signum() * ab_d.signum() <= 0
        && cd_a.signum() * cd_b.signum() <= 0
}

impl MixedFacePartition {
    /// Apply one mixed boundary privately. Both outputs are returned together only
    /// after candidate recertification and affine physical/topology checks.
    /// Other faces, missing spans and complete surface coverage remain unresolved.
    pub fn apply(
        self: &Arc<Self>,
        affine_tolerance_mm: f64,
        limits: AppliedFaceLimits,
        cancel: impl Fn() -> bool,
    ) -> Result<Result<AppliedMixedFace, MixedFaceApplyRefusal>, SurfaceError> {
        self.apply_mixed_with_budget(affine_tolerance_mm, limits, MemoryBudget::volumes(), cancel)
    }

    pub(in crate::surface) fn apply_mixed_with_budget(
        self: &Arc<Self>,
        affine_tolerance_mm: f64,
        limits: AppliedFaceLimits,
        budget: Arc<MemoryBudget>,
        cancel: impl Fn() -> bool,
    ) -> Result<Result<AppliedMixedFace, MixedFaceApplyRefusal>, SurfaceError> {
        check(&cancel)?;
        if !affine_tolerance_mm.is_finite()
            || affine_tolerance_mm < 0.0
            || limits.tolerance_bits.iter().any(|bits| *bits > 16)
            || limits.max_vertices_per_cell > MAX_CELL_VERTICES
            || limits.max_triangles_per_cell > MAX_CELL_TRIANGLES
            || limits.max_operations == 0
            || limits.max_operations > MAX_WORK
            || limits.max_bytes > MAX_OUTPUT
        {
            return Err(SurfaceError::Limit);
        }
        let mut work = Work {
            used: 0,
            limit: limits.max_operations,
            cancel: &cancel,
        };
        let candidate = self.candidate();
        let original = self.affine().vertices();
        if !(3..=6).contains(&original.len()) {
            return Err(SurfaceError::Geometry);
        }
        let mut candidate_splits = 0usize;
        let mut affine_splits = 0usize;
        for point in self.points() {
            work.charge(1)?;
            candidate_splits += usize::from(matches!(point.candidate, FacePlacement::Split { .. }));
            affine_splits += usize::from(
                point.affine == MixedAffinePlacement::Split && point.affine_position.is_some(),
            );
        }
        let cv = candidate
            .vertices
            .len()
            .checked_add(candidate_splits)
            .ok_or(SurfaceError::Limit)?;
        let ct = candidate
            .triangles
            .len()
            .checked_add(candidate_splits)
            .ok_or(SurfaceError::Limit)?;
        let av = original
            .len()
            .checked_add(affine_splits)
            .ok_or(SurfaceError::Limit)?;
        let at = av.checked_sub(2).ok_or(SurfaceError::Geometry)?;
        if cv > limits.max_vertices_per_cell
            || av > limits.max_vertices_per_cell
            || ct > limits.max_triangles_per_cell
            || at > limits.max_triangles_per_cell
        {
            return Err(SurfaceError::Limit);
        }
        let bytes = size(
            &[
                (cv, std::mem::size_of::<AppliedFaceVertex>()),
                (ct, std::mem::size_of::<AppliedFaceTriangle>()),
                (av, std::mem::size_of::<[u32; 3]>()),
                (at, std::mem::size_of::<[u32; 3]>()),
                (self.points().len(), std::mem::size_of::<[Option<u32>; 2]>()),
                (self.spans().len(), std::mem::size_of::<AppliedMixedSpan>()),
            ],
            std::mem::size_of::<AppliedMixedFace>()
                + crate::surface::shared_header_bytes::<AppliedMixedFace>(),
        )?;
        if bytes > limits.max_bytes {
            return Err(SurfaceError::Limit);
        }
        let scratch = size(
            &[(candidate_splits, std::mem::size_of::<Request>())],
            std::mem::size_of::<Vec<Request>>(),
        )?;
        let reservation = budget.reserve(bytes).ok_or(SurfaceError::Memory)?;
        let _scratch = budget.reserve(scratch).ok_or(SurfaceError::Memory)?;
        let mut candidate_vertices = exact(cv)?;
        let mut requests = exact(candidate_splits)?;
        let mut point_vertices = exact(self.points().len())?;
        let mut affine_vertices = exact(av)?;
        let mut affine_triangles = exact(at)?;
        let mut spans = exact(self.spans().len())?;
        for (id, vertex) in candidate.vertices.iter().enumerate() {
            work.charge(1)?;
            candidate_vertices.push(AppliedFaceVertex::Retained {
                id: u32::try_from(id).map_err(|_| SurfaceError::Limit)?,
                position: vertex.position(),
            });
        }
        for (id, point) in self.points().iter().enumerate() {
            work.charge(1)?;
            let point_id = u32::try_from(id).map_err(|_| SurfaceError::Limit)?;
            let mapped = match point.candidate {
                FacePlacement::Absent => None,
                FacePlacement::Existing { vertex } => {
                    candidate
                        .vertices
                        .get(vertex as usize)
                        .and_then(|v| v.position())
                        .ok_or(SurfaceError::Geometry)?;
                    Some(vertex)
                }
                FacePlacement::Split {
                    triangle,
                    edge,
                    position,
                } => {
                    let vertex =
                        u32::try_from(candidate_vertices.len()).map_err(|_| SurfaceError::Limit)?;
                    candidate_vertices.push(AppliedFaceVertex::Split {
                        point: point_id,
                        edge,
                        position,
                    });
                    requests.push(Request {
                        triangle,
                        edge,
                        point: point_id,
                        vertex,
                    });
                    Some(vertex)
                }
            };
            point_vertices.push([mapped, None]);
        }
        let criterion = match candidate.criterion {
            Criterion::PhysicalMm(mm) => Criterion::PhysicalMm(mm),
            Criterion::AxialBits(_) => Criterion::AxialBits(limits.tolerance_bits[0]),
        };
        let candidate_triangles = build_triangles(
            candidate,
            &candidate_vertices,
            &mut requests,
            ct,
            criterion,
            &mut work,
        )?;
        let candidate_output = AppliedFaceCell {
            vertices: candidate_vertices,
            triangles: candidate_triangles,
        };
        let face_axis = usize::from(self.face().axis);
        let projection = usize::from(self.projection_axis());
        if face_axis >= 3 || projection >= 3 || face_axis == projection {
            return Err(SurfaceError::Geometry);
        }
        let along = (0..3)
            .find(|axis| *axis != face_axis && *axis != projection)
            .ok_or(SurfaceError::Geometry)?;
        let affine_origin = self.affine().cell().origin();
        let face_origin = self.face().origin;
        let face_upper = if face_origin[face_axis] == affine_origin[face_axis] {
            false
        } else if affine_origin[face_axis].checked_add(1) == Some(face_origin[face_axis]) {
            true
        } else {
            return Err(SurfaceError::Geometry);
        };
        let face_value = i64::from(face_upper);
        let on_face = |vertex: [Rational; 3]| {
            vertex[face_axis].numerator() == face_value * vertex[face_axis].denominator() as i64
        };
        let anchor = original
            .iter()
            .position(|vertex| !on_face(*vertex))
            .ok_or(SurfaceError::Geometry)?;
        let basis = physical_basis(self)?;
        let mut error_mm = 0_f64;
        let mut endpoint_ids = [None; 2];
        for (index, point) in self.points().iter().enumerate() {
            work.charge(1)?;
            if let MixedAffinePlacement::Endpoint(endpoint) = point.affine {
                let slot = endpoint_ids
                    .get_mut(usize::from(endpoint))
                    .ok_or(SurfaceError::Geometry)?;
                if slot.replace(index).is_some() {
                    return Err(SurfaceError::Geometry);
                }
            }
        }
        let endpoint_ids = [
            endpoint_ids[0].ok_or(SurfaceError::Geometry)?,
            endpoint_ids[1].ok_or(SurfaceError::Geometry)?,
        ];
        let mut seen_edge = false;
        for offset in 0..original.len() {
            work.charge(1)?;
            let index = (anchor + offset) % original.len();
            let next = (index + 1) % original.len();
            let vertex = original[index];
            let mut position = [0; 3];
            for axis in 0..3 {
                position[axis] = rounded(vertex[axis])?;
            }
            if on_face(vertex) {
                let coordinate = ExactCoordinate::from_rational(vertex[along])
                    .map_err(error)?
                    .ok_or(SurfaceError::Geometry)?;
                let mut matched = None;
                for endpoint in endpoint_ids {
                    if work.compare(coordinate, self.points()[endpoint].coordinate)?
                        == Ordering::Equal
                    {
                        matched = Some(endpoint);
                    }
                }
                let endpoint = matched.ok_or(SurfaceError::Geometry)?;
                if let Some(candidate_position) = self.points()[endpoint].affine_position {
                    position = candidate_position;
                }
                point_vertices[endpoint][1] =
                    Some(u32::try_from(affine_vertices.len()).map_err(|_| SurfaceError::Limit)?);
            }
            // A shared polygon vertex can also lie on another cell face. A
            // tolerance allowance must not move it off that exact boundary.
            for axis in 0..3 {
                let coordinate = vertex[axis];
                if (coordinate.numerator() == 0 && position[axis] != 0)
                    || (coordinate.numerator() == coordinate.denominator() as i64
                        && position[axis] != REFINED_GRID)
                {
                    return Ok(Err(MixedFaceApplyRefusal::Topology));
                }
            }
            error_mm = error_mm.max(vertex_error(position, vertex, basis)?);
            affine_vertices.push(position);
            if on_face(vertex) && on_face(original[next]) {
                if seen_edge {
                    return Err(SurfaceError::Geometry);
                }
                seen_edge = true;
                let forward = difference(original[next][along], vertex[along]).0 > 0;
                for step in 0..self.points().len() {
                    work.charge(1)?;
                    let id = if forward {
                        step
                    } else {
                        self.points().len() - 1 - step
                    };
                    let point = &self.points()[id];
                    if point.affine != MixedAffinePlacement::Split {
                        continue;
                    }
                    if let Some(position) = point.affine_position {
                        let expected_face = if face_upper { REFINED_GRID } else { 0 };
                        if position[face_axis] != expected_face {
                            return Err(SurfaceError::Geometry);
                        }
                        let gradient = self.affine().gradient().ok_or(SurfaceError::Geometry)?;
                        let base = self.affine().cell().corners()[0]
                            .value
                            .ok_or(SurfaceError::Geometry)?;
                        let constant = i32::from(base)
                            + i32::from(gradient[face_axis]) * i32::from(face_upper);
                        if gradient[along] == 0
                            && ((constant == 0 && position[projection] != 0)
                                || (constant + i32::from(gradient[projection]) == 0
                                    && position[projection] != REFINED_GRID))
                        {
                            return Ok(Err(MixedFaceApplyRefusal::Topology));
                        }
                        error_mm =
                            error_mm.max(split_error(self, point, position, basis, &mut work)?);
                        point_vertices[id][1] = Some(
                            u32::try_from(affine_vertices.len())
                                .map_err(|_| SurfaceError::Limit)?,
                        );
                        affine_vertices.push(position);
                    }
                }
            }
        }
        if !seen_edge
            || affine_vertices.len() != av
            || endpoint_ids
                .iter()
                .any(|id| point_vertices[*id][1].is_none())
        {
            return Err(SurfaceError::Geometry);
        }
        if !error_mm.is_finite() {
            return Err(SurfaceError::Geometry);
        }
        if error_mm > affine_tolerance_mm {
            return Ok(Err(MixedFaceApplyRefusal::PhysicalTolerance));
        }
        let gradient = self.affine().gradient().ok_or(SurfaceError::Geometry)?;
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
        for i in 1..affine_vertices.len() - 1 {
            work.charge(1)?;
            if turn(
                affine_vertices[0],
                affine_vertices[i],
                affine_vertices[i + 1],
                axes,
            )
            .signum()
                != orientation
            {
                return Ok(Err(MixedFaceApplyRefusal::Topology));
            }
            affine_triangles.push([
                0,
                u32::try_from(i).map_err(|_| SurfaceError::Limit)?,
                u32::try_from(i + 1).map_err(|_| SurfaceError::Limit)?,
            ]);
        }
        for i in 0..affine_vertices.len() {
            for j in i + 1..affine_vertices.len() {
                work.charge(1)?;
                if (i + 1) % affine_vertices.len() == j || (j + 1) % affine_vertices.len() == i {
                    continue;
                }
                if intersects(
                    affine_vertices[i],
                    affine_vertices[(i + 1) % affine_vertices.len()],
                    affine_vertices[j],
                    affine_vertices[(j + 1) % affine_vertices.len()],
                    axes,
                ) {
                    return Ok(Err(MixedFaceApplyRefusal::Topology));
                }
            }
        }
        for (id, span) in self.spans().iter().enumerate() {
            work.charge(1)?;
            let a = *point_vertices
                .get(span.points[0] as usize)
                .ok_or(SurfaceError::Geometry)?;
            let b = *point_vertices
                .get(span.points[1] as usize)
                .ok_or(SurfaceError::Geometry)?;
            let mut segments = [None; 2];
            let mut certified = false;
            if let FaceSpanCoverage::Present { triangle, .. } = span.candidate {
                let edge = [
                    a[0].ok_or(SurfaceError::Geometry)?,
                    b[0].ok_or(SurfaceError::Geometry)?,
                ];
                let mut matched = None;
                for output in candidate_output.triangles() {
                    work.charge(1)?;
                    if output.source_triangle == triangle
                        && output.vertices.contains(&edge[0])
                        && output.vertices.contains(&edge[1])
                        && matched.replace(output.state).is_some()
                    {
                        return Err(SurfaceError::Geometry);
                    }
                }
                certified = matches!(
                    matched.ok_or(SurfaceError::Geometry)?,
                    AppliedTriangleState::WithinTolerance { .. }
                );
                segments[0] = Some(edge);
            }
            if span.affine {
                segments[1] = Some([
                    a[1].ok_or(SurfaceError::Geometry)?,
                    b[1].ok_or(SurfaceError::Geometry)?,
                ]);
            }
            spans.push(AppliedMixedSpan {
                partition_span: u32::try_from(id).map_err(|_| SurfaceError::Limit)?,
                segments,
                candidate_within_tolerance: certified,
            });
        }
        check(&cancel)?;
        Ok(Ok(AppliedMixedFace {
            input: Arc::clone(self),
            candidate: candidate_output,
            affine_vertices,
            affine_triangles,
            point_vertices,
            spans,
            affine_error_mm: error_mm,
            bytes,
            _reservation: reservation,
        }))
    }
}
