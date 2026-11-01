// SPDX-License-Identifier: GPL-3.0-or-later
//! Immutable completed outlines for separately reviewed local illustrations.
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
struct SourceOutline {
    key: ImageKey,
    mark: Outline,
}
/// A bounded source collection keeps coordinates and original plane identities,
/// without decoding or retaining the other contact-sheet images on the UI thread.
pub(crate) struct SourceOutlines {
    marks: Vec<SourceOutline>,
    _memory: Option<Reservation>,
}
impl SourceOutlines {
    pub fn count(&self) -> usize {
        self.marks.len()
    }
    pub fn for_frame(
        &self,
        image: &Frame,
        view: &ViewState,
        cancel: &impl Fn() -> bool,
    ) -> Result<Outlines, &'static str> {
        if view.plane != orv_core::Plane::Acquired || image.slab.is_some() {
            return Err("Contact-sheet measurements need a source image plane");
        }
        let count = self
            .marks
            .iter()
            .filter(|m| m.key.frame == Some(view.slice))
            .count();
        let references = reference_bytes(image);
        if count > 0 && references > 128 * 1024 {
            return Err("Contact-sheet reference data exceeds its allowance");
        }
        let memory = if count > 0 {
            Some(
                MemoryBudget::volumes()
                    .reserve(OUTLINE_BYTES + references as u64)
                    .ok_or("Not enough memory for tile measurements")?,
            )
        } else {
            None
        };
        let mut marks = Vec::new();
        marks
            .try_reserve_exact(count)
            .map_err(|_| "Not enough memory for tile measurements")?;
        let key = if count > 0 {
            Annotations::default().key(image, view)
        } else {
            empty_key(image, Some(view.slice))
        };
        for captured in self
            .marks
            .iter()
            .filter(|m| m.key.frame == Some(view.slice))
        {
            if cancel() {
                return Err("Illustration operation cancelled");
            }
            if captured.key != key {
                return Err("The contact image differs from its measured source plane");
            }
            marks.push(Outline {
                number: captured.mark.number,
                kind: captured.mark.kind,
                points: history::copy_points(&captured.mark.points)
                    .map_err(|_| "Not enough memory for tile points")?,
            });
        }
        let outlines = Outlines {
            key,
            marks,
            _memory: memory,
        };
        outlines.validate(image, view, cancel)?;
        Ok(outlines)
    }
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
            || !self.marks.is_empty() && self.key.geometry != image.geometry
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
    pub(crate) fn source_outlines(&self) -> Result<SourceOutlines, &'static str> {
        if !self.pending.is_empty() || self.candidate.is_some() || self.editor.unfinished() {
            return Err("Finish or cancel the measurement before preparing a contact sheet");
        }
        if self.preserve_plane
            || self
                .marks
                .iter()
                .any(|m| m.image.frame.is_none() || m.image.slab.is_some())
        {
            return Err("Contact sheets need source-image measurements");
        }
        if self.marks.len() > MAX_MARKS
            || self
                .marks
                .iter()
                .try_fold(0usize, |total, m| total.checked_add(m.points.len()))
                .is_none_or(|n| n > MAX_POINTS)
        {
            return Err("Too many contact-sheet measurement points");
        }
        // Source keys can contain a private reference. Bound their total copy
        // separately rather than assuming every supplied identifier is short.
        let references = self
            .marks
            .iter()
            .try_fold(0usize, |sum, m| {
                sum.checked_add(
                    m.image
                        .geometry
                        .as_ref()
                        .and_then(|g| g.frame_of_reference.as_ref())
                        .map_or(0, String::len),
                )
            })
            .filter(|n| *n <= 128 * 1024)
            .ok_or("Contact-sheet reference data exceeds its allowance")?;
        let memory = if self.marks.is_empty() {
            None
        } else {
            Some(
                MemoryBudget::volumes()
                    .reserve(256 * 1024 + references as u64)
                    .ok_or("Not enough memory for contact-sheet measurements")?,
            )
        };
        let mut marks = Vec::new();
        marks
            .try_reserve_exact(self.marks.len())
            .map_err(|_| "Not enough memory for contact-sheet measurements")?;
        for mark in &self.marks {
            marks.push(SourceOutline {
                key: mark.image.clone(),
                mark: Outline {
                    number: mark.id.saturating_add(1),
                    kind: SavedTool::from_tool(mark.tool)?,
                    points: history::copy_points(&mark.points)
                        .map_err(|_| "Not enough memory for contact-sheet points")?,
                },
            });
        }
        Ok(SourceOutlines {
            marks,
            _memory: memory,
        })
    }
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
        let frame = if self.preserve_plane {
            None
        } else {
            Some(view.slice)
        };
        let matches = |m: &&Mark| {
            m.image.slab == image.slab
                && m.image.size == [image.width, image.height]
                && m.image.geometry == image.geometry
                && m.image.frame == frame
        };
        let count = self.marks.iter().filter(matches).count();
        let total = self
            .marks
            .iter()
            .filter(matches)
            .try_fold(0usize, |n, m| n.checked_add(m.points.len()));
        let references = reference_bytes(image);
        if count > MAX_MARKS || total.is_none_or(|n| n > MAX_POINTS) {
            return Err("Too many illustration measurement points");
        }
        if count > 0 && references > 128 * 1024 {
            return Err("Illustration reference data exceeds its allowance");
        }
        let memory = if count == 0 {
            None
        } else {
            Some(
                budget
                    .reserve(OUTLINE_BYTES + references as u64)
                    .ok_or("Not enough memory for illustration measurements")?,
            )
        };
        let mut marks = Vec::new();
        marks
            .try_reserve_exact(count)
            .map_err(|_| "Not enough memory for illustration measurements")?;
        for mark in self.marks.iter().filter(matches) {
            let points = history::copy_points(&mark.points)
                .map_err(|_| "Not enough memory for illustration points")?;
            marks.push(Outline {
                number: mark.id.saturating_add(1),
                kind: SavedTool::from_tool(mark.tool)?,
                points,
            });
        }
        let key = if count > 0 {
            self.key(image, view)
        } else {
            empty_key(image, frame)
        };
        Ok(Outlines {
            key,
            marks,
            _memory: memory,
        })
    }
}

fn reference_bytes(image: &Frame) -> usize {
    image
        .geometry
        .as_ref()
        .and_then(|g| g.frame_of_reference.as_ref())
        .map_or(0, String::len)
}
fn empty_key(image: &Frame, frame: Option<u32>) -> ImageKey {
    // With no coordinates there is no reference to associate. Keep the cheap
    // raster/ordinal boundary; the captured image/volume still verifies its source.
    ImageKey {
        slab: image.slab,
        size: [image.width, image.height],
        geometry: None,
        frame,
    }
}
