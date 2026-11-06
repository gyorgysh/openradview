// SPDX-License-Identifier: GPL-3.0-or-later
//! Deterministic, bounded two-class propagation over one acquired scalar plane.
use super::Labels;
use crate::resources::{MemoryBudget, Reservation};
use std::sync::Arc;

const MAX_CELLS: usize = 1024 * 1024;
const MAX_ITERATIONS: u32 = 4096;
const MAX_WORK: usize = 64 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SeedClass {
    Include,
    Exclude,
}
impl SeedClass {
    fn code(self) -> u8 {
        match self {
            Self::Include => 1,
            Self::Exclude => 2,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SeedPoint {
    /// Source-image column and row, zero-based.
    pub position: [usize; 2],
    pub class: SeedClass,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SeededRequest {
    pub slice: usize,
    /// Inclusive source-image origin and dimensions of the selected rectangle.
    pub origin: [usize; 2],
    pub size: [usize; 2],
    pub label: u8,
    pub max_iterations: u32,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SeededSummary {
    pub iterations: u32,
    pub included: usize,
    pub excluded: usize,
    pub unresolved: usize,
    pub added: usize,
    pub removed: usize,
}
/// RAM-only result tied to its original source and label revision. Only a fully
/// converged calculation is returned; other IDs and missing samples are barriers.
pub struct SeededRegion {
    base: Arc<Labels>,
    request: SeededRequest,
    seeds: Vec<u8>,
    classes: Vec<u8>,
    summary: SeededSummary,
    _reservation: Reservation,
}
impl SeededRegion {
    pub fn base(&self) -> &Arc<Labels> {
        &self.base
    }
    pub fn request(&self) -> SeededRequest {
        self.request
    }
    pub fn summary(&self) -> SeededSummary {
        self.summary
    }
    /// Row-major rectangle, with None for a barrier or unreached sample.
    pub fn classes(&self) -> impl Iterator<Item = Option<SeedClass>> + '_ {
        self.classes.iter().map(|v| match v {
            1 => Some(SeedClass::Include),
            2 => Some(SeedClass::Exclude),
            _ => None,
        })
    }
    /// The retained seed set has the same source-grid indexing as `classes`.
    pub fn seeds(&self) -> impl Iterator<Item = Option<SeedClass>> + '_ {
        self.seeds.iter().map(|v| match v {
            1 => Some(SeedClass::Include),
            2 => Some(SeedClass::Exclude),
            _ => None,
        })
    }
    /// Worker-only. Match `base()` to the current object and revision before publishing.
    pub fn apply(&self, cancel: impl Fn() -> bool) -> Result<Labels, &'static str> {
        let mut result = self.base.try_clone(&cancel)?;
        result.begin_edit();
        let [source_width, source_height, _] = result.source.dimensions();
        let [width, height] = self.request.size;
        for y in 0..height {
            let mut x = 0;
            while x < width {
                if cancel() {
                    return Err("Cancelled");
                }
                let index = (self.request.slice * source_height + self.request.origin[1] + y)
                    * source_width
                    + self.request.origin[0]
                    + x;
                let current = result
                    .values
                    .run(index)
                    .ok_or("Label tile is inconsistent")?;
                let count = current.len().min(width - x);
                if count == 0 {
                    return Err("Label tile is inconsistent");
                }
                let mut row = [0u8; 64];
                row[..count].copy_from_slice(&current[..count]);
                for (value, class) in row[..count]
                    .iter_mut()
                    .zip(&self.classes[y * width + x..y * width + x + count])
                {
                    match *class {
                        1 if *value == 0 => *value = self.request.label,
                        2 if *value == self.request.label => *value = 0,
                        _ => (),
                    }
                }
                result.set_values(index, &row[..count])?;
                x += count;
            }
        }
        if cancel() {
            return Err("Cancelled");
        }
        Ok(result)
    }
}
#[derive(Clone, Copy, Default)]
struct CellState {
    strength: f32,
    class: u8,
}
fn allocate<T: Clone>(count: usize, value: T) -> Result<Vec<T>, &'static str> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(count)
        .map_err(|_| "Not enough memory for seeded region")?;
    if values.capacity() != count {
        return Err("Not enough memory for seeded region");
    }
    values.resize(count, value);
    Ok(values)
}
impl Labels {
    /// Compute a two-class proposal on an acquired scalar slice. Four-neighbor
    /// synchronous updates use 1-|a-b|/(valid ROI maximum-minimum) affinity;
    /// a constant ROI has affinity 1. Seeds are fixed, strength starts at 1,
    /// zero-strength attacks cannot label a cell. Equal strength preserves the
    /// existing class; equally strong improvements prefer Exclude. No display-window
    /// parameters enter this calculation.
    pub fn seeded_region(
        self: &Arc<Self>,
        request: SeededRequest,
        seeds: &[SeedPoint],
        cancel: impl Fn() -> bool,
    ) -> Result<SeededRegion, &'static str> {
        self.seeded_with_budget(request, seeds, &cancel, &MemoryBudget::volumes())
    }
    fn seeded_with_budget(
        self: &Arc<Self>,
        request: SeededRequest,
        seeds: &[SeedPoint],
        cancel: &dyn Fn() -> bool,
        budget: &Arc<MemoryBudget>,
    ) -> Result<SeededRegion, &'static str> {
        if cancel() {
            return Err("Cancelled");
        }
        let [source_width, source_height, depth] = self.source.dimensions();
        let [width, height] = request.size;
        let count = width
            .checked_mul(height)
            .filter(|v| *v > 0 && *v <= MAX_CELLS)
            .ok_or("Seeded regions are limited to one million cells")?;
        if request.slice >= depth
            || request.label == 0
            || request.max_iterations == 0
            || request.max_iterations > MAX_ITERATIONS
            || request.origin[0]
                .checked_add(width)
                .is_none_or(|v| v > source_width)
            || request.origin[1]
                .checked_add(height)
                .is_none_or(|v| v > source_height)
            || self.source.is_color()
            || seeds.len() > count.saturating_mul(2)
        {
            return Err("Invalid seeded-region scope, label, seeds or iteration limit");
        }
        let frame = &self.source.frames()[request.slice];
        let base_offset = request.slice * source_width * source_height;
        let source_index = |local: usize| {
            (request.origin[1] + local / width) * source_width + request.origin[0] + local % width
        };
        let eligible = |local: usize| {
            let i = source_index(local);
            frame.has_sample(i)
                && self
                    .values
                    .get(base_offset + i)
                    .is_some_and(|v| *v == 0 || *v == request.label)
        };
        let retained = count
            .checked_mul(2)
            .and_then(|v| v.checked_add(std::mem::size_of::<SeededRegion>()))
            .ok_or("Seeded region is too large")?;
        let scratch = count
            .checked_mul(2 * std::mem::size_of::<CellState>())
            .and_then(|v| v.checked_add(2 * std::mem::size_of::<Vec<CellState>>()))
            .ok_or("Seeded region is too large")?;
        let reservation = budget
            .reserve(retained as u64)
            .ok_or("Not enough memory for seeded region")?;
        let _scratch = budget
            .reserve(scratch as u64)
            .ok_or("Not enough memory for seeded region")?;
        let mut seed_map = allocate(count, 0u8)?;
        let mut classes = allocate(count, 0u8)?;
        let mut current = allocate(count, CellState::default())?;
        let mut next = allocate(count, CellState::default())?;
        let mut minimum = f64::INFINITY;
        let mut maximum = f64::NEG_INFINITY;
        for i in 0..count {
            if i.is_multiple_of(256) && cancel() {
                return Err("Cancelled");
            }
            if eligible(i) {
                let v = f64::from(frame.pixels[source_index(i)]);
                minimum = minimum.min(v);
                maximum = maximum.max(v);
            }
        }
        let mut present = [false; 2];
        for (i, seed) in seeds.iter().enumerate() {
            if i.is_multiple_of(256) && cancel() {
                return Err("Cancelled");
            }
            let [x, y] = seed.position;
            if x < request.origin[0]
                || y < request.origin[1]
                || x >= request.origin[0] + width
                || y >= request.origin[1] + height
            {
                return Err("A seed is outside the selected rectangle");
            }
            let local = (y - request.origin[1]) * width + x - request.origin[0];
            if !eligible(local) {
                return Err("A seed is missing or protected by another label");
            }
            let code = seed.class.code();
            if seed_map[local] != 0 && seed_map[local] != code {
                return Err("Include and exclude seeds overlap");
            }
            seed_map[local] = code;
            current[local] = CellState {
                strength: 1.,
                class: code,
            };
            present[usize::from(code - 1)] = true;
        }
        if !present.into_iter().all(|v| v) {
            return Err("Provide include and exclude seeds");
        }
        let range = maximum - minimum;
        let mut summary = SeededSummary::default();
        let mut work = 0usize;
        let mut converged = false;
        for iteration in 0..request.max_iterations {
            work = work
                .checked_add(count)
                .filter(|v| *v <= MAX_WORK)
                .ok_or("Seeded region exceeded its work limit before convergence")?;
            let mut changed = false;
            for i in 0..count {
                if i.is_multiple_of(256) && cancel() {
                    return Err("Cancelled");
                }
                let mut best = current[i];
                if seed_map[i] == 0 && eligible(i) {
                    let x = i % width;
                    let y = i / width;
                    let neighbors = [
                        (x > 0).then(|| i - 1),
                        (x + 1 < width).then_some(i + 1),
                        (y > 0).then(|| i - width),
                        (y + 1 < height).then_some(i + width),
                    ];
                    let value = f64::from(frame.pixels[source_index(i)]);
                    for n in neighbors.into_iter().flatten() {
                        let other = current[n];
                        if other.class == 0 {
                            continue;
                        }
                        let delta = (value - f64::from(frame.pixels[source_index(n)])).abs();
                        let affinity = if range == 0. {
                            1.
                        } else {
                            (1. - delta / range).clamp(0., 1.)
                        };
                        let attack = (f64::from(other.strength) * affinity) as f32;
                        if attack > 0.
                            && (attack > best.strength
                                || (attack == best.strength
                                    && attack > current[i].strength
                                    && other.class > best.class))
                        {
                            best = CellState {
                                strength: attack,
                                class: other.class,
                            };
                        }
                    }
                }
                changed |= best.class != current[i].class || best.strength != current[i].strength;
                next[i] = best;
            }
            std::mem::swap(&mut current, &mut next);
            summary.iterations = iteration + 1;
            if !changed {
                converged = true;
                break;
            }
        }
        if !converged {
            return Err("Seeded region did not converge within its iteration limit");
        }
        for i in 0..count {
            if i.is_multiple_of(256) && cancel() {
                return Err("Cancelled");
            }
            let class = current[i].class;
            classes[i] = class;
            let label = self
                .values
                .get(base_offset + source_index(i))
                .copied()
                .ok_or("Label tile is inconsistent")?;
            match class {
                1 => {
                    summary.included += 1;
                    summary.added += usize::from(label == 0);
                }
                2 => {
                    summary.excluded += 1;
                    summary.removed += usize::from(label == request.label);
                }
                _ => summary.unresolved += 1,
            }
        }
        if cancel() {
            return Err("Cancelled");
        }
        Ok(SeededRegion {
            base: Arc::clone(self),
            request,
            seeds: seed_map,
            classes,
            summary,
            _reservation: reservation,
        })
    }
}
#[cfg(test)]
mod tests;
