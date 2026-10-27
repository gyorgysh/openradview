// SPDX-License-Identifier: GPL-3.0-or-later
//! Bounded discrete extrema of trilinear source samples along a physical normal.
use crate::{oblique::ObliquePlane, spatial::GeometryError, volume::Volume};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum SlabProjection {
    Maximum,
    Minimum,
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct Slab {
    pub projection: SlabProjection,
    pub thickness_mm: f64,
    /// Even interval count includes both boundaries and the central plane.
    pub intervals: u16,
}
impl Slab {
    pub fn validate(&self) -> Result<(), &'static str> {
        if !self.thickness_mm.is_finite()
            || !(0.001..=2000.).contains(&self.thickness_mm)
            || !(2..=256).contains(&self.intervals)
            || !self.intervals.is_multiple_of(2)
        {
            return Err("Slabs need 0.001–2000 mm thickness and 2–256 even sampling intervals");
        }
        Ok(())
    }
    pub fn for_volume(volume: &Volume, thickness_mm: f64, projection: SlabProjection) -> Self {
        // Bound displacement to half a voxel on every axis where capacity permits.
        // The explicit interval count records coarser sampling at the work limit.
        let inverse: [[f64; 3]; 3] = std::array::from_fn(|axis| {
            volume.world_vector_to_voxel(std::array::from_fn(|i| if i == axis { 1. } else { 0. }))
        });
        let speed = (0..3)
            .map(|row| {
                inverse
                    .iter()
                    .map(|column| column[row].powi(2))
                    .sum::<f64>()
                    .sqrt()
            })
            .fold(0., f64::max);
        let intervals = ((thickness_mm * speed * 2.).ceil() as u16).clamp(2, 256);
        Self {
            projection,
            thickness_mm,
            intervals: (intervals + intervals % 2).min(256),
        }
    }
    pub fn preview(self) -> Self {
        Self {
            intervals: self.intervals.min(16),
            ..self
        }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SlabPlane {
    pub plane: ObliquePlane,
    pub slab: Slab,
}
impl SlabPlane {
    pub fn validate(&self) -> Result<(), &'static str> {
        self.plane
            .validate()
            .map_err(|_| "Invalid slab sampling plane")?;
        self.slab.validate()
    }
}
impl Volume {
    pub fn reformat_slab(
        &self,
        plane: &ObliquePlane,
        slab: Slab,
        max_side: u32,
        cancel: impl Fn() -> bool,
    ) -> Result<std::sync::Arc<crate::study::Frame>, GeometryError> {
        slab.validate().map_err(|_| GeometryError::Limit)?;
        if self.is_color() {
            return Err(GeometryError::Dimensions);
        }
        self.resample_oblique(plane, (max_side, Some(slab)), cancel)
    }
}
