// SPDX-License-Identifier: GPL-3.0-or-later
//! Typed source intensity and grayscale choices. No free-text labels or raw tags.
use crate::{WindowLevel, view::VoiFunction};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Storage {
    UnsignedInteger,
    SignedInteger,
    Float,
    Complex,
}
impl Storage {
    pub fn label(self) -> &'static str {
        match self {
            Self::UnsignedInteger => "unsigned integer",
            Self::SignedInteger => "signed integer",
            Self::Float => "floating point",
            Self::Complex => "complex",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ValueType {
    #[serde(rename = "f32")]
    F32,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Unit {
    #[default]
    Unknown,
    Hounsfield,
    OpticalDensity,
    Counts,
    CountsPerSecond,
    BecquerelsPerMillilitre,
    Radians,
}
impl Unit {
    pub fn label(self) -> &'static str {
        match self {
            Self::Unknown => "Unknown intensity units",
            Self::Hounsfield => "Declared HU",
            Self::OpticalDensity => "Declared optical density",
            Self::Counts => "Declared counts",
            Self::CountsPerSecond => "Declared counts/s",
            Self::BecquerelsPerMillilitre => "Declared Bq/mL",
            Self::Radians => "Radians",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rescale {
    pub slope: f64,
    pub intercept: f64,
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScalarInfo {
    pub value_type: ValueType,
    pub storage: Storage,
    pub stored_bits: u16,
    pub unit: Unit,
    /// Source mapping only. Resampled images do not claim one per-frame mapping.
    pub rescale: Option<Rescale>,
    pub resampled: bool,
}
impl ScalarInfo {
    pub fn validate(&self) -> bool {
        (match self.storage {
            Storage::UnsignedInteger | Storage::SignedInteger => {
                (1..=64).contains(&self.stored_bits)
            }
            Storage::Float => matches!(self.stored_bits, 32 | 64),
            Storage::Complex => matches!(self.stored_bits, 64 | 128),
        }) && self.rescale.is_none_or(|scale| {
            scale.slope.is_finite() && scale.slope != 0. && scale.intercept.is_finite()
        }) && !(self.resampled && self.rescale.is_some())
    }
}
#[derive(Clone, Debug, PartialEq)]
pub struct VoiLut {
    first: i32,
    bits: u16,
    entries: Arc<[u16]>,
    digest: [u8; 32],
}
impl VoiLut {
    pub fn new(first: i32, bits: u16, entries: Arc<[u16]>) -> Result<Self, &'static str> {
        if !(-32768..=65535).contains(&first)
            || !matches!(bits, 8 | 16)
            || entries.is_empty()
            || entries.len() > 65536
            || (bits == 8 && entries.iter().any(|value| *value > 255))
        {
            return Err("Unsupported VOI lookup table");
        }
        let mut hash = Sha256::new();
        hash.update(b"OpenRadView VOI table v1\0");
        hash.update(first.to_le_bytes());
        hash.update(bits.to_le_bytes());
        hash.update((entries.len() as u32).to_le_bytes());
        for value in entries.iter() {
            hash.update(value.to_le_bytes());
        }
        Ok(Self {
            first,
            bits,
            entries,
            digest: hash.finalize().into(),
        })
    }
    pub fn first(&self) -> i32 {
        self.first
    }
    pub fn bits(&self) -> u16 {
        self.bits
    }
    pub fn entries(&self) -> &[u16] {
        &self.entries
    }
    pub fn digest(&self) -> [u8; 32] {
        self.digest
    }
    /// Piecewise-constant lookup after scalar interpolation; indices round down.
    pub fn normalize(&self, value: f32) -> Option<f32> {
        if !value.is_finite() {
            return None;
        }
        let index = (f64::from(value) - f64::from(self.first))
            .floor()
            .clamp(0., (self.entries.len() - 1) as f64) as usize;
        self.entries
            .get(index)
            .map(|value| f32::from(*value) / if self.bits == 8 { 255. } else { 65535. })
    }
}
#[derive(Clone, Debug)]
pub struct VoiChoices {
    windows: Vec<WindowLevel>,
    luts: Vec<VoiLut>,
    digest: [u8; 32],
}
impl VoiChoices {
    pub fn new(windows: Vec<WindowLevel>, luts: Vec<VoiLut>) -> Result<Arc<Self>, &'static str> {
        if windows.len() > 16
            || luts.len() > 16
            || windows.iter().any(|window| {
                !window.is_valid() || matches!(window.function, VoiFunction::Lookup { .. })
            })
        {
            return Err("Unsupported grayscale choices");
        }
        let mut hash = Sha256::new();
        hash.update(b"OpenRadView grayscale choices v1\0");
        hash.update(serde_json::to_vec(&windows).map_err(|_| "Invalid grayscale choices")?);
        hash.update((luts.len() as u32).to_le_bytes());
        for lut in &luts {
            hash.update(lut.digest);
        }
        Ok(Arc::new(Self {
            windows,
            luts,
            digest: hash.finalize().into(),
        }))
    }
    pub fn windows(&self) -> &[WindowLevel] {
        &self.windows
    }
    pub fn luts(&self) -> &[VoiLut] {
        &self.luts
    }
    pub fn digest(&self) -> [u8; 32] {
        self.digest
    }
    pub fn original(&self) -> Option<WindowLevel> {
        self.windows
            .first()
            .copied()
            .or_else(|| (!self.luts.is_empty()).then_some(WindowLevel::lookup(0)))
    }
    pub(crate) fn byte_len(&self) -> u64 {
        std::mem::size_of::<Self>() as u64
            + (self.windows.len() * std::mem::size_of::<WindowLevel>()) as u64
            + self
                .luts
                .iter()
                .map(|lut| std::mem::size_of::<VoiLut>() as u64 + lut.entries.len() as u64 * 2)
                .sum::<u64>()
    }
}
#[derive(Clone, Debug)]
pub struct ScalarData {
    info: ScalarInfo,
    choices: Arc<VoiChoices>,
    unit_identity: Option<[u8; 32]>,
    digest: [u8; 32],
}
impl ScalarData {
    pub fn new(info: ScalarInfo, choices: Arc<VoiChoices>) -> Result<Arc<Self>, &'static str> {
        Self::with_unit_identity(info, choices, None)
    }
    pub(crate) fn with_unit_identity(
        info: ScalarInfo,
        choices: Arc<VoiChoices>,
        unit_identity: Option<[u8; 32]>,
    ) -> Result<Arc<Self>, &'static str> {
        if !info.validate() {
            return Err("Invalid scalar source context");
        }
        let digest = context_digest(info, choices.digest, unit_identity);
        Ok(Arc::new(Self {
            info,
            choices,
            unit_identity,
            digest,
        }))
    }
    pub fn info(&self) -> ScalarInfo {
        self.info
    }
    pub fn choices(&self) -> &Arc<VoiChoices> {
        &self.choices
    }
    pub fn digest(&self) -> [u8; 32] {
        self.digest
    }
    pub fn compatible(&self, other: &Self) -> bool {
        self.info.storage == other.info.storage
            && self.info.stored_bits == other.info.stored_bits
            && self.info.unit == other.info.unit
            && self.unit_identity == other.unit_identity
            && self.choices.luts.len() == other.choices.luts.len()
            && self
                .choices
                .luts
                .iter()
                .zip(&other.choices.luts)
                .all(|(a, b)| a.digest == b.digest)
    }
    pub fn resampled(&self) -> Arc<Self> {
        let mut info = self.info;
        info.rescale = None;
        info.resampled = true;
        Arc::new(Self {
            info,
            choices: self.choices.clone(),
            unit_identity: self.unit_identity,
            digest: context_digest(info, self.choices.digest, self.unit_identity),
        })
    }
    pub fn supports(&self, window: WindowLevel) -> bool {
        window.is_valid()
            && match window.function {
                VoiFunction::Lookup { index } => {
                    self.choices.luts.get(usize::from(index)).is_some()
                }
                _ => true,
            }
    }
    pub fn normalize(&self, window: WindowLevel, value: f32) -> Option<f32> {
        if !self.supports(window) || !value.is_finite() {
            return None;
        }
        match window.function {
            VoiFunction::Lookup { index } => {
                self.choices.luts.get(usize::from(index))?.normalize(value)
            }
            _ => Some(window.normalize(value)),
        }
    }
}

fn context_digest(info: ScalarInfo, choices: [u8; 32], unit: Option<[u8; 32]>) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(b"OpenRadView scalar context v1\0");
    hash.update([
        info.storage as u8,
        info.unit as u8,
        u8::from(info.resampled),
        info.value_type as u8,
    ]);
    hash.update(info.stored_bits.to_le_bytes());
    hash.update([u8::from(info.rescale.is_some())]);
    if let Some(scale) = info.rescale {
        hash.update(scale.slope.to_le_bytes());
        hash.update(scale.intercept.to_le_bytes());
    }
    hash.update(choices);
    hash.update([u8::from(unit.is_some())]);
    if let Some(identity) = unit {
        hash.update(identity);
    }
    hash.finalize().into()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn signed_tables_clip_endpoints_without_rescaling_source_values() {
        let lut = VoiLut::new(-2, 8, Arc::from([0, 64, 192, 255])).unwrap();
        for (value, expected) in [
            (-100., 0.),
            (-2., 0.),
            (-1., 64. / 255.),
            (-0.5, 64. / 255.),
            (0., 192. / 255.),
            (1., 1.),
            (100., 1.),
        ] {
            assert_eq!(lut.normalize(value), Some(expected));
        }
        assert_eq!(lut.normalize(f32::NAN), None);
        assert!(VoiLut::new(0, 8, Arc::from([256])).is_err());
        assert!(VoiLut::new(0, 12, Arc::from([1])).is_err());
        assert!(VoiLut::new(0, 16, Arc::from([])).is_err());
        assert!(VoiLut::new(-32769, 16, Arc::from([1])).is_err());
    }
    #[test]
    fn source_units_precision_and_choices_are_bound_to_immutable_context() {
        let choices = VoiChoices::new(
            Vec::new(),
            vec![VoiLut::new(100, 16, Arc::from([0, 32768, 65535])).unwrap()],
        )
        .unwrap();
        let info = ScalarInfo {
            value_type: crate::scalar::ValueType::F32,
            storage: Storage::SignedInteger,
            stored_bits: 12,
            unit: Unit::Unknown,
            rescale: Some(Rescale {
                slope: 2.,
                intercept: -100.,
            }),
            resampled: false,
        };
        let data = ScalarData::new(info, choices.clone()).unwrap();
        assert_eq!(choices.original(), Some(WindowLevel::lookup(0)));
        assert_eq!(
            data.normalize(WindowLevel::lookup(0), 101.),
            Some(32768. / 65535.)
        );
        assert!(!data.supports(WindowLevel::lookup(1)));
        let mut alternate = info;
        alternate.stored_bits = 16;
        let precision = ScalarData::new(alternate, choices.clone()).unwrap();
        assert_ne!(data.digest(), precision.digest());
        assert!(!data.compatible(&precision));
        alternate = info;
        alternate.unit = Unit::Hounsfield;
        let declared = ScalarData::new(alternate, choices.clone()).unwrap();
        assert_ne!(data.digest(), declared.digest());
        assert!(!data.compatible(&declared));
        alternate = info;
        alternate.rescale.as_mut().unwrap().slope = 3.;
        let scaled = ScalarData::new(alternate, choices.clone()).unwrap();
        assert!(data.compatible(&scaled));
        assert_ne!(data.digest(), scaled.digest());
        let plane = data.resampled();
        assert_eq!(plane.info().rescale, None);
        assert!(plane.info().resampled);
        assert!(Arc::ptr_eq(plane.choices(), &choices));
        assert_eq!(
            plane.normalize(WindowLevel::lookup(0), 101.),
            data.normalize(WindowLevel::lookup(0), 101.)
        );
    }
}
