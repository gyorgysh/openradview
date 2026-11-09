// SPDX-License-Identifier: GPL-3.0-or-later
//! Explicit, source-bound categorical mask combinations.
use super::{LabelScope, Labels};
use crate::{
    mask::{MaskKey, MaskOwner, MaskRevision},
    resources::{MemoryBudget, Reservation},
};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CombineOperation {
    Union,
    Intersection,
    Subtract,
    Copy,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CombineRequest {
    pub scope: LabelScope,
    pub target: u8,
    pub operand: u16,
    pub operation: CombineOperation,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CombineSummary {
    pub added: usize,
    pub removed: usize,
    /// Known, valid requested additions blocked by an existing different ID.
    pub protected: usize,
    pub unknown: usize,
    pub missing_source: usize,
}
/// A RAM-only proposal retaining both immutable source-bound revisions. Other
/// IDs, unknown coverage and missing source samples are never overwritten.
pub struct CombinePreview {
    base: Arc<Labels>,
    operand: MaskOwner,
    key: MaskKey,
    revision: MaskRevision,
    request: CombineRequest,
    offset: usize,
    changes: Vec<u8>,
    summary: CombineSummary,
    _reservation: Reservation,
}
impl CombinePreview {
    pub fn base(&self) -> &Arc<Labels> {
        &self.base
    }
    pub fn operand(&self) -> &MaskOwner {
        &self.operand
    }
    pub fn operand_key(&self) -> &MaskKey {
        &self.key
    }
    pub fn operand_revision(&self) -> &MaskRevision {
        &self.revision
    }
    pub fn request(&self) -> CombineRequest {
        self.request
    }
    pub fn summary(&self) -> CombineSummary {
        self.summary
    }
    /// Scope-local row-major changes: 1 adds, 2 removes, 0 preserves the cell.
    pub fn changes(&self) -> &[u8] {
        &self.changes
    }
    /// Apply only to the same current target and operand revisions. On refusal or
    /// cancellation, neither retained original is mutated or partially published.
    pub fn apply(
        &self,
        current: &Arc<Labels>,
        operand: &MaskOwner,
        cancel: impl Fn() -> bool,
    ) -> Result<Labels, &'static str> {
        if cancel() {
            return Err("Cancelled");
        }
        if !Arc::ptr_eq(current, &self.base) {
            return Err("The target label map has changed since preview");
        }
        let view = operand.view(self.request.operand)?;
        if !Arc::ptr_eq(&operand.source(), &self.base.source_arc())
            || view.key() != self.key
            || view.revision() != &self.revision
        {
            return Err("The operand mask has changed since preview");
        }
        let mut result = current.try_clone(&cancel)?;
        result.begin_edit();
        let mut index = 0;
        while index < self.changes.len() {
            if cancel() {
                return Err("Cancelled");
            }
            let start = self.offset + index;
            let values = result
                .values
                .run(start)
                .ok_or("Label tile is inconsistent")?;
            let count = values.len().min(self.changes.len() - index);
            if count == 0 {
                return Err("Label tile is inconsistent");
            }
            let mut row = [0u8; 64];
            row[..count].copy_from_slice(&values[..count]);
            for (value, change) in row[..count]
                .iter_mut()
                .zip(&self.changes[index..index + count])
            {
                match *change {
                    1 => *value = self.request.target,
                    2 => *value = 0,
                    _ => {}
                }
            }
            result.set_values(start, &row[..count])?;
            index += count;
        }
        if cancel() {
            return Err("Cancelled");
        }
        Ok(result)
    }
}
impl Labels {
    /// Worker-only preview over one slice or the admitted source grid. The
    /// operand must retain the exact same prepared source, not just equal geometry.
    pub fn combine(
        self: &Arc<Self>,
        operand: MaskOwner,
        request: CombineRequest,
        cancel: impl Fn() -> bool,
    ) -> Result<CombinePreview, &'static str> {
        self.combine_with_budget(operand, request, &MemoryBudget::volumes(), &cancel)
    }
    fn combine_with_budget(
        self: &Arc<Self>,
        operand: MaskOwner,
        request: CombineRequest,
        budget: &Arc<MemoryBudget>,
        cancel: &impl Fn() -> bool,
    ) -> Result<CombinePreview, &'static str> {
        if cancel() {
            return Err("Cancelled");
        }
        if request.target == 0 {
            return Err("Choose a nonzero target label ID");
        }
        if !Arc::ptr_eq(&operand.source(), &self.source_arc()) {
            return Err("Mask and labels belong to different prepared sources");
        }
        let view = operand.view(request.operand)?;
        let key = view.key();
        let revision = view.revision().clone();
        let [width, height, depth] = self.source.dimensions();
        let count = Self::check_dimensions([width, height, depth])?;
        let plane = width * height;
        let (offset, count) = match request.scope {
            LabelScope::Slice(z) if z < depth => (z * plane, plane),
            LabelScope::Volume => (0, count),
            _ => return Err("The selected source slice is outside the volume"),
        };
        let reservation = budget
            .reserve(count as u64 + std::mem::size_of::<CombinePreview>() as u64)
            .ok_or("Not enough memory for a mask-combination preview")?;
        let mut changes = Vec::new();
        changes
            .try_reserve_exact(count)
            .map_err(|_| "Not enough memory for a mask-combination preview")?;
        if changes.capacity() != count {
            return Err("The preview allocation exceeds its admitted size");
        }
        let mut summary = CombineSummary::default();
        for index in offset..offset + count {
            if index.is_multiple_of(1024) && cancel() {
                return Err("Cancelled");
            }
            let voxel = [index % width, index % plane / width, index / plane];
            let sample = view.sample(voxel).ok_or("Mask grid is inconsistent")?;
            let value = *self.values.get(index).ok_or("Label tile is inconsistent")?;
            let change = if !sample.source_valid {
                summary.missing_source += 1;
                0
            } else if !sample.known {
                summary.unknown += 1;
                0
            } else {
                let selected = value == request.target;
                let desired = match request.operation {
                    CombineOperation::Union => selected || sample.present,
                    CombineOperation::Intersection => selected && sample.present,
                    CombineOperation::Subtract => selected && !sample.present,
                    CombineOperation::Copy => sample.present,
                };
                if desired && !selected && value != 0 {
                    summary.protected += 1;
                    0
                } else if desired && !selected {
                    summary.added += 1;
                    1
                } else if !desired && selected {
                    summary.removed += 1;
                    2
                } else {
                    0
                }
            };
            changes.push(change);
        }
        if cancel() {
            return Err("Cancelled");
        }
        Ok(CombinePreview {
            base: self.clone(),
            operand,
            key,
            revision,
            request,
            offset,
            changes,
            summary,
            _reservation: reservation,
        })
    }
}
#[cfg(test)]
mod tests;
