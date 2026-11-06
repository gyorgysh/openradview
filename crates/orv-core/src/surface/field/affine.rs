// SPDX-License-Identifier: GPL-3.0-or-later
//! Exact local plane intersections for affine source-cell fields.
use super::{
    CellPhase, ConformingSurfaceMesh, ExactCoordinate, FaceKey, FacePlacement, FaceSpanCoverage,
    FieldCell, FieldError, MaskField, Rational,
};
use crate::resources::Reservation;
use std::sync::Arc;

/// A cell-local result. Boundary faces still require adjacent-cell coordination,
/// unknown/grid policy and a separate physical rendering/error contract.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AffineDisposition {
    Unavailable {
        invalid_corners: u8,
        unknown_corners: u8,
    },
    /// Includes zero-only boundary strata; these do not imply grid closure.
    NoInteriorCrossing(CellPhase),
    NonAffine,
    /// The closed-cell intersection is exactly the retained convex polygon.
    Plane,
}

/// Exact trace on the two ascending axes of a shared source-cell face.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AffineFaceTrace {
    Empty,
    Point([Rational; 2]),
    Segment([[Rational; 2]; 2]),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MixedFaceUnsupported {
    /// The listed face does not have exactly one affine crossing cell.
    NotMixed,
    /// The other listed cell did not produce a general candidate mesh.
    CandidateUnavailable,
    AffineCellUnavailable,
    ProjectionEndFace,
    NoSegment,
    CollapsedProjection,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MixedAffinePlacement {
    Outside,
    /// Endpoint 0/1 in increasing candidate-chart coordinate order.
    Endpoint(u8),
    Split,
}
#[derive(Clone, Copy, Debug)]
pub struct MixedFacePoint {
    pub coordinate: ExactCoordinate,
    pub affine: MixedAffinePlacement,
    pub candidate: FacePlacement,
    /// Candidate-grid position translated into the affine cell, if available.
    /// It is a proposed split and has not been checked against the affine
    /// physical error target or applied to the polygon.
    pub affine_position: Option<[u32; 3]>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MixedFaceSpan {
    pub points: [u32; 2],
    pub affine: bool,
    pub candidate: FaceSpanCoverage,
    /// False for absent or uncertified candidate facets.
    pub candidate_within_tolerance: bool,
}
/// A source-bound directory of split requests, not a completed or watertight seam.
pub struct MixedFacePartition {
    pub(in crate::surface) affine: AffineCell,
    pub(in crate::surface) candidate: Arc<ConformingSurfaceMesh>,
    pub(in crate::surface) face: FaceKey,
    pub(in crate::surface) projection_axis: u8,
    pub(in crate::surface) points: Vec<MixedFacePoint>,
    pub(in crate::surface) spans: Vec<MixedFaceSpan>,
    pub(in crate::surface) bytes: u64,
    pub(in crate::surface) _reservation: Reservation,
}
impl MixedFacePartition {
    pub fn affine(&self) -> &AffineCell {
        &self.affine
    }
    pub fn candidate(&self) -> &Arc<ConformingSurfaceMesh> {
        &self.candidate
    }
    pub fn face(&self) -> FaceKey {
        self.face
    }
    pub fn projection_axis(&self) -> u8 {
        self.projection_axis
    }
    pub fn points(&self) -> &[MixedFacePoint] {
        &self.points
    }
    pub fn spans(&self) -> &[MixedFaceSpan] {
        &self.spans
    }
    pub fn referenced_bytes(&self) -> u64 {
        self.bytes
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AffineFaceState {
    Matched(AffineFaceTrace),
    /// One or both neighboring cells do not supply an affine crossing plane.
    Unresolved {
        negative: AffineDisposition,
        positive: AffineDisposition,
    },
}
/// A matched local face is not proof of a complete surface or grid closure.
pub struct AffineFace {
    field: MaskField,
    key: FaceKey,
    state: AffineFaceState,
}
impl AffineFace {
    pub fn field(&self) -> &MaskField {
        &self.field
    }
    pub fn key(&self) -> FaceKey {
        self.key
    }
    pub fn state(&self) -> AffineFaceState {
        self.state
    }
}

/// Exact source/revision-bound local geometry, with no edit authority or global
/// surface-coverage claim. Coordinates are ratios relative to the source cell.
#[derive(Clone)]
pub struct AffineCell {
    field: MaskField,
    cell: FieldCell,
    disposition: AffineDisposition,
    gradient: [i16; 3],
    vertices: [[Rational; 3]; 6],
    count: u8,
}
impl AffineCell {
    pub fn field(&self) -> &MaskField {
        &self.field
    }
    pub fn cell(&self) -> &FieldCell {
        &self.cell
    }
    pub fn disposition(&self) -> AffineDisposition {
        self.disposition
    }
    /// Increasing-field gradient in source lattice coordinates.
    pub fn gradient(&self) -> Option<[i16; 3]> {
        (self.disposition == AffineDisposition::Plane).then_some(self.gradient)
    }
    /// Counterclockwise from the outward normal in physical source orientation.
    /// Vertices are exact local source-cell coordinates, not GPU positions.
    pub fn vertices(&self) -> &[[Rational; 3]] {
        &self.vertices[..usize::from(self.count)]
    }
    pub fn triangles(&self) -> impl Iterator<Item = [u8; 3]> + '_ {
        (1..self.count.saturating_sub(1)).map(|i| [0, i, i + 1])
    }
    pub(in crate::surface) fn face_trace(
        &self,
        axis: usize,
        upper: bool,
    ) -> Result<AffineFaceTrace, FieldError> {
        if axis >= 3 || self.disposition != AffineDisposition::Plane {
            return Err(FieldError::Bounds);
        }
        face_trace(self, axis, upper)
    }
    /// Match the exact traces of two adjacent cells from one field revision.
    /// Unknown neighbors and non-affine or boundary-only cells stay unresolved.
    pub fn shared_face(
        &self,
        other: &Self,
        axis: usize,
        cancel: impl Fn() -> bool,
    ) -> Result<AffineFace, FieldError> {
        if cancel() {
            return Err(FieldError::Cancelled);
        }
        if axis >= 3 {
            return Err(FieldError::Bounds);
        }
        if !Arc::ptr_eq(self.field.source(), other.field.source())
            || self.field.key() != other.field.key()
            || self.field.revision() != other.field.revision()
        {
            return Err(FieldError::Selection);
        }
        let a = self.cell.origin();
        let b = other.cell.origin();
        if (0..3).any(|i| i != axis && a[i] != b[i]) {
            return Err(FieldError::Bounds);
        }
        let (negative, positive) = if a[axis].checked_add(1) == Some(b[axis]) {
            (self, other)
        } else if b[axis].checked_add(1) == Some(a[axis]) {
            (other, self)
        } else {
            return Err(FieldError::Bounds);
        };
        let key = FaceKey {
            origin: positive.cell.origin(),
            axis: axis as u8,
        };
        let state = if negative.disposition == AffineDisposition::Plane
            && positive.disposition == AffineDisposition::Plane
        {
            let first = face_trace(negative, axis, true)?;
            let second = face_trace(positive, axis, false)?;
            if first != second {
                return Err(FieldError::Arithmetic);
            }
            AffineFaceState::Matched(first)
        } else {
            AffineFaceState::Unresolved {
                negative: negative.disposition,
                positive: positive.disposition,
            }
        };
        if cancel() {
            return Err(FieldError::Cancelled);
        }
        Ok(AffineFace {
            field: self.field.clone(),
            key,
            state,
        })
    }
    /// Outward world-space unit normal for F>=0, applying the inverse transpose.
    pub fn physical_normal(&self) -> Option<[f64; 3]> {
        if self.disposition != AffineDisposition::Plane {
            return None;
        }
        let gradient: [f64; 3] = std::array::from_fn(|a| {
            let mut axis = [0.; 3];
            axis[a] = 1.;
            -self
                .field
                .source()
                .world_vector_to_voxel(axis)
                .into_iter()
                .zip(self.gradient)
                .map(|(a, b)| a * f64::from(b))
                .sum::<f64>()
        });
        crate::volume_material::gradient_normal(gradient)
    }
}

fn face_trace(cell: &AffineCell, axis: usize, upper: bool) -> Result<AffineFaceTrace, FieldError> {
    let axes = match axis {
        0 => [1, 2],
        1 => [0, 2],
        _ => [0, 1],
    };
    let zero = Rational::new(0, 1)?;
    let mut points = [[zero; 2]; 2];
    let mut count = 0usize;
    for vertex in cell.vertices() {
        if vertex[axis].numerator()
            == if upper {
                vertex[axis].denominator() as i64
            } else {
                0
            }
        {
            let point = [vertex[axes[0]], vertex[axes[1]]];
            if !points[..count].contains(&point) {
                *points.get_mut(count).ok_or(FieldError::Arithmetic)? = point;
                count += 1;
            }
        }
    }
    if count == 2
        && cmp(points[0][0], points[1][0])
            .then_with(|| cmp(points[0][1], points[1][1]))
            .is_gt()
    {
        points.swap(0, 1);
    }
    Ok(match count {
        0 => AffineFaceTrace::Empty,
        1 => AffineFaceTrace::Point(points[0]),
        2 => AffineFaceTrace::Segment(points),
        _ => return Err(FieldError::Arithmetic),
    })
}

impl MaskField {
    /// Worker-only exact intersection for affine trilinear fields. All source
    /// values and validity must be known. Other fields remain explicitly unresolved.
    pub fn affine_cell(
        &self,
        origin: [usize; 3],
        cancel: impl Fn() -> bool,
    ) -> Result<AffineCell, FieldError> {
        if cancel() {
            return Err(FieldError::Cancelled);
        }
        let cell = self.cell(origin)?;
        let zero = Rational::new(0, 1)?;
        let mut result = AffineCell {
            field: self.clone(),
            cell,
            disposition: AffineDisposition::NonAffine,
            gradient: [0; 3],
            vertices: [[zero; 3]; 6],
            count: 0,
        };
        let mut invalid_corners = 0u8;
        let mut unknown_corners = 0u8;
        for (i, sample) in cell.corners().iter().enumerate() {
            invalid_corners |= u8::from(!sample.source_valid) << i;
            unknown_corners |= u8::from(sample.value.is_none()) << i;
        }
        if invalid_corners != 0 || unknown_corners != 0 {
            result.disposition = AffineDisposition::Unavailable {
                invalid_corners,
                unknown_corners,
            };
            if cancel() {
                return Err(FieldError::Cancelled);
            }
            return Ok(result);
        }
        let interpolant = cell.interpolant().ok_or(FieldError::Value)?;
        let phase = interpolant.phase();
        if phase != CellPhase::Crossing {
            result.disposition = AffineDisposition::NoInteriorCrossing(phase);
            if cancel() {
                return Err(FieldError::Cancelled);
            }
            return Ok(result);
        }
        let values = interpolant.values();
        let base = i32::from(values[0]);
        let gradient = [
            i32::from(values[1]) - base,
            i32::from(values[2]) - base,
            i32::from(values[4]) - base,
        ];
        for (index, value) in values.iter().enumerate() {
            let expected = base
                + (0..3)
                    .map(|axis| gradient[axis] * ((index >> axis) & 1) as i32)
                    .sum::<i32>();
            if expected != i32::from(*value) {
                if cancel() {
                    return Err(FieldError::Cancelled);
                }
                return Ok(result);
            }
        }
        result.gradient = gradient.map(|v| v as i16);
        let (vertices, count) = polygon(values, gradient, &cancel)?;
        result.vertices = vertices;
        result.count = count;
        // The exact hull is oriented in lattice coordinates. A reflected
        // source affine reverses physical triangle winding.
        let inverse: [[f64; 3]; 3] = std::array::from_fn(|a| {
            let mut axis = [0.; 3];
            axis[a] = 1.;
            self.source().world_vector_to_voxel(axis)
        });
        let determinant = inverse[0][0]
            * (inverse[1][1] * inverse[2][2] - inverse[1][2] * inverse[2][1])
            - inverse[0][1] * (inverse[1][0] * inverse[2][2] - inverse[1][2] * inverse[2][0])
            + inverse[0][2] * (inverse[1][0] * inverse[2][1] - inverse[1][1] * inverse[2][0]);
        if !determinant.is_finite() || determinant == 0. {
            return Err(FieldError::Arithmetic);
        }
        if determinant < 0. {
            result.vertices[..usize::from(count)].reverse();
        }
        result.disposition = AffineDisposition::Plane;
        if result.physical_normal().is_none() {
            return Err(FieldError::Arithmetic);
        }
        if cancel() {
            return Err(FieldError::Cancelled);
        }
        Ok(result)
    }
}

fn difference(a: Rational, b: Rational) -> (i128, i128) {
    (
        i128::from(a.numerator()) * i128::from(b.denominator())
            - i128::from(b.numerator()) * i128::from(a.denominator()),
        i128::from(a.denominator()) * i128::from(b.denominator()),
    )
}
fn cmp(a: Rational, b: Rational) -> std::cmp::Ordering {
    difference(a, b).0.cmp(&0)
}
// Ratios originate on cube edges of a field in [-255,255], so each denominator
// is <=510. The largest cross-multiplied value is below 2^80, within i128.
fn turn(a: [Rational; 3], b: [Rational; 3], c: [Rational; 3], axes: [usize; 2]) -> i128 {
    let (x, xd) = difference(b[axes[0]], a[axes[0]]);
    let (y, yd) = difference(b[axes[1]], a[axes[1]]);
    let (u, ud) = difference(c[axes[0]], a[axes[0]]);
    let (v, vd) = difference(c[axes[1]], a[axes[1]]);
    x * v * yd * ud - y * u * xd * vd
}
fn polygon(
    values: [i16; 8],
    gradient: [i32; 3],
    cancel: &impl Fn() -> bool,
) -> Result<([[Rational; 3]; 6], u8), FieldError> {
    let zero = Rational::new(0, 1)?;
    let one = Rational::new(1, 1)?;
    let mut points = [[zero; 3]; 12];
    let mut count = 0usize;
    let mut push = |point: [Rational; 3]| -> Result<(), FieldError> {
        if !points[..count].contains(&point) {
            *points.get_mut(count).ok_or(FieldError::Capacity)? = point;
            count += 1;
        }
        Ok(())
    };
    for axis in 0..3 {
        for corner in 0usize..8 {
            if cancel() {
                return Err(FieldError::Cancelled);
            }
            if corner & (1 << axis) != 0 {
                continue;
            }
            let other = corner | (1 << axis);
            let a = values[corner];
            let b = values[other];
            let mut point =
                std::array::from_fn(|i| if corner & (1 << i) == 0 { zero } else { one });
            if a == 0 {
                push(point)?;
            }
            if b == 0 {
                point[axis] = one;
                push(point)?;
            }
            if a.signum() * b.signum() == -1 {
                point[axis] = Rational::new(i128::from(a), i128::from(a) - i128::from(b))?;
                push(point)?;
            }
        }
    }
    let projection = (0..3)
        .max_by_key(|a| gradient[*a].abs())
        .ok_or(FieldError::Arithmetic)?;
    if gradient[projection] == 0 {
        return Err(FieldError::Arithmetic);
    }
    let axes = match projection {
        0 => [1, 2],
        1 => [0, 2],
        _ => [0, 1],
    };
    // Insertion sort is bounded by the twelve cube edges and uses exact ratios.
    for i in 1..count {
        let mut j = i;
        while j > 0 {
            let order = cmp(points[j - 1][axes[0]], points[j][axes[0]])
                .then_with(|| cmp(points[j - 1][axes[1]], points[j][axes[1]]));
            if !order.is_gt() {
                break;
            }
            points.swap(j - 1, j);
            j -= 1;
        }
    }
    let mut hull = [[zero; 3]; 24];
    let mut size = 0usize;
    for p in points[..count].iter().copied() {
        while size >= 2 && turn(hull[size - 2], hull[size - 1], p, axes) <= 0 {
            size -= 1;
        }
        hull[size] = p;
        size += 1;
    }
    let lower = size;
    for p in points[..count].iter().rev().skip(1).copied() {
        while size > lower && turn(hull[size - 2], hull[size - 1], p, axes) <= 0 {
            size -= 1;
        }
        hull[size] = p;
        size += 1;
    }
    if size == 0 {
        return Err(FieldError::Arithmetic);
    }
    size -= 1;
    if !(3..=6).contains(&size) {
        return Err(FieldError::Arithmetic);
    }
    let parity = if projection == 1 { -1 } else { 1 };
    if parity * gradient[projection].signum() > 0 {
        hull[..size].reverse();
    }
    let mut result = [[zero; 3]; 6];
    result[..size].copy_from_slice(&hull[..size]);
    Ok((result, size as u8))
}
#[cfg(test)]
mod tests;
