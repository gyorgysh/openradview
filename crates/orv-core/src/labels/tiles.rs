// SPDX-License-Identifier: GPL-3.0-or-later
//! Bounded copy-on-write label tiles. Missing source samples are handled separately.
use crate::resources::{MemoryBudget, Reservation};
use std::{ops::Range, sync::Arc};

pub const LABEL_TILE_SIDE: usize = 32;
static ZERO_ROW: [u8; 64] = [0; 64];

pub use crate::mask_identity::{MaskObjectId as LabelObjectId, MaskRevision as LabelRevision};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LabelTile {
    pub coordinate: [usize; 3],
    /// Source-cell minimum and exclusive maximum, clipped at the grid edge.
    pub bounds: [[usize; 3]; 2],
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LabelStorageStatistics {
    pub tile_side: usize,
    pub tiles: usize,
    pub allocated_tiles: usize,
    pub directory_bytes: u64,
    /// Referenced tile allocations; shared tiles must not be summed across revisions.
    pub referenced_tile_bytes: u64,
}
#[derive(Clone, Copy)]
struct Layout {
    dimensions: [usize; 3],
    grid: [usize; 3],
    side: usize,
    count: usize,
    tiles: usize,
}
impl Layout {
    fn new(dimensions: [usize; 3], side: usize) -> Result<Self, &'static str> {
        if ![32, 64].contains(&side) || dimensions.contains(&0) {
            return Err("Invalid label tile layout");
        }
        let count = super::Labels::check_dimensions(dimensions)?;
        let grid = dimensions.map(|n| n.div_ceil(side));
        let tiles = grid
            .into_iter()
            .try_fold(1usize, usize::checked_mul)
            .ok_or("Label tile count overflow")?;
        Ok(Self {
            dimensions,
            grid,
            side,
            count,
            tiles,
        })
    }
    fn tile(&self, index: usize) -> Option<LabelTile> {
        if index >= self.tiles {
            return None;
        }
        let coordinate = [
            index % self.grid[0],
            index / self.grid[0] % self.grid[1],
            index / (self.grid[0] * self.grid[1]),
        ];
        let minimum = coordinate.map(|n| n * self.side);
        let maximum = std::array::from_fn(|a| (minimum[a] + self.side).min(self.dimensions[a]));
        Some(LabelTile {
            coordinate,
            bounds: [minimum, maximum],
        })
    }
    fn address(&self, index: usize) -> Option<(usize, usize, usize)> {
        if index >= self.count {
            return None;
        }
        let [w, h, _] = self.dimensions;
        let [x, y, z] = [index % w, index / w % h, index / (w * h)];
        let cell = [x / self.side, y / self.side, z / self.side];
        let tile = (cell[2] * self.grid[1] + cell[1]) * self.grid[0] + cell[0];
        let width = (w - cell[0] * self.side).min(self.side);
        let height = (h - cell[1] * self.side).min(self.side);
        let local = (z % self.side * height + y % self.side) * width + x % self.side;
        let run = (width - x % self.side).min(w - x);
        Some((tile, local, run))
    }
}
struct Tile {
    values: Vec<u8>,
    nonzero: usize,
    bytes: u64,
    _reservation: Reservation,
}
impl Tile {
    fn copy(
        source: Option<&Self>,
        count: usize,
        budget: &Arc<MemoryBudget>,
    ) -> Result<Self, &'static str> {
        let bytes =
            count as u64 + (std::mem::size_of::<Self>() + 2 * std::mem::size_of::<usize>()) as u64;
        let reservation = budget
            .reserve(bytes)
            .ok_or("Not enough memory for a changed label tile")?;
        let mut values = Vec::new();
        values
            .try_reserve_exact(count)
            .map_err(|_| "Not enough memory for a changed label tile")?;
        if let Some(source) = source {
            values.extend_from_slice(&source.values);
        } else {
            values.resize(count, 0);
        }
        Ok(Self {
            values,
            nonzero: source.map_or(0, |s| s.nonzero),
            bytes,
            _reservation: reservation,
        })
    }
}
pub(super) struct Store {
    layout: Layout,
    pages: Vec<Option<Arc<Tile>>>,
    dirty: Vec<bool>,
    changed: usize,
    budget: Arc<MemoryBudget>,
    directory_bytes: u64,
    _directory: Reservation,
}
impl Store {
    pub fn empty(
        dimensions: [usize; 3],
        side: usize,
        budget: Arc<MemoryBudget>,
    ) -> Result<Self, &'static str> {
        let layout = Layout::new(dimensions, side)?;
        let directory_bytes = (layout.tiles
            * (std::mem::size_of::<Option<Arc<Tile>>>() + std::mem::size_of::<bool>())
            + std::mem::size_of::<super::Labels>()
            + 2 * std::mem::size_of::<usize>()) as u64;
        let reservation = budget
            .reserve(directory_bytes)
            .ok_or("Not enough memory for the label tile directory")?;
        let mut pages = Vec::new();
        pages
            .try_reserve_exact(layout.tiles)
            .map_err(|_| "Not enough memory for the label tile directory")?;
        pages.resize_with(layout.tiles, || None);
        let mut dirty = Vec::new();
        dirty
            .try_reserve_exact(layout.tiles)
            .map_err(|_| "Not enough memory for label changes")?;
        dirty.resize(layout.tiles, false);
        Ok(Self {
            layout,
            pages,
            dirty,
            changed: 0,
            budget,
            directory_bytes,
            _directory: reservation,
        })
    }
    pub fn try_clone(&self, cancel: impl Fn() -> bool) -> Result<Self, &'static str> {
        let mut result = Self::empty(
            self.layout.dimensions,
            self.layout.side,
            self.budget.clone(),
        )?;
        for (index, (target, source)) in result.pages.iter_mut().zip(&self.pages).enumerate() {
            if index.is_multiple_of(128) && cancel() {
                return Err("Cancelled");
            }
            *target = source.clone();
        }
        if cancel() {
            return Err("Cancelled");
        }
        Ok(result)
    }
    pub fn budget(&self) -> &Arc<MemoryBudget> {
        &self.budget
    }
    pub fn len(&self) -> usize {
        self.layout.count
    }
    pub fn get(&self, index: usize) -> Option<&u8> {
        let (tile, local, _) = self.layout.address(index)?;
        match self.pages.get(tile)? {
            Some(page) => page.values.get(local),
            None => Some(&ZERO_ROW[0]),
        }
    }
    #[cfg(test)]
    pub fn set(&mut self, index: usize, value: u8) -> Result<(), &'static str> {
        self.write_run(index, &[value])
    }
    pub fn write_run(&mut self, index: usize, values: &[u8]) -> Result<(), &'static str> {
        if values.is_empty() {
            return Ok(());
        }
        let (tile, local, capacity) = self
            .layout
            .address(index)
            .ok_or("Label index is outside the source")?;
        if values.len() > capacity {
            return Err("Label write crosses a tile row");
        }
        let previous = self
            .run(index)
            .and_then(|run| run.get(..values.len()))
            .ok_or("Label tile is inconsistent")?;
        if previous == values {
            return Ok(());
        }
        let previous_nonzero = previous.iter().filter(|v| **v != 0).count();
        let next_nonzero = values.iter().filter(|v| **v != 0).count();
        let unique = self
            .pages
            .get_mut(tile)
            .and_then(Option::as_mut)
            .and_then(Arc::get_mut)
            .is_some();
        if !unique {
            let bounds = self
                .layout
                .tile(tile)
                .ok_or("Label tile is outside the source")?
                .bounds;
            let count = (0..3).map(|a| bounds[1][a] - bounds[0][a]).product();
            let copy = Tile::copy(self.pages[tile].as_deref(), count, &self.budget)?;
            self.pages[tile] = Some(Arc::new(copy));
        }
        let page = self
            .pages
            .get_mut(tile)
            .and_then(Option::as_mut)
            .and_then(Arc::get_mut)
            .ok_or("Label tile is not privately editable")?;
        let target = page
            .values
            .get_mut(local..local + values.len())
            .ok_or("Label tile is inconsistent")?;
        target.copy_from_slice(values);
        page.nonzero = page.nonzero - previous_nonzero + next_nonzero;
        if page.nonzero == 0 {
            self.pages[tile] = None;
        }
        if !self.dirty[tile] {
            self.dirty[tile] = true;
            self.changed += 1;
        }
        Ok(())
    }
    pub fn reset_changes(&mut self) {
        self.dirty.fill(false);
        self.changed = 0;
    }
    pub fn has_changes(&self) -> bool {
        self.changed != 0
    }
    pub fn changed_tiles(&self) -> impl Iterator<Item = LabelTile> + '_ {
        self.dirty
            .iter()
            .enumerate()
            .filter_map(|(i, changed)| changed.then(|| self.layout.tile(i)).flatten())
    }
    pub fn tiles(&self) -> impl Iterator<Item = LabelTile> + '_ {
        (0..self.layout.tiles).filter_map(|index| self.layout.tile(index))
    }
    pub fn copy_tile(&self, tile: LabelTile, output: &mut [u8]) -> Result<(), &'static str> {
        if tile
            .coordinate
            .iter()
            .zip(self.layout.grid)
            .any(|(a, b)| *a >= b)
        {
            return Err("Label tile is outside the source");
        }
        let [x, y, z] = tile.coordinate;
        let index = (z * self.layout.grid[1] + y) * self.layout.grid[0] + x;
        if self.layout.tile(index) != Some(tile) {
            return Err("Label tile bounds differ from the source layout");
        }
        let [minimum, maximum] = tile.bounds;
        let count: usize = (0..3).map(|a| maximum[a] - minimum[a]).product();
        if output.len() != count {
            return Err("Label tile output length is incorrect");
        }
        let width = maximum[0] - minimum[0];
        let mut offset = 0;
        for z in minimum[2]..maximum[2] {
            for y in minimum[1]..maximum[1] {
                let start =
                    (z * self.layout.dimensions[1] + y) * self.layout.dimensions[0] + minimum[0];
                self.copy_into(start, &mut output[offset..offset + width])?;
                offset += width;
            }
        }
        Ok(())
    }
    pub fn changed_since<'a>(
        &'a self,
        previous: &'a Self,
    ) -> Result<impl Iterator<Item = LabelTile> + 'a, &'static str> {
        if self.layout.dimensions != previous.layout.dimensions
            || self.layout.side != previous.layout.side
        {
            return Err("Label tile layouts differ");
        }
        Ok(self
            .pages
            .iter()
            .zip(&previous.pages)
            .enumerate()
            .filter_map(|(index, (current, old))| {
                let changed = match (current, old) {
                    (None, None) => false,
                    (Some(a), Some(b)) => !Arc::ptr_eq(a, b),
                    _ => true,
                };
                changed.then(|| self.layout.tile(index)).flatten()
            }))
    }
    pub(super) fn run(&self, index: usize) -> Option<&[u8]> {
        let (tile, local, count) = self.layout.address(index)?;
        match self.pages.get(tile)? {
            Some(page) => page.values.get(local..local + count),
            None => ZERO_ROW.get(..count),
        }
    }
    pub fn range(&self, range: Range<usize>) -> Option<Values<'_>> {
        (range.start <= range.end && range.end <= self.len())
            .then_some(Values { store: self, range })
    }
    pub fn view(&self) -> Values<'_> {
        Values {
            store: self,
            range: 0..self.len(),
        }
    }
    pub fn iter(&self) -> ValueIter<'_> {
        Values {
            store: self,
            range: 0..self.len(),
        }
        .iter()
    }
    pub fn copy_into(&self, start: usize, output: &mut [u8]) -> Result<(), &'static str> {
        if start
            .checked_add(output.len())
            .is_none_or(|end| end > self.len())
        {
            return Err("Label range is outside the source");
        }
        let mut written = 0;
        while written < output.len() {
            let run = self
                .run(start + written)
                .ok_or("Label tile is inconsistent")?;
            let count = run.len().min(output.len() - written);
            output[written..written + count].copy_from_slice(&run[..count]);
            written += count;
        }
        Ok(())
    }
    pub fn statistics(&self) -> LabelStorageStatistics {
        LabelStorageStatistics {
            tile_side: self.layout.side,
            tiles: self.layout.tiles,
            allocated_tiles: self.pages.iter().flatten().count(),
            directory_bytes: self.directory_bytes,
            referenced_tile_bytes: self.pages.iter().flatten().map(|p| p.bytes).sum(),
        }
    }
}
/// Read-only logical x-fastest values without flattening a second volume.
#[derive(Clone)]
pub struct Values<'a> {
    store: &'a Store,
    range: Range<usize>,
}
impl<'a> Values<'a> {
    pub fn len(&self) -> usize {
        self.range.len()
    }
    pub fn is_empty(&self) -> bool {
        self.range.is_empty()
    }
    pub fn get(&self, index: usize) -> Option<&'a u8> {
        (index < self.len())
            .then(|| self.store.get(self.range.start + index))
            .flatten()
    }
    pub fn iter(&self) -> ValueIter<'a> {
        ValueIter {
            store: self.store,
            next: self.range.start,
            end: self.range.end,
            remaining: self.len(),
            run: [].iter(),
        }
    }
}
pub struct ValueIter<'a> {
    store: &'a Store,
    next: usize,
    end: usize,
    remaining: usize,
    run: std::slice::Iter<'a, u8>,
}
impl<'a> Iterator for ValueIter<'a> {
    type Item = &'a u8;
    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }
        if self.run.len() == 0 {
            let run = self.store.run(self.next)?;
            let count = run.len().min(self.end - self.next);
            self.run = run[..count].iter();
            self.next += count;
        }
        self.remaining -= 1;
        self.run.next()
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}
impl ExactSizeIterator for ValueIter<'_> {}
impl std::iter::FusedIterator for ValueIter<'_> {}

#[cfg(test)]
#[path = "tiles_tests.rs"]
mod tests;
