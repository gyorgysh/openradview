// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

#[derive(Clone, Copy)]
pub(super) struct Layout {
    dimensions: [usize; 3],
    pub grid: [usize; 3],
    pub len: usize,
}
impl Layout {
    pub fn new(dimensions: [usize; 3]) -> Result<Self, SurfaceError> {
        crate::labels::Labels::check_dimensions(dimensions).map_err(|_| SurfaceError::Limit)?;
        let grid = dimensions.map(|d| d.div_ceil(CHUNK_SIDE));
        let len = grid
            .into_iter()
            .try_fold(1usize, usize::checked_mul)
            .filter(|n| *n <= MAX_CHUNKS)
            .ok_or(SurfaceError::Limit)?;
        Ok(Self {
            dimensions,
            grid,
            len,
        })
    }
    pub fn coordinate(&self, index: usize) -> [usize; 3] {
        [
            index % self.grid[0],
            index / self.grid[0] % self.grid[1],
            index / (self.grid[0] * self.grid[1]),
        ]
    }
    pub fn index(&self, coordinate: [usize; 3]) -> Option<usize> {
        coordinate
            .iter()
            .zip(self.grid)
            .all(|(v, d)| *v < d)
            .then(|| (coordinate[2] * self.grid[1] + coordinate[1]) * self.grid[0] + coordinate[0])
    }
    pub fn bounds(&self, index: usize) -> [[usize; 3]; 2] {
        let lo = self.coordinate(index).map(|v| v * CHUNK_SIDE);
        [
            lo,
            std::array::from_fn(|a| (lo[a] + CHUNK_SIDE).min(self.dimensions[a])),
        ]
    }
    pub fn mark_neighbors(
        &self,
        coordinate: [usize; 3],
        dirty: &mut [bool],
    ) -> Result<(), SurfaceError> {
        dirty[self.index(coordinate).ok_or(SurfaceError::Geometry)?] = true;
        for axis in 0..3 {
            for delta in [-1isize, 1] {
                let mut neighbor = coordinate;
                if let Some(n) = coordinate[axis].checked_add_signed(delta) {
                    neighbor[axis] = n;
                    if let Some(index) = self.index(neighbor) {
                        dirty[index] = true;
                    }
                }
            }
        }
        Ok(())
    }
}
