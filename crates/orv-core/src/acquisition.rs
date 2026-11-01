// SPDX-License-Identifier: GPL-3.0-or-later
//! Explicit nonspatial coordinates. Families are local import capabilities, not names or UIDs.
mod catalog;
mod cine;
mod dicom;
use crate::nifti_view::NiftiView;
pub use catalog::{NiftiCatalog, SelectionError};
pub(crate) use cine::read_cine;
pub use cine::{CineError, CineMetadata, CineTiming};
pub use dicom::{AxesError, DicomAxes, DicomAxis, DicomCatalog, DimensionValue};
use std::{
    hash::{Hash, Hasher},
    sync::Arc,
};

#[derive(Clone)]
pub(crate) struct Family(Arc<()>);
impl Family {
    pub(crate) fn new() -> Self {
        Self(Arc::new(()))
    }
}
impl PartialEq for Family {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
impl Eq for Family {}
impl Hash for Family {
    fn hash<H: Hasher>(&self, state: &mut H) {
        std::ptr::hash(Arc::as_ptr(&self.0), state);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrequencyUnit {
    Hertz,
    PartsPerMillion,
    RadiansPerSecond,
}
impl FrequencyUnit {
    pub fn label(self) -> &'static str {
        match self {
            Self::Hertz => "Hz",
            Self::PartsPerMillion => "ppm",
            Self::RadiansPerSecond => "rad/s",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimingError {
    Units,
    Nonfinite,
    Step,
    Range,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Sampling {
    Absent,
    UnknownUnits,
    Invalid(TimingError),
    Seconds {
        step: f64,
        offset: f64,
    },
    Frequency {
        step: f64,
        offset: f64,
        unit: FrequencyUnit,
    },
}
impl Sampling {
    pub(crate) fn nifti(count: u32, units: u32, step: f64, offset: f64) -> Self {
        if count <= 1 {
            return Self::Absent;
        }
        if units & !63 != 0 {
            return Self::Invalid(TimingError::Units);
        }
        let temporal = units & 56;
        if temporal == 0 {
            return Self::UnknownUnits;
        }
        let scale = match temporal {
            8 => 1.,
            16 => 0.001,
            24 => 0.000001,
            32 | 40 | 48 => 1.,
            _ => return Self::Invalid(TimingError::Units),
        };
        if !step.is_finite() || !offset.is_finite() {
            return Self::Invalid(TimingError::Nonfinite);
        }
        let (step, offset) = (step * scale, offset * scale);
        if step <= 0. {
            return Self::Invalid(TimingError::Step);
        }
        if !step.is_finite()
            || !offset.is_finite()
            || !(offset + step * f64::from(count - 1)).is_finite()
        {
            return Self::Invalid(TimingError::Range);
        }
        if count > 4096 {
            return Self::Invalid(TimingError::Range);
        }
        // The importer already bounds this axis. Check every increment because
        // rounding at a large offset can collapse an interior pair of points.
        let mut previous = offset;
        for index in 1..count {
            let coordinate = offset + step * f64::from(index);
            if coordinate <= previous {
                return Self::Invalid(TimingError::Range);
            }
            previous = coordinate;
        }
        match temporal {
            8 | 16 | 24 => Self::Seconds { step, offset },
            _ => Self::Frequency {
                step,
                offset,
                unit: match temporal {
                    32 => FrequencyUnit::Hertz,
                    40 => FrequencyUnit::PartsPerMillion,
                    _ => FrequencyUnit::RadiansPerSecond,
                },
            },
        }
    }
}

/// Auxiliary header metadata; serialized image/view identity retains NiftiView unchanged.
#[derive(Clone)]
pub struct NiftiAxes {
    family: Family,
    view: NiftiView,
    sampling: Sampling,
}
impl NiftiAxes {
    pub(crate) fn new(family: Family, view: NiftiView, sampling: Sampling) -> Self {
        Self {
            family,
            view,
            sampling,
        }
    }
    pub fn view(&self) -> &NiftiView {
        &self.view
    }
    pub fn sampling(&self) -> Sampling {
        self.sampling
    }
    pub fn coordinate(&self) -> Option<f64> {
        let (step, offset) = match self.sampling {
            Sampling::Seconds { step, offset } | Sampling::Frequency { step, offset, .. } => {
                (step, offset)
            }
            _ => return None,
        };
        (self.view.indices[0] < self.view.dimensions[0])
            .then(|| offset + step * f64::from(self.view.indices[0]))
    }
}
impl std::fmt::Debug for NiftiAxes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NiftiAxes")
            .field("view", &self.view)
            .field("sampling", &self.sampling)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests;
