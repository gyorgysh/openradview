// SPDX-License-Identifier: GPL-3.0-or-later
//! Physical orthographic rays and monochrome transfer functions. No rendering APIs.
use crate::{ViewState, WindowLevel, vector::*, volume::Volume};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum Projection {
    Maximum,
    Composite,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum Quality {
    Interactive,
    Refined,
}
impl Quality {
    pub fn step_factor(self) -> f64 {
        match self {
            Self::Interactive => 2.0,
            Self::Refined => 0.5,
        }
    }
    pub fn max_steps(self) -> u32 {
        match self {
            Self::Interactive => 256,
            Self::Refined => 4096,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum TransferShape {
    Ramp,
    Band,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct TransferFunction {
    pub low: f32,
    pub high: f32,
    /// Extinction per physical millimetre, not per sample.
    pub opacity_per_mm: f32,
    pub shape: TransferShape,
}
impl TransferFunction {
    pub fn validate(&self) -> Result<(), &'static str> {
        if !self.low.is_finite()
            || !self.high.is_finite()
            || self.low.abs() > 1e9
            || self.high.abs() > 1e9
            || self.high - self.low < 1.0
            || !self.opacity_per_mm.is_finite()
            || !(0.0..=10.0).contains(&self.opacity_per_mm)
        {
            return Err("Invalid volume transfer function");
        }
        Ok(())
    }
    pub fn weight(&self, value: f32) -> f32 {
        let t = (value - self.low) / (self.high - self.low);
        match self.shape {
            TransferShape::Ramp => t.clamp(0.0, 1.0),
            TransferShape::Band => (1.0 - (2.0 * t - 1.0).abs()).clamp(0.0, 1.0),
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum TransferPreset {
    SoftTissue,
    Bone,
    Lung,
}
impl TransferPreset {
    pub fn values(self) -> (WindowLevel, TransferFunction) {
        let (window, low, high, opacity_per_mm, shape) = match self {
            Self::SoftTissue => (
                WindowLevel::SOFT_TISSUE,
                -150.,
                250.,
                0.025,
                TransferShape::Ramp,
            ),
            Self::Bone => (WindowLevel::BONE, 150., 1200., 0.08, TransferShape::Ramp),
            Self::Lung => (WindowLevel::LUNG, -950., -350., 0.025, TransferShape::Band),
        };
        (
            window,
            TransferFunction {
                low,
                high,
                opacity_per_mm,
                shape,
            },
        )
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct Camera {
    pub target_mm: [f64; 3],
    /// Yaw about LPS Z; pitch about local right; roll about the viewing direction.
    pub yaw_degrees: f64,
    pub pitch_degrees: f64,
    pub roll_degrees: f64,
    pub vertical_span_mm: f64,
}
pub struct RayFrame {
    pub origin_mm: [f64; 3],
    pub right_mm: [f64; 3],
    pub down_mm: [f64; 3],
    pub direction_unit: [f64; 3],
}
impl Camera {
    pub fn fit(volume: &Volume) -> Self {
        let [min, max] = volume.bounds();
        Self {
            target_mm: volume.center(),
            yaw_degrees: 0.,
            pitch_degrees: 0.,
            roll_degrees: 0.,
            vertical_span_mm: norm(sub(max, min)) * 1.05,
        }
    }
    pub fn validate(&self) -> Result<(), &'static str> {
        if !finite(self.target_mm)
            || self.target_mm.iter().any(|v| v.abs() > 1e7)
            || [self.yaw_degrees, self.pitch_degrees, self.roll_degrees]
                .iter()
                .any(|a| !a.is_finite() || a.abs() > 360.0)
            || !self.vertical_span_mm.is_finite()
            || !(0.0001..=1e9).contains(&self.vertical_span_mm)
        {
            return Err("Invalid physical volume camera");
        }
        Ok(())
    }
    /// Fit projected affine corners at the current orbit, preserving orientation.
    pub fn fit_to_volume(&mut self, volume: &Volume, aspect: f64) -> Result<(), &'static str> {
        if !aspect.is_finite() || aspect <= 0.0 {
            return Err("Invalid volume viewport aspect");
        }
        self.target_mm = volume.center();
        let frame = self.frame(&ViewState::default(), 1.0)?;
        let right = mul(frame.right_mm, 1.0 / self.vertical_span_mm);
        let down = mul(frame.down_mm, 1.0 / self.vertical_span_mm);
        let mut radius = 0.0f64;
        for corner in volume.corners() {
            let offset = sub(corner, self.target_mm);
            radius = radius
                .max(dot(offset, right).abs() / aspect)
                .max(dot(offset, down).abs());
        }
        self.vertical_span_mm = radius * 2.1;
        self.validate()
    }
    pub fn frame(&self, view: &ViewState, aspect: f64) -> Result<RayFrame, &'static str> {
        self.validate()?;
        view.validate()?;
        if !aspect.is_finite() || aspect <= 0.0 || aspect > 1024.0 {
            return Err("Invalid volume viewport aspect");
        }
        let (yaw, pitch, roll) = (
            self.yaw_degrees.to_radians(),
            self.pitch_degrees.to_radians(),
            self.roll_degrees.to_radians(),
        );
        let right = [yaw.cos(), yaw.sin(), 0.];
        let down = add(
            mul([0., 0., -1.], pitch.cos()),
            mul(cross(right, [0., 0., -1.]), pitch.sin()),
        );
        let direction_unit = cross(right, down);
        let right_rotated = add(mul(right, roll.cos()), mul(down, roll.sin()));
        let down_rotated = add(mul(down, roll.cos()), mul(right, -roll.sin()));
        let span = self.vertical_span_mm / f64::from(view.zoom);
        let right_mm = mul(right_rotated, span * aspect);
        let down_mm = mul(down_rotated, span);
        let origin_mm = sub(
            self.target_mm,
            add(
                mul(right_mm, f64::from(view.pan[0])),
                mul(down_mm, f64::from(view.pan[1])),
            ),
        );
        Ok(RayFrame {
            origin_mm,
            right_mm,
            down_mm,
            direction_unit,
        })
    }
}
impl RayFrame {
    /// Ray parameter is physical mm; bounds are voxel centres, matching MPR.
    pub fn ray(&self, uv: [f64; 2]) -> ([f64; 3], [f64; 3]) {
        (
            add(
                self.origin_mm,
                add(
                    mul(self.right_mm, uv[0] - 0.5),
                    mul(self.down_mm, uv[1] - 0.5),
                ),
            ),
            self.direction_unit,
        )
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct VolumeView {
    pub schema_version: u32,
    pub camera: Camera,
    pub projection: Projection,
    pub transfer: TransferFunction,
    pub quality: Quality,
}
impl VolumeView {
    pub fn fit(volume: &Volume) -> Self {
        Self {
            schema_version: 1,
            camera: Camera::fit(volume),
            projection: Projection::Maximum,
            transfer: if volume.is_color() {
                TransferFunction {
                    low: 0.,
                    high: 255.,
                    opacity_per_mm: 0.05,
                    shape: TransferShape::Ramp,
                }
            } else {
                TransferPreset::Bone.values().1
            },
            quality: Quality::Refined,
        }
    }
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.schema_version != 1 {
            return Err("Unsupported volume view schema");
        }
        self.camera.validate()?;
        self.transfer.validate()
    }
}

impl Volume {
    /// Clip an infinite physical ray against the complete affine source domain.
    pub fn ray_interval(&self, origin_mm: V3, direction_unit: V3) -> Option<[f64; 2]> {
        if !finite(origin_mm) || !finite(direction_unit) || (norm(direction_unit) - 1.).abs() > 1e-6
        {
            return None;
        }
        let origin = self.world_to_voxel(origin_mm);
        let direction = self.world_vector_to_voxel(direction_unit);
        let mut near = f64::NEG_INFINITY;
        let mut far = f64::INFINITY;
        for axis in 0..3 {
            let maximum = (self.dimensions()[axis] - 1) as f64;
            if direction[axis].abs() < 1e-12 {
                if origin[axis] < 0. || origin[axis] > maximum {
                    return None;
                }
            } else {
                let a = -origin[axis] / direction[axis];
                let b = (maximum - origin[axis]) / direction[axis];
                near = near.max(a.min(b));
                far = far.min(a.max(b));
            }
        }
        (near.is_finite() && far.is_finite() && near <= far).then_some([near, far])
    }
}
