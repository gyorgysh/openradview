// SPDX-License-Identifier: GPL-3.0-or-later
//! Worker-built tuple lookup; selection does not scan studies, decode or prepare volumes.
use super::*;
use crate::{load::LoadError, nifti_view::ComplexView, study::Study};
use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Selection {
    indices: [u32; 4],
    complex: ComplexView,
}
struct Member {
    selection: Selection,
    series: usize,
}
struct Entry {
    axes: NiftiAxes,
    group: usize,
}
#[derive(Default)]
pub struct NiftiCatalog {
    entries: HashMap<usize, Entry>,
    groups: Vec<Vec<Member>>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectionError {
    OutOfRange,
    Unavailable,
    Ambiguous,
}
impl NiftiCatalog {
    pub fn prepare(study: &Study, cancelled: impl Fn() -> bool) -> Result<Self, LoadError> {
        let mut catalog = Self::default();
        let mut families = HashMap::new();
        for (index, series) in study.series.iter().enumerate() {
            if index % 256 == 0 && cancelled() {
                return Err(LoadError::Cancelled);
            }
            let Some(axes) = series.nifti_axes() else {
                continue;
            };
            if catalog.entries.len() >= 4096 {
                return Err(LoadError::Limit);
            }
            let view = axes.view();
            if view.dimensions.contains(&0)
                || view
                    .indices
                    .iter()
                    .zip(view.dimensions)
                    .any(|(i, count)| *i >= count)
            {
                return Err(LoadError::Unsupported);
            }
            let group = if let Some(group) = families.get(&axes.family) {
                *group
            } else {
                catalog
                    .groups
                    .try_reserve(1)
                    .map_err(|_| LoadError::Limit)?;
                families.try_reserve(1).map_err(|_| LoadError::Limit)?;
                let group = catalog.groups.len();
                catalog.groups.push(Vec::new());
                families.insert(axes.family.clone(), group);
                group
            };
            let members = &mut catalog.groups[group];
            members.try_reserve(1).map_err(|_| LoadError::Limit)?;
            members.push(Member {
                selection: Selection {
                    indices: view.indices,
                    complex: view.complex,
                },
                series: index,
            });
            catalog
                .entries
                .try_reserve(1)
                .map_err(|_| LoadError::Limit)?;
            catalog.entries.insert(
                index,
                Entry {
                    axes: axes.clone(),
                    group,
                },
            );
        }
        for members in &mut catalog.groups {
            if cancelled() {
                return Err(LoadError::Cancelled);
            }
            members.sort_unstable_by_key(|member| member.selection);
        }
        Ok(catalog)
    }
    pub fn axes(&self, series: usize) -> Option<&NiftiAxes> {
        Some(&self.entries.get(&series)?.axes)
    }
    pub fn resolve(
        &self,
        source: usize,
        indices: [u32; 4],
        complex: ComplexView,
    ) -> Result<usize, SelectionError> {
        let entry = self
            .entries
            .get(&source)
            .ok_or(SelectionError::Unavailable)?;
        if indices
            .iter()
            .zip(entry.axes.view.dimensions)
            .any(|(i, count)| *i >= count)
        {
            return Err(SelectionError::OutOfRange);
        }
        let members = self
            .groups
            .get(entry.group)
            .ok_or(SelectionError::Unavailable)?;
        let wanted = Selection { indices, complex };
        let index = members.partition_point(|member| member.selection < wanted);
        let member = members
            .get(index)
            .filter(|member| member.selection == wanted)
            .ok_or(SelectionError::Unavailable)?;
        if members
            .get(index + 1)
            .is_some_and(|member| member.selection == wanted)
        {
            return Err(SelectionError::Ambiguous);
        }
        Ok(member.series)
    }
}
