// SPDX-License-Identifier: GPL-3.0-or-later
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum VoiFunction {
    #[default]
    Linear,
    LinearExact,
    Sigmoid,
    Lookup {
        index: u8,
    },
}
impl VoiFunction {
    fn is_linear(&self) -> bool {
        *self == Self::Linear
    }
}

/// DICOM grayscale display mapping (PS3.3 C.11.2), after source rescale.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct WindowLevel {
    pub width: f32,
    pub center: f32,
    #[serde(default, skip_serializing_if = "VoiFunction::is_linear")]
    pub function: VoiFunction,
}

impl WindowLevel {
    pub const SOFT_TISSUE: Self = Self {
        function: VoiFunction::Linear,
        width: 400.0,
        center: 40.0,
    };
    pub const LUNG: Self = Self {
        function: VoiFunction::Linear,
        width: 1500.0,
        center: -600.0,
    };
    pub const BONE: Self = Self {
        function: VoiFunction::Linear,
        width: 2000.0,
        center: 300.0,
    };
    pub const BRAIN: Self = Self {
        function: VoiFunction::Linear,
        width: 80.0,
        center: 40.0,
    };

    pub fn is_valid(self) -> bool {
        self.width.is_finite()
            && self.center.is_finite()
            && match self.function {
                VoiFunction::Linear => self.width >= 1.,
                VoiFunction::LinearExact | VoiFunction::Sigmoid => self.width > 0.,
                VoiFunction::Lookup { index } => {
                    index < 16 && self.width == 1. && self.center == 0.
                }
            }
    }

    /// Analytic functions only. A lookup requires its source table and returns NaN here.
    pub fn normalize(self, value: f32) -> f32 {
        if !self.is_valid() || !value.is_finite() {
            return 0.0;
        }
        match self.function {
            VoiFunction::Lookup { .. } => return f32::NAN,
            VoiFunction::LinearExact => {
                return ((f64::from(value) - f64::from(self.center)) / f64::from(self.width) + 0.5)
                    .clamp(0., 1.) as f32;
            }
            VoiFunction::Sigmoid => {
                return (1.
                    / (1.
                        + (-4. * (f64::from(value) - f64::from(self.center))
                            / f64::from(self.width))
                        .exp())) as f32;
            }
            VoiFunction::Linear => (),
        }
        if self.width == 1.0 {
            return if value > self.center - 0.5 { 1.0 } else { 0.0 };
        }
        ((value - (self.center - 0.5)) / (self.width - 1.0) + 0.5).clamp(0.0, 1.0)
    }
    pub const fn lookup(index: u8) -> Self {
        Self {
            width: 1.,
            center: 0.,
            function: VoiFunction::Lookup { index },
        }
    }
    pub fn source_range(low: f32, high: f32) -> Self {
        let width = (f64::from(high) - f64::from(low))
            .clamp(f64::from(f32::MIN_POSITIVE), f64::from(f32::MAX)) as f32;
        Self {
            width: if low == high { 1. } else { width },
            center: (f64::from(low) / 2. + f64::from(high) / 2.) as f32,
            function: VoiFunction::LinearExact,
        }
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
            return Err("Invalid display function or window parameters");
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
            function: Default::default(),
            width: 400.0,
            center: 40.0,
        };
        assert_eq!(w.normalize(-160.0), 0.0);
        assert_eq!(w.normalize(239.0), 1.0);
        assert_eq!(w.normalize(39.5), 0.5);
        let w = WindowLevel {
            function: Default::default(),
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

    #[test]
    fn exact_and_sigmoid_windows_preserve_fractional_widths_and_distinct_identity() {
        let mut window = WindowLevel {
            width: 0.5,
            center: 0.25,
            function: VoiFunction::LinearExact,
        };
        assert!(window.is_valid());
        for (value, expected) in [
            (0., 0.),
            (0.125, 0.25),
            (0.25, 0.5),
            (0.375, 0.75),
            (0.5, 1.),
        ] {
            assert_eq!(window.normalize(value), expected);
        }
        window.function = VoiFunction::Linear;
        assert!(!window.is_valid());
        window = WindowLevel {
            width: 2.,
            center: 0.,
            function: VoiFunction::Sigmoid,
        };
        assert_eq!(window.normalize(0.), 0.5);
        assert!((window.normalize(0.5) - 0.7310586).abs() < 0.000001);
        assert!((window.normalize(-1.) - 0.11920292).abs() < 0.000001);
        assert_eq!(window.normalize(f32::MAX), 1.);
        assert_eq!(window.normalize(-f32::MAX), 0.);
        let legacy: WindowLevel = serde_json::from_str(r#"{"width":200.0,"center":0.0}"#).unwrap();
        assert_eq!(legacy.function, VoiFunction::Linear);
        assert_eq!(
            serde_json::to_string(&legacy).unwrap(),
            r#"{"width":200.0,"center":0.0}"#
        );
        let a = ViewState {
            window,
            ..Default::default()
        };
        let mut b = a.clone();
        b.window.function = VoiFunction::LinearExact;
        assert_ne!(a.hash().unwrap(), b.hash().unwrap());
        assert!(WindowLevel::lookup(0).normalize(0.).is_nan());
        assert!(!WindowLevel::lookup(16).is_valid());
    }
}
