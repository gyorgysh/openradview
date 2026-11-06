// SPDX-License-Identifier: GPL-3.0-or-later
//! Exact split requests between an affine polygon and a general candidate face.
use super::*;
use crate::surface::field::{
    AffineCell, AffineDisposition, AffineFaceTrace, MixedAffinePlacement, MixedFacePartition,
    MixedFacePoint, MixedFaceSpan, MixedFaceUnsupported,
};

impl ConformingSurfaceMesh {
    /// Compare one affine-cell face with the represented general-candidate edges
    /// on its neighbor. Returned positions are requests, not applied edits or a
    /// coverage certificate; unsupported strata remain explicit.
    pub fn mixed_face_partition(
        self: &Arc<Self>,
        affine: &AffineCell,
        face_axis: usize,
        limits: FacePartitionLimits,
        cancel: impl Fn() -> bool,
    ) -> Result<Result<MixedFacePartition, MixedFaceUnsupported>, SurfaceError> {
        self.mixed_face_partition_with_budget(
            affine,
            face_axis,
            limits,
            MemoryBudget::volumes(),
            cancel,
        )
    }
    pub(in crate::surface) fn mixed_face_partition_with_budget(
        self: &Arc<Self>,
        affine: &AffineCell,
        face_axis: usize,
        limits: FacePartitionLimits,
        budget: Arc<MemoryBudget>,
        cancel: impl Fn() -> bool,
    ) -> Result<Result<MixedFacePartition, MixedFaceUnsupported>, SurfaceError> {
        check(&cancel)?;
        if face_axis >= 3
            || limits.max_edges_per_cell > MAX_EDGES
            || limits.max_points > MAX_POINTS
            || limits.max_operations == 0
            || limits.max_operations > MAX_WORK
            || limits.max_bytes > MAX_BYTES
        {
            return Err(SurfaceError::Limit);
        }
        let chart = chart(self);
        let field = chart.graph().field();
        if !Arc::ptr_eq(field.source(), affine.field().source())
            || field.key() != affine.field().key()
            || field.revision() != affine.field().revision()
        {
            return Err(SurfaceError::Geometry);
        }
        let candidate_origin = chart.graph().cell().origin();
        let affine_origin = affine.cell().origin();
        if (0..3).any(|axis| axis != face_axis && candidate_origin[axis] != affine_origin[axis]) {
            return Err(SurfaceError::Geometry);
        }
        let (candidate_upper, affine_upper) =
            if candidate_origin[face_axis].checked_add(1) == Some(affine_origin[face_axis]) {
                (true, false)
            } else if affine_origin[face_axis].checked_add(1) == Some(candidate_origin[face_axis]) {
                (false, true)
            } else {
                return Err(SurfaceError::Geometry);
            };
        if affine.disposition() != AffineDisposition::Plane {
            return Ok(Err(MixedFaceUnsupported::AffineCellUnavailable));
        }
        if usize::from(chart.axis()) == face_axis {
            return Ok(Err(MixedFaceUnsupported::ProjectionEndFace));
        }
        let candidate_face = chart
            .graph()
            .cell()
            .face(face_axis, candidate_upper)
            .ok_or(SurfaceError::Geometry)?
            .0;
        let affine_face = affine
            .cell()
            .face(face_axis, affine_upper)
            .ok_or(SurfaceError::Geometry)?
            .0;
        if candidate_face != affine_face {
            return Err(SurfaceError::Geometry);
        }
        let AffineFaceTrace::Segment(trace) =
            affine.face_trace(face_axis, affine_upper).map_err(error)?
        else {
            return Ok(Err(MixedFaceUnsupported::NoSegment));
        };
        let axes = match face_axis {
            0 => [1, 2],
            1 => [0, 2],
            _ => [0, 1],
        };
        let along_axis = *chart
            .transverse_axes()
            .iter()
            .find(|axis| **axis != face_axis)
            .ok_or(SurfaceError::Geometry)?;
        let slot = axes
            .iter()
            .position(|axis| *axis == along_axis)
            .ok_or(SurfaceError::Geometry)?;
        let mut endpoints = [trace[0][slot], trace[1][slot]];
        if endpoints[0] == endpoints[1] {
            return Ok(Err(MixedFaceUnsupported::CollapsedProjection));
        }
        if i128::from(endpoints[0].numerator()) * i128::from(endpoints[1].denominator())
            > i128::from(endpoints[1].numerator()) * i128::from(endpoints[0].denominator())
        {
            endpoints.swap(0, 1);
        }
        let exact_endpoints = endpoints.map(|point| {
            ExactCoordinate::from_rational(point)
                .map_err(error)
                .and_then(|point| point.ok_or(SurfaceError::Geometry))
        });
        let [lower, upper] = exact_endpoints;
        let lower = lower?;
        let upper = upper?;
        let bytes = size(
            &[
                (limits.max_points, std::mem::size_of::<MixedFacePoint>()),
                (
                    limits.max_points.saturating_sub(1),
                    std::mem::size_of::<MixedFaceSpan>(),
                ),
            ],
            std::mem::size_of::<MixedFacePartition>()
                + crate::surface::shared_header_bytes::<MixedFacePartition>(),
        )?;
        if bytes > limits.max_bytes {
            return Err(SurfaceError::Limit);
        }
        let scratch = size(
            &[
                (limits.max_edges_per_cell, std::mem::size_of::<FaceEdge>()),
                (limits.max_points, std::mem::size_of::<FaceKeyPoint>()),
            ],
            0,
        )?;
        let reservation = budget.reserve(bytes).ok_or(SurfaceError::Memory)?;
        let _scratch = budget.reserve(scratch).ok_or(SurfaceError::Memory)?;
        let mut edges = exact(limits.max_edges_per_cell)?;
        let mut keys = exact(limits.max_points)?;
        let mut points = exact(limits.max_points)?;
        let mut spans = exact(limits.max_points.saturating_sub(1))?;
        let mut work = Work {
            used: 0,
            limit: limits.max_operations,
            cancel: &cancel,
        };
        collect_edges(
            self,
            face_axis,
            candidate_upper,
            &mut edges,
            &mut keys,
            &mut work,
        )?;
        for (endpoint, rational) in [(lower, endpoints[0]), (upper, endpoints[1])] {
            let mut existing = None;
            for key in &keys {
                if work.compare(key.coordinate, endpoint)? == Ordering::Equal {
                    existing = Some(key.representative);
                    break;
                }
            }
            let representative = if let Some(value) = existing {
                value
            } else {
                let numerator =
                    u64::try_from(rational.numerator()).map_err(|_| SurfaceError::Geometry)?;
                let denominator = rational.denominator();
                let scaled = numerator
                    .checked_mul(u64::from(REFINED_GRID))
                    .ok_or(SurfaceError::Limit)?;
                let value = scaled
                    .checked_mul(2)
                    .and_then(|v| v.checked_add(denominator))
                    .map(|v| v / (2 * denominator))
                    .ok_or(SurfaceError::Limit)?;
                u32::try_from(value).map_err(|_| SurfaceError::Limit)?
            };
            insert_key(
                &mut keys,
                FaceKeyPoint {
                    coordinate: endpoint,
                    representative,
                },
                &mut work,
            )?;
        }
        for key in keys.iter().copied() {
            work.charge(1)?;
            let lo = work.compare(key.coordinate, lower)?;
            let hi = work.compare(key.coordinate, upper)?;
            let affine_placement = if lo == Ordering::Less || hi == Ordering::Greater {
                MixedAffinePlacement::Outside
            } else if lo == Ordering::Equal {
                MixedAffinePlacement::Endpoint(0)
            } else if hi == Ordering::Equal {
                MixedAffinePlacement::Endpoint(1)
            } else {
                MixedAffinePlacement::Split
            };
            let candidate = placement(&edges, key, chart, face_axis, candidate_upper, &mut work)?;
            let affine_position = if candidate == FacePlacement::Absent {
                None
            } else {
                let mut position = location(self, candidate).ok_or(SurfaceError::Geometry)?;
                position[face_axis] = if affine_upper { REFINED_GRID } else { 0 };
                Some(position)
            };
            points.push(MixedFacePoint {
                coordinate: key.coordinate,
                affine: affine_placement,
                candidate,
                affine_position,
            });
        }
        for (index, pair) in keys.windows(2).enumerate() {
            work.charge(1)?;
            let first = u32::try_from(index).map_err(|_| SurfaceError::Limit)?;
            let within = work.compare(pair[0].coordinate, lower)? != Ordering::Less
                && work.compare(pair[1].coordinate, upper)? != Ordering::Greater;
            let candidate =
                span_coverage(&edges, pair[0].coordinate, pair[1].coordinate, &mut work)?;
            let candidate_within_tolerance = match candidate {
                FaceSpanCoverage::Absent => false,
                FaceSpanCoverage::Present { triangle, .. } => matches!(
                    self.triangles
                        .get(triangle as usize)
                        .ok_or(SurfaceError::Geometry)?
                        .state,
                    ConformingFacetState::WithinTolerance { .. }
                ),
            };
            spans.push(MixedFaceSpan {
                points: [first, first.checked_add(1).ok_or(SurfaceError::Limit)?],
                affine: within,
                candidate,
                candidate_within_tolerance,
            });
        }
        check(&cancel)?;
        Ok(Ok(MixedFacePartition {
            affine: affine.clone(),
            candidate: Arc::clone(self),
            face: candidate_face,
            projection_axis: chart.axis(),
            points,
            spans,
            bytes,
            _reservation: reservation,
        }))
    }
}
