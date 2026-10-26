// SPDX-License-Identifier: GPL-3.0-or-later
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// DICOM LINEAR windowing (PS3.3 C.11.2); values are in modality units.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct WindowLevel {
    pub width: f32,
    pub center: f32,
}

impl WindowLevel {
    pub const SOFT_TISSUE: Self = Self {
        width: 400.0,
        center: 40.0,
    };
    pub const LUNG: Self = Self {
        width: 1500.0,
        center: -600.0,
    };
    pub const BONE: Self = Self {
        width: 2000.0,
        center: 300.0,
    };
    pub const BRAIN: Self = Self {
        width: 80.0,
        center: 40.0,
    };

    pub fn is_valid(self) -> bool {
        self.width.is_finite() && self.width >= 1.0 && self.center.is_finite()
    }

    pub fn normalize(self, value: f32) -> f32 {
        if !self.is_valid() || !value.is_finite() {
            return 0.0;
        }
        if self.width == 1.0 {
            return if value > self.center - 0.5 { 1.0 } else { 0.0 };
        }
        ((value - (self.center - 0.5)) / (self.width - 1.0) + 0.5).clamp(0.0, 1.0)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum Plane {
    #[default]
    Acquired,
    Axial,
    Coronal,
    Sagittal,
}

/// Reproducible display settings. Contains no patient metadata or file paths.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ViewState {
    pub schema_version: u32,
    pub plane: Plane,
    pub slice: u32,
    pub window: WindowLevel,
    pub zoom: f32,
    /// Pan in fractions of viewport width/height.
    pub pan: [f32; 2],
    pub invert: bool,
}

impl Default for ViewState {
    fn default() -> Self {
        Self {
            schema_version: 1,
            plane: Plane::Acquired,
            slice: 0,
            window: WindowLevel::SOFT_TISSUE,
            zoom: 1.0,
            pan: [0.0, 0.0],
            invert: false,
        }
    }
}

impl ViewState {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.schema_version != 1 {
            return Err("Unsupported view schema");
        }
        if !self.window.is_valid() {
            return Err("Window width must be finite and at least one");
        }
        if !self.zoom.is_finite() || !(0.05..=50.0).contains(&self.zoom) {
            return Err("Zoom is outside the supported range");
        }
        if self.pan.iter().any(|v| !v.is_finite() || v.abs() > 10.0) {
            return Err("Pan is outside the supported range");
        }
        Ok(())
    }

    pub fn hash(&self) -> Result<String, &'static str> {
        self.validate()?;
        let bytes = serde_json::to_vec(self).map_err(|_| "Could not serialize view")?;
        Ok(Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dicom_window_boundaries_and_threshold() {
        let w = WindowLevel {
            width: 400.0,
            center: 40.0,
        };
        assert_eq!(w.normalize(-160.0), 0.0);
        assert_eq!(w.normalize(239.0), 1.0);
        assert_eq!(w.normalize(39.5), 0.5);
        let w = WindowLevel {
            width: 1.0,
            center: 0.0,
        };
        assert_eq!(w.normalize(-0.5), 0.0);
        assert_eq!(w.normalize(-0.49), 1.0);
    }

    #[test]
    fn view_hash_roundtrips_and_changes_with_display() {
        let mut v = ViewState::default();
        let json = serde_json::to_string(&v).unwrap();
        let restored: ViewState = serde_json::from_str(&json).unwrap();
        assert_eq!(v.hash(), restored.hash());
        let original = v.hash().unwrap();
        v.slice = 1;
        assert_ne!(original, v.hash().unwrap());
        v.window.center = f32::NAN;
        assert!(v.hash().is_err());
    }
}
