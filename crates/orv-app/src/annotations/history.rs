// SPDX-License-Identifier: GPL-3.0-or-later
//! Reversible geometry/report pairs from already validated edits; no source pixels.
use super::*;
use orv_core::resources::{MemoryBudget, Reservation};
const LIMIT: usize = 16;
pub(super) const MAX_REPORT_BYTES: usize = 4096;
struct Change {
    id: u64,
    points: Vec<[f64; 2]>,
    label: String,
    _memory: Reservation,
}
impl Change {
    fn capture(mark: &Mark, budget: &Arc<MemoryBudget>) -> Result<Self, String> {
        if mark.points.len() > measurements::MAX_POLYGON_VERTICES
            || mark.label.len() > MAX_REPORT_BYTES
        {
            return Err("Measurement history exceeds its storage allowance".into());
        }
        let bytes = std::mem::size_of::<Self>() + mark.points.len() * 16 + MAX_REPORT_BYTES + 64;
        let memory = budget
            .reserve(bytes as u64)
            .ok_or("Not enough memory for measurement undo. Current geometry was kept.")?;
        let points = copy_points(&mark.points)?;
        let mut label = String::new();
        label
            .try_reserve_exact(mark.label.len())
            .map_err(|_| "Not enough memory for measurement undo")?;
        label.push_str(&mark.label);
        Ok(Self {
            id: mark.id,
            points,
            label,
            _memory: memory,
        })
    }
}
pub(super) fn copy_points(points: &[[f64; 2]]) -> Result<Vec<[f64; 2]>, String> {
    let mut copy = Vec::new();
    copy.try_reserve_exact(points.len())
        .map_err(|_| "Not enough memory for measurement coordinates")?;
    copy.extend_from_slice(points);
    Ok(copy)
}
#[derive(Default)]
pub(super) struct History {
    undo: Vec<Change>,
    redo: Vec<Change>,
}
impl History {
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
    pub fn record(&mut self, mark: &Mark) -> Result<(), String> {
        self.record_with(mark, &MemoryBudget::volumes())
    }
    fn record_with(&mut self, mark: &Mark, budget: &Arc<MemoryBudget>) -> Result<(), String> {
        let change = Change::capture(mark, budget)?;
        for stack in [&mut self.undo, &mut self.redo] {
            stack
                .try_reserve_exact(LIMIT.saturating_sub(stack.len()))
                .map_err(|_| "Not enough memory for measurement history")?;
        }
        self.redo.clear();
        if self.undo.len() == LIMIT {
            self.undo.remove(0);
        }
        self.undo.push(change);
        Ok(())
    }
}
impl Annotations {
    pub(super) fn undo_edit(&mut self, redo: bool) {
        self.cancel_draft();
        let source = if redo {
            &mut self.history.redo
        } else {
            &mut self.history.undo
        };
        let Some(mut change) = source.pop() else {
            return;
        };
        let Some(mark) = self.marks.iter_mut().find(|mark| mark.id == change.id) else {
            self.history = History::default();
            return;
        };
        self.calculation.cancel_for(mark.id);
        std::mem::swap(&mut mark.points, &mut change.points);
        std::mem::swap(&mut mark.label, &mut change.label);
        // Reports travel with their validated coordinates on this unchanged source.
        // Revisions never travel backwards, so an older job cannot replace them.
        mark.revision = self.next_revision;
        self.next_revision = self.next_revision.wrapping_add(1);
        self.selected = Some(mark.id);
        if redo {
            self.history.undo.push(change);
        } else {
            self.history.redo.push(change);
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn mark() -> Mark {
        Mark {
            id: 7,
            revision: 0,
            image: ImageKey {
                slab: None,
                size: [5, 5],
                geometry: None,
                frame: Some(0),
            },
            points: vec![[1., 1.], [3., 3.]],
            tool: Tool::EllipseRoi,
            label: "original report".into(),
        }
    }
    #[test]
    fn history_pressure_refuses_without_losing_entries_and_total_storage_stays_bounded() {
        let mark = mark();
        let bytes = (std::mem::size_of::<Change>() + 32 + MAX_REPORT_BYTES + 64) as u64;
        let budget = MemoryBudget::new(bytes);
        let mut history = History::default();
        history.record_with(&mark, &budget).unwrap();
        assert_eq!(budget.used(), bytes);
        assert!(history.record_with(&mark, &budget).is_err());
        assert_eq!(history.undo.len(), 1);
        assert_eq!(history.undo[0].label, "original report");
        budget.set_limit(bytes * 20);
        for _ in 0..32 {
            history.record_with(&mark, &budget).unwrap();
        }
        assert_eq!(history.undo.len(), LIMIT);
        assert_eq!(budget.used(), bytes * LIMIT as u64);
        let mut annotations = Annotations {
            marks: vec![mark],
            history,
            next_revision: 1,
            ..Default::default()
        };
        for _ in 0..LIMIT {
            annotations.undo_edit(false);
        }
        assert!(!annotations.history.can_undo());
        assert_eq!(annotations.history.redo.len(), LIMIT);
        assert_eq!(budget.used(), bytes * LIMIT as u64);
        annotations
            .history
            .record_with(&annotations.marks[0], &budget)
            .unwrap();
        assert!(!annotations.history.can_redo());
        assert_eq!(budget.used(), bytes);
        annotations.clear();
        assert_eq!(budget.used(), 0);
    }
    #[test]
    fn deletion_drops_history_and_late_old_revisions_cannot_replace_undo_reports() {
        let ctx = egui::Context::default();
        let image = orv_core::study::demo_study().series[0].frames[0].clone();
        let mut annotations = Annotations::default();
        annotations.ensure_admitted().unwrap();
        annotations.bind_image(&image, &ViewState::default());
        let mut mark = mark();
        mark.image = annotations.image.clone().unwrap();
        annotations.history.record(&mark).unwrap();
        mark.label = "changed report".into();
        mark.revision = 1;
        annotations.marks.push(mark);
        annotations.next_revision = 2;
        annotations
            .calculation
            .start(
                7,
                1,
                image.clone(),
                vec![[1., 1.], [3., 3.]],
                Tool::EllipseRoi,
                &ctx,
            )
            .unwrap();
        annotations.undo_edit(false);
        assert_eq!(annotations.marks[0].label, "original report");
        assert_eq!(annotations.marks[0].revision, 2);
        assert!(!annotations.calculation.is_busy());
        annotations.poll();
        assert_eq!(annotations.marks[0].label, "original report");
        annotations.delete_selected();
        assert!(annotations.marks.is_empty());
        assert!(!annotations.history.can_redo());
    }
}
