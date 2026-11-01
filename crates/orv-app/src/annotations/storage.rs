// SPDX-License-Identifier: GPL-3.0-or-later
//! Admission for the bounded mark collection, drafts, reports and worker point copies.
use super::*;
use orv_core::resources::MemoryBudget;
const COLLECTION_BYTES: u64 = 1024 * 1024;
impl Annotations {
    pub(super) fn ensure_admitted(&mut self) -> Result<(), String> {
        self.admit_from(&MemoryBudget::volumes())
    }
    pub(super) fn prepare_storage(&mut self, tool: Tool) -> Result<(), String> {
        self.ensure_admitted()?;
        self.marks
            .try_reserve_exact(MAX_MARKS.saturating_sub(self.marks.len()))
            .map_err(|_| "Not enough memory for measurement marks")?;
        let capacity = if tool == Tool::PolygonRoi {
            measurements::MAX_POLYGON_VERTICES
        } else {
            3
        };
        self.pending
            .try_reserve_exact(capacity.saturating_sub(self.pending.len()))
            .map_err(|_| "Not enough memory for measurement points")?;
        Ok(())
    }
    fn admit_from(&mut self, budget: &Arc<MemoryBudget>) -> Result<(), String> {
        if self.memory.is_none() {
            let memory =
                Arc::new(budget.reserve(COLLECTION_BYTES).ok_or(
                    "Not enough memory for measurements. Close another view and try again.",
                )?);
            self.calculation.set_admission(memory.clone());
            self.memory = Some(memory);
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failed_admission_and_clear_release_private_collection_storage() {
        let budget = MemoryBudget::new(COLLECTION_BYTES - 1);
        let mut annotations = Annotations::default();
        assert!(annotations.admit_from(&budget).is_err());
        assert_eq!(budget.used(), 0);
        assert!(annotations.memory.is_none());
        budget.set_limit(COLLECTION_BYTES);
        annotations.admit_from(&budget).unwrap();
        assert_eq!(budget.used(), COLLECTION_BYTES);
        annotations.admit_from(&budget).unwrap();
        assert_eq!(budget.used(), COLLECTION_BYTES);
        annotations.clear();
        assert_eq!(budget.used(), 0);
    }
}
