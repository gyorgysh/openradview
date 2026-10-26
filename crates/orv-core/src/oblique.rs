// SPDX-License-Identifier: GPL-3.0-or-later
//! Explicit physical image axes; no anatomical name is inferred for an oblique plane.
use crate::{spatial::GeometryError, study::Frame, vector::*, volume::Volume};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ObliquePlane {
    pub center_mm: [f64; 3],
    /// Unit LPS direction of increasing column and row indices respectively.
    pub column_direction: [f64; 3],
    pub row_direction: [f64; 3],
}
impl ObliquePlane {
    pub fn normal_direction(&self) -> [f64; 3] {
        cross(self.column_direction, self.row_direction)
    }
    pub fn validate(&self) -> Result<(), GeometryError> {
        let (u, v) = (self.column_direction, self.row_direction);
        if !finite(self.center_mm) || self.center_mm.iter().any(|v| v.abs() > 10_000_000.0) {
            return Err(GeometryError::Missing);
        }
        if !finite(u)
            || !finite(v)
            || (norm(u) - 1.0).abs() > 1e-6
            || (norm(v) - 1.0).abs() > 1e-6
            || dot(u, v).abs() > 1e-6
        {
            return Err(GeometryError::Orientation);
        }
        Ok(())
    }
    /// Extrinsic rotations about LPS X, then Y, then Z. Zero gives axial +X/+Y.
    pub fn from_angles(center_mm: [f64; 3], degrees: [f64; 3]) -> Result<Self, GeometryError> {
        Self {
            center_mm,
            column_direction: [1., 0., 0.],
            row_direction: [0., 1., 0.],
        }
        .rotated(degrees)
    }
    /// Rotate directions about the explicit plane pivot, retaining exact values at zero.
    pub fn rotated(&self, degrees: [f64; 3]) -> Result<Self, GeometryError> {
        self.validate()?;
        if degrees.iter().any(|a| !a.is_finite() || a.abs() > 360.0) {
            return Err(GeometryError::Orientation);
        }
        if degrees == [0.; 3] {
            return Ok(self.clone());
        }
        let angles = degrees.map(f64::to_radians);
        let rotate = |mut p: V3| {
            for (axis, angle) in angles.iter().enumerate() {
                let basis = std::array::from_fn(|i| if axis == i { 1.0 } else { 0.0 });
                p = add(
                    add(mul(p, angle.cos()), mul(cross(basis, p), angle.sin())),
                    mul(basis, dot(basis, p) * (1.0 - angle.cos())),
                );
            }
            p
        };
        let plane = Self {
            center_mm: self.center_mm,
            column_direction: rotate(self.column_direction),
            row_direction: rotate(self.row_direction),
        };
        plane.validate()?;
        Ok(plane)
    }
}

impl Volume {
    pub fn reformat_oblique(
        &self,
        plane: &ObliquePlane,
        max_side: u32,
        cancel: impl Fn() -> bool,
    ) -> Result<Arc<Frame>, GeometryError> {
        plane.validate()?;
        let (u, v) = (plane.column_direction, plane.row_direction);
        let mut minimum = [f64::INFINITY; 2];
        let mut maximum = [f64::NEG_INFINITY; 2];
        // Project the actual affine corners, preserving regular shear.
        for corner in self.corners() {
            let d = sub(corner, plane.center_mm);
            for (i, value) in [dot(d, u), dot(d, v)].into_iter().enumerate() {
                minimum[i] = minimum[i].min(value);
                maximum[i] = maximum[i].max(value);
            }
        }
        let position = add(plane.center_mm, add(mul(u, minimum[0]), mul(v, minimum[1])));
        self.resample(
            position,
            u,
            v,
            std::array::from_fn(|i| maximum[i] - minimum[i]),
            max_side,
            cancel,
        )
    }
}
