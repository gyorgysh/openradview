// SPDX-License-Identifier: GPL-3.0-or-later
//! Immutable completed current-plane marks for a separately reviewed local illustration.
use super::*;
use orv_core::resources::{MemoryBudget, Reservation};
const OUTLINE_BYTES: u64 = 128 * 1024;
#[cfg(test)]
#[path = "outlines_tests.rs"]
mod tests;
#[derive(Serialize)]
pub(crate) struct Outline {
    number: u64,
    kind: SavedTool,
    points: Vec<[f64; 2]>,
}
pub(crate) struct Outlines {
    key: ImageKey,
    marks: Vec<Outline>,
    _memory: Option<Reservation>,
}
impl Outlines {
    pub fn check_drawing_size(
        &self,
        rect: Rect,
        image: &Frame,
        view: &ViewState,
    ) -> Result<(), &'static str> {
        let mut total = 0usize;
        for mark in &self.marks {
            let count = drawing::segments(rect, image, view, &mark.points, mark.kind.tool())
                .ok_or("The measurement view cannot be mapped")?;
            total = total.checked_add(count).filter(|n| *n <= 32 * 1024).ok_or(
                "Too many projected outlines. Zoom out or exclude marks from this illustration",
            )?;
        }
        Ok(())
    }
    pub fn marks(&self) -> &[Outline] {
        &self.marks
    }
    pub fn validate(
        &self,
        image: &Frame,
        view: &ViewState,
        cancelled: &impl Fn() -> bool,
    ) -> Result<(), &'static str> {
        if cancelled() {
            return Err("Illustration operation cancelled");
        }
        if self.key.slab != image.slab
            || self.key.size != [image.width, image.height]
            || self.key.geometry != image.geometry
            || self.key.frame.is_some_and(|n| n != view.slice)
        {
            return Err("The illustration measurement plane changed");
        }
        for mark in &self.marks {
            validate_points(image, mark.kind.tool(), &mark.points, cancelled)?;
        }
        Ok(())
    }
    pub fn paint(&self, painter: &egui::Painter, rect: Rect, image: &Frame, view: &ViewState) {
        for mark in &self.marks {
            drawing::geometry(
                painter,
                rect,
                image,
                view,
                &mark.points,
                mark.kind.tool(),
                false,
            );
            if let Some(point) = mark.points.first() {
                drawing::number(painter, rect, image, view, *point, mark.number, false);
            }
        }
    }
}
impl Annotations {
    pub(crate) fn outlines(
        &self,
        image: &Frame,
        view: &ViewState,
    ) -> Result<Outlines, &'static str> {
        self.outlines_with_budget(image, view, &MemoryBudget::volumes())
    }
    fn outlines_with_budget(
        &self,
        image: &Frame,
        view: &ViewState,
        budget: &Arc<MemoryBudget>,
    ) -> Result<Outlines, &'static str> {
        if !self.pending.is_empty() || self.candidate.is_some() || self.editor.unfinished() {
            return Err(
                "Finish or cancel the current measurement before preparing an illustration",
            );
        }
        if image.slab.is_some() {
            return Err("Measurement illustrations need a thin image plane");
        }
        let key = self.key(image, view);
        let count = self.marks.iter().filter(|m| m.image == key).count();
        let total = self
            .marks
            .iter()
            .filter(|m| m.image == key)
            .try_fold(0usize, |n, m| n.checked_add(m.points.len()));
        if count > MAX_MARKS || total.is_none_or(|n| n > MAX_POINTS) {
            return Err("Too many illustration measurement points");
        }
        let memory = if count == 0 {
            None
        } else {
            Some(
                budget
                    .reserve(OUTLINE_BYTES)
                    .ok_or("Not enough memory for illustration measurements")?,
            )
        };
        let mut marks = Vec::new();
        marks
            .try_reserve_exact(count)
            .map_err(|_| "Not enough memory for illustration measurements")?;
        for mark in self.marks.iter().filter(|m| m.image == key) {
            let points = history::copy_points(&mark.points)
                .map_err(|_| "Not enough memory for illustration points")?;
            marks.push(Outline {
                number: mark.id.saturating_add(1),
                kind: SavedTool::from_tool(mark.tool)?,
                points,
            });
        }
        Ok(Outlines {
            key,
            marks,
            _memory: memory,
        })
    }
}
