// SPDX-License-Identifier: GPL-3.0-or-later
//! Worker-owned local mosaics. Each rendered tile retains the ordinary slice cap.
use orv_core::resources::{MemoryBudget, Reservation};
use serde::Serialize;
use std::sync::Arc;

const MAX_SIDE: u32 = 2048;
const MAX_TILES: usize = 16;
const GAP: u32 = 8;
const OVERHEAD: u64 = 16 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Tile {
    pub origin: [u32; 2],
    pub size: [u32; 2],
}

/// Validated fixed cells with centred, aspect-preserving tiles. Dimensions are
/// rounded down to whole pixels; a very thin axis still occupies one pixel.
#[derive(Debug, Serialize)]
pub struct Layout {
    size: [u32; 2],
    columns: u32,
    tiles: Vec<Tile>,
}
impl Layout {
    pub fn new(sizes: &[[u32; 2]], columns: u32) -> Result<Self, &'static str> {
        if sizes.is_empty()
            || sizes.len() > MAX_TILES
            || !(1..=4).contains(&columns)
            || columns as usize > sizes.len()
            || sizes.iter().flatten().any(|n| !(1..=1024).contains(n))
        {
            return Err("Local mosaics need 1–16 bounded tiles and 1–4 occupied columns");
        }
        let rows = (sizes.len() as u32).div_ceil(columns);
        let maximum = sizes
            .iter()
            .fold([0u32; 2], |a, b| [a[0].max(b[0]), a[1].max(b[1])]);
        let limit = [
            (MAX_SIDE - GAP * (columns - 1)) / columns,
            (MAX_SIDE - GAP * (rows - 1)) / rows,
        ];
        let scale = (f64::from(limit[0]) / f64::from(maximum[0]))
            .min(f64::from(limit[1]) / f64::from(maximum[1]))
            .min(1.);
        let cell = maximum.map(|n| (f64::from(n) * scale).floor().max(1.) as u32);
        let size = [
            columns * cell[0] + (columns - 1) * GAP,
            rows * cell[1] + (rows - 1) * GAP,
        ];
        let mut tiles = Vec::new();
        tiles
            .try_reserve_exact(sizes.len())
            .map_err(|_| "Not enough memory for mosaic layout")?;
        for (index, original) in sizes.iter().enumerate() {
            let tile_size = original.map(|n| (f64::from(n) * scale).floor().max(1.) as u32);
            tiles.push(Tile {
                origin: [
                    index as u32 % columns * (cell[0] + GAP) + (cell[0] - tile_size[0]) / 2,
                    index as u32 / columns * (cell[1] + GAP) + (cell[1] - tile_size[1]) / 2,
                ],
                size: tile_size,
            });
        }
        Ok(Self {
            size,
            columns,
            tiles,
        })
    }
    pub fn size(&self) -> [u32; 2] {
        self.size
    }
    pub fn tiles(&self) -> &[Tile] {
        &self.tiles
    }
    pub fn rgba_bytes(&self) -> usize {
        self.size[0] as usize * self.size[1] as usize * 4
    }
    /// The caller retaining encoded output must admit this allowance separately.
    pub fn png_limit(&self) -> usize {
        self.rgba_bytes() * 2 + 1024 * 1024
    }
}

/// Holds one admitted CPU image. Copy and release one finished GPU tile at a
/// time; no source frames, network authority or snapshot manifest are retained.
pub struct Mosaic {
    layout: Layout,
    rgba: Vec<u8>,
    completed: u16,
    _memory: Reservation,
}
impl Mosaic {
    pub fn new(layout: Layout, cancelled: impl Fn() -> bool) -> Result<Self, &'static str> {
        Self::with_budget(layout, &MemoryBudget::volumes(), cancelled)
    }
    fn with_budget(
        layout: Layout,
        budget: &Arc<MemoryBudget>,
        cancelled: impl Fn() -> bool,
    ) -> Result<Self, &'static str> {
        if cancelled() {
            return Err("Cancelled");
        }
        let bytes = layout.rgba_bytes();
        let memory = budget
            .reserve(bytes as u64 + OVERHEAD)
            .ok_or("Not enough memory for a local mosaic")?;
        let mut rgba = Vec::new();
        rgba.try_reserve_exact(bytes)
            .map_err(|_| "Not enough memory for mosaic pixels")?;
        let mut row = [10u8; MAX_SIDE as usize * 4];
        for pixel in row.as_chunks_mut::<4>().0 {
            pixel[3] = 255;
        }
        for _ in 0..layout.size[1] {
            if cancelled() {
                return Err("Cancelled");
            }
            rgba.extend_from_slice(&row[..layout.size[0] as usize * 4]);
        }
        Ok(Self {
            layout,
            rgba,
            completed: 0,
            _memory: memory,
        })
    }
    pub fn layout(&self) -> &Layout {
        &self.layout
    }
    pub fn rgba(&self) -> &[u8] {
        &self.rgba
    }
    /// A cancelled partial copy leaves this tile incomplete; a retry overwrites
    /// its whole rectangle. No partial or duplicated tile can produce a PNG.
    pub fn copy_tile(
        &mut self,
        index: usize,
        pixels: &[u8],
        cancelled: impl Fn() -> bool,
    ) -> Result<(), &'static str> {
        let tile = self.layout.tiles.get(index).ok_or("Unknown mosaic tile")?;
        let bit = 1u16 << index;
        let stride = tile.size[0] as usize * 4;
        if self.completed & bit != 0 || pixels.len() != stride * tile.size[1] as usize {
            return Err("Mosaic tile is duplicated or has different dimensions");
        }
        let destination_stride = self.layout.size[0] as usize * 4;
        for (row, source) in pixels.chunks_exact(stride).enumerate() {
            if cancelled() {
                return Err("Cancelled");
            }
            let offset =
                (tile.origin[1] as usize + row) * destination_stride + tile.origin[0] as usize * 4;
            self.rgba[offset..offset + stride].copy_from_slice(source);
        }
        if cancelled() {
            return Err("Cancelled");
        }
        self.completed |= bit;
        Ok(())
    }
    /// Encodes only a complete composition. Returned PNG bytes need their own
    /// retained-output admission; encoder scratch is admitted during this call.
    pub fn png(&self, cancelled: impl Fn() -> bool) -> Result<Vec<u8>, &'static str> {
        let expected = ((1u32 << self.layout.tiles.len()) - 1) as u16;
        if self.completed != expected {
            return Err("The local mosaic is incomplete");
        }
        super::readback::encode_mosaic(&self.rgba, self.layout.size, &cancelled)
    }
}

#[cfg(test)]
mod tests;
