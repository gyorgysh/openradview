// SPDX-License-Identifier: GPL-3.0-or-later
//! DICOM PS3.3 C.7.6.2.1: LPS millimetres and zero-based pixel centres.
use crate::{study::Geometry, vector::*};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GeometryError {
    Missing,
    Reference,
    Orientation,
    Spacing,
    Dimensions,
    Irregular,
    TooFew,
    Pixels,
    Limit,
    Cancelled,
}
impl std::fmt::Display for GeometryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Missing => {
                "MPR needs complete position, orientation and spacing in a known millimetre coordinate system."
            }
            Self::Reference => {
                "MPR needs a single, known frame of reference for the entire series."
            }
            Self::Orientation => "The image directions are invalid or differ between slices.",
            Self::Spacing => {
                "Pixel spacing must be finite, positive and consistent between slices."
            }
            Self::Dimensions => "The image dimensions do not form a consistent volume.",
            Self::Irregular => "Slices overlap, have gaps or do not form a regular spatial volume.",
            Self::TooFew => "MPR needs at least two spatially distinct images.",
            Self::Pixels => "MPR and 3D need valid samples and one consistent scalar or RGB representation.",
            Self::Limit => "The requested volume or reformat exceeds the supported size limit.",
            Self::Cancelled => "View update cancelled.",
        })
    }
}
impl std::error::Error for GeometryError {}

impl Geometry {
    pub fn validate(&self) -> Result<(), GeometryError> {
        if !finite(self.position) {
            return Err(GeometryError::Missing);
        }
        let (u, v) = self.directions();
        if !finite(u)
            || !finite(v)
            || (norm(u) - 1.0).abs() > 1e-4
            || (norm(v) - 1.0).abs() > 1e-4
            || dot(u, v).abs() > 1e-4
        {
            return Err(GeometryError::Orientation);
        }
        if self.spacing.iter().any(|s| !s.is_finite() || *s <= 0.0) {
            return Err(GeometryError::Spacing);
        }
        Ok(())
    }
    pub fn directions(&self) -> (V3, V3) {
        let o = self.orientation;
        ([o[0], o[1], o[2]], [o[3], o[4], o[5]])
    }
    /// Coordinates refer to pixel centres: (0,0) is Image Position Patient.
    pub fn pixel_to_world(&self, pixel: [f64; 2]) -> Result<V3, GeometryError> {
        self.validate()?;
        if pixel.iter().any(|v| !v.is_finite()) {
            return Err(GeometryError::Pixels);
        }
        let (u, v) = self.directions();
        let result = add(
            self.position,
            add(
                mul(u, pixel[0] * self.spacing[1]),
                mul(v, pixel[1] * self.spacing[0]),
            ),
        );
        if !finite(result) {
            return Err(GeometryError::Pixels);
        }
        Ok(result)
    }
    pub fn world_to_pixel(&self, world: V3) -> Result<[f64; 2], GeometryError> {
        self.validate()?;
        if !finite(world) {
            return Err(GeometryError::Pixels);
        }
        let delta = sub(world, self.position);
        if dot(delta, self.normal()).abs() > 0.01 {
            return Err(GeometryError::Missing);
        }
        let (u, v) = self.directions();
        Ok([
            dot(delta, u) / self.spacing[1],
            dot(delta, v) / self.spacing[0],
        ])
    }
}
