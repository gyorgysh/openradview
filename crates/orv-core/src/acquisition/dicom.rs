// SPDX-License-Identifier: GPL-3.0-or-later
//! Declared enhanced-DICOM dimensions, with ordinals separate from pointed values.
use super::SelectionError;
use crate::{load::LoadError, study::Study};
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DicomAxis {
    TemporalPosition,
    EchoTime,
}
impl DicomAxis {
    pub const ALL: [Self; 2] = [Self::TemporalPosition, Self::EchoTime];
    pub fn index(self) -> usize {
        match self {
            Self::TemporalPosition => 0,
            Self::EchoTime => 1,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::TemporalPosition => "Temporal position",
            Self::EchoTime => "Echo time",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DimensionValue {
    pub ordinal: u32,
    pub value: Option<f64>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AxesError {
    Incomplete,
    Unsupported,
    Inconsistent,
    Limit,
}
impl AxesError {
    pub fn text(self) -> &'static str {
        match self {
            Self::Incomplete => {
                "Acquisition axes need complete declared dimension organizations and index values."
            }
            Self::Unsupported => {
                "This dimension definition has no supported acquisition selector. Use the series list."
            }
            Self::Inconsistent => {
                "The declared dimension values are inconsistent or ambiguous. Use the series list."
            }
            Self::Limit => {
                "Acquisition navigation metadata reached its resource limit. Use the series list."
            }
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DicomAxes {
    pub(crate) values: [Option<DimensionValue>; 2],
    pub(crate) other_dimensions: bool,
}
impl DicomAxes {
    pub fn value(&self, axis: DicomAxis) -> Option<DimensionValue> {
        self.values[axis.index()]
    }
    pub fn other_dimensions(&self) -> bool {
        self.other_dimensions
    }
    fn ordinals(&self) -> [u32; 2] {
        self.values.map(|v| v.map_or(0, |v| v.ordinal))
    }
}
struct Entry {
    axes: Result<DicomAxes, AxesError>,
    group: Option<usize>,
}
#[derive(Default)]
struct Group {
    members: Vec<([u32; 2], usize)>,
    options: [Vec<DimensionValue>; 2],
    invalid: bool,
}
#[derive(Default)]
pub struct DicomCatalog {
    entries: HashMap<usize, Entry>,
    groups: Vec<Group>,
    timings: HashMap<usize, super::CineMetadata>,
}
impl DicomCatalog {
    pub fn prepare(study: &Study, cancelled: impl Fn() -> bool) -> Result<Self, LoadError> {
        let mut catalog = Self::default();
        let mut reader = crate::index::AxisReader::default();
        let mut families = HashMap::new();
        if study.series.len() > 4096 {
            return Err(LoadError::Limit);
        }
        for (index, series) in study.series.iter().enumerate() {
            if cancelled() {
                return Err(LoadError::Cancelled);
            }
            let Some(source) = &series.source else {
                continue;
            };
            let timing = source.cine_metadata(&cancelled)?;
            if !matches!(timing, super::CineMetadata::Absent) {
                catalog
                    .timings
                    .try_reserve(1)
                    .map_err(|_| LoadError::Limit)?;
                catalog.timings.insert(index, timing);
            }
            let Some(result) = reader.read(source, &cancelled)? else {
                continue;
            };
            catalog
                .entries
                .try_reserve(1)
                .map_err(|_| LoadError::Limit)?;
            let raw = match result {
                Ok(raw) => raw,
                Err(error) => {
                    catalog.entries.insert(
                        index,
                        Entry {
                            axes: Err(error),
                            group: None,
                        },
                    );
                    continue;
                }
            };
            let group = if let Some(group) = families.get(&raw.scope) {
                *group
            } else {
                families.try_reserve(1).map_err(|_| LoadError::Limit)?;
                catalog
                    .groups
                    .try_reserve(1)
                    .map_err(|_| LoadError::Limit)?;
                let group = catalog.groups.len();
                catalog.groups.push(Group::default());
                families.insert(raw.scope, group);
                group
            };
            let target = &mut catalog.groups[group];
            for (options, value) in target.options.iter_mut().zip(raw.axes.values) {
                if let Some(value) = value {
                    if let Some(previous) = options.iter().find(|v| v.ordinal == value.ordinal) {
                        target.invalid |= *previous != value;
                    } else {
                        options.try_reserve(1).map_err(|_| LoadError::Limit)?;
                        options.push(value);
                    }
                }
            }
            target
                .members
                .try_reserve(1)
                .map_err(|_| LoadError::Limit)?;
            target.members.push((raw.axes.ordinals(), index));
            catalog.entries.insert(
                index,
                Entry {
                    axes: Ok(raw.axes),
                    group: Some(group),
                },
            );
        }
        for group in &mut catalog.groups {
            if cancelled() {
                return Err(LoadError::Cancelled);
            }
            group.members.sort_unstable();
            for options in &mut group.options {
                options.sort_unstable_by_key(|v| v.ordinal);
            }
        }
        Ok(catalog)
    }
    pub fn axes(&self, series: usize) -> Option<Result<&DicomAxes, AxesError>> {
        let entry = self.entries.get(&series)?;
        if entry.group.is_some_and(|group| self.groups[group].invalid) {
            return Some(Err(AxesError::Inconsistent));
        }
        Some(entry.axes.as_ref().map_err(|error| *error))
    }
    pub fn options(&self, series: usize, axis: DicomAxis) -> &[DimensionValue] {
        self.entries
            .get(&series)
            .and_then(|entry| entry.group)
            .and_then(|group| self.groups.get(group))
            .filter(|group| !group.invalid)
            .map_or(&[], |group| group.options[axis.index()].as_slice())
    }
    pub fn resolve(&self, series: usize, ordinals: [u32; 2]) -> Result<usize, SelectionError> {
        self.axes(series)
            .ok_or(SelectionError::Unavailable)?
            .map_err(|_| SelectionError::Unavailable)?;
        let group = self
            .entries
            .get(&series)
            .and_then(|entry| entry.group)
            .and_then(|index| self.groups.get(index))
            .ok_or(SelectionError::Unavailable)?;
        let index = group
            .members
            .partition_point(|(value, _)| *value < ordinals);
        let (_, series) = group
            .members
            .get(index)
            .filter(|(value, _)| *value == ordinals)
            .ok_or(SelectionError::Unavailable)?;
        if group
            .members
            .get(index + 1)
            .is_some_and(|(value, _)| *value == ordinals)
        {
            return Err(SelectionError::Ambiguous);
        }
        Ok(*series)
    }
    pub fn cine(&self, series: usize) -> Option<&super::CineMetadata> {
        self.timings.get(&series)
    }
}
