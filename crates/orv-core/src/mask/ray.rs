// SPDX-License-Identifier: GPL-3.0-or-later
//! Ordered intervals through affine nearest-neighbor source cells.
use super::{MaskKey, MaskRevision, MaskView, Membership};
use crate::{vector::*, volume::Volume, volume_view::PhysicalClip};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CellInterval {
    pub voxel: [usize; 3],
    /// Physical ray parameters, ordered near to far. Only positive lengths are emitted.
    pub range_mm: [f64; 2],
}
/// Allocation-free geometry reference, independent of intensity sampling steps.
/// Cells cover [-0.5, dimension-0.5), with half ties toward the larger index.
/// At a crossing, the interval names the cell's interior; zero-length edge/corner
/// contacts do not create an additional cell. All tied axes advance together.
pub struct CellRay {
    dimensions: [usize; 3],
    origin: V3,
    direction: V3,
    cell: [i64; 3],
    cursor: f64,
    end: f64,
    remaining: usize,
}
fn intersect(
    origin: V3,
    direction: V3,
    minimum: V3,
    maximum: V3,
    upper_exclusive: bool,
    range: &mut [f64; 2],
) -> bool {
    for a in 0..3 {
        if direction[a] == 0. {
            if origin[a] < minimum[a]
                || origin[a] > maximum[a]
                || (upper_exclusive && origin[a] == maximum[a])
            {
                return false;
            }
        } else {
            let p = (minimum[a] - origin[a]) / direction[a];
            let q = (maximum[a] - origin[a]) / direction[a];
            range[0] = range[0].max(p.min(q));
            range[1] = range[1].min(p.max(q));
            if range[0] >= range[1] {
                return false;
            }
        }
    }
    range[0] < range[1]
}
impl CellRay {
    /// The caller supplies the physical interval to inspect; negative parameters
    /// are useful for an orthographic camera whose origin lies inside the source.
    pub fn new(
        source: &Volume,
        origin_mm: V3,
        direction_unit: V3,
        range_mm: [f64; 2],
        clip: Option<&PhysicalClip>,
    ) -> Result<Self, &'static str> {
        if !finite(origin_mm)
            || !finite(direction_unit)
            || (norm(direction_unit) - 1.).abs() > 1e-6
            || range_mm.iter().any(|v| !v.is_finite())
            || range_mm[0] >= range_mm[1]
        {
            return Err("Invalid physical mask ray or interval");
        }
        if let Some(clip) = clip {
            clip.validate()?;
        }
        let dimensions = source.dimensions();
        let origin = source.world_to_voxel(origin_mm);
        let direction = source.world_vector_to_voxel(direction_unit);
        if !finite(origin) || !finite(direction) || direction == [0.; 3] {
            return Err("Mask ray coordinates exceed the supported range");
        }
        let mut range = range_mm;
        let active = intersect(
            origin,
            direction,
            [-0.5; 3],
            dimensions.map(|d| d as f64 - 0.5),
            true,
            &mut range,
        ) && clip.is_none_or(|clip| {
            intersect(
                origin_mm,
                direction_unit,
                clip.minimum_mm,
                clip.maximum_mm,
                false,
                &mut range,
            )
        });
        let mut result = Self {
            dimensions,
            origin,
            direction,
            cell: [0; 3],
            cursor: range[0],
            end: if active { range[1] } else { range[0] },
            remaining: if active {
                dimensions
                    .into_iter()
                    .try_fold(3usize, usize::checked_add)
                    .ok_or("Mask traversal limit overflow")?
            } else {
                0
            },
        };
        if !active {
            return Ok(result);
        }
        for a in 0..3 {
            let at_entry = direction[a].mul_add(range[0], origin[a]);
            if !at_entry.is_finite() {
                return Err("Mask ray entry is not finite");
            }
            result.cell[a] = (at_entry + 0.5)
                .floor()
                .clamp(0., (dimensions[a] - 1) as f64) as i64;
            if direction[a] == 0. {
                continue;
            }
            // Compare boundary parameters, rather than nudging a physical point
            // with an epsilon that could erase a thin cell or change tie rules.
            for _ in 0..3 {
                let low = (result.cell[a] as f64 - 0.5 - origin[a]) / direction[a];
                let high = (result.cell[a] as f64 + 0.5 - origin[a]) / direction[a];
                if range[0] < low.min(high) {
                    result.cell[a] -= direction[a].signum() as i64;
                } else if range[0] >= low.max(high) {
                    result.cell[a] += direction[a].signum() as i64;
                } else {
                    break;
                }
            }
            if result.cell[a] < 0 || result.cell[a] >= dimensions[a] as i64 {
                return Err("Mask ray entry cannot resolve a source cell");
            }
            let low = (result.cell[a] as f64 - 0.5 - origin[a]) / direction[a];
            let high = (result.cell[a] as f64 + 0.5 - origin[a]) / direction[a];
            if range[0] < low.min(high) || range[0] >= low.max(high) {
                return Err("Mask ray entry exceeds numerical resolution");
            }
        }
        Ok(result)
    }
    pub fn next_cell(&mut self) -> Result<Option<CellInterval>, &'static str> {
        while self.remaining > 0 && self.cursor < self.end {
            self.remaining -= 1;
            if (0..3).any(|a| self.cell[a] < 0 || self.cell[a] >= self.dimensions[a] as i64) {
                // This is the same boundary division used to intersect the source.
                return Err("Mask traversal left the source before its exit");
            }
            let voxel = self.cell.map(|v| v as usize);
            let crossing: V3 = std::array::from_fn(|a| {
                if self.direction[a] == 0. {
                    f64::INFINITY
                } else {
                    (self.cell[a] as f64 + self.direction[a].signum() * 0.5 - self.origin[a])
                        / self.direction[a]
                }
            });
            let next = crossing.into_iter().fold(self.end, f64::min);
            if !next.is_finite() || next < self.cursor {
                return Err("Mask traversal boundary is not ordered");
            }
            let start = self.cursor;
            self.cursor = next;
            for (a, time) in crossing.into_iter().enumerate() {
                if time <= next {
                    self.cell[a] += self.direction[a].signum() as i64;
                }
            }
            if start < next {
                return Ok(Some(CellInterval {
                    voxel,
                    range_mm: [start, next],
                }));
            }
        }
        if self.cursor < self.end && self.remaining == 0 {
            return Err("Mask traversal exceeded its source-cell limit");
        }
        Ok(None)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MaskInterval {
    pub cell: CellInterval,
    pub membership: Membership,
}
#[derive(Clone, Debug, PartialEq)]
pub struct MaskHit {
    pub key: MaskKey,
    pub revision: MaskRevision,
    pub voxel: [usize; 3],
    pub range_mm: [f64; 2],
    pub position_mm: V3,
}
/// Borrowed immutable mask/source identity. GPU/UI callers must also revalidate
/// the completed scene, visible segment keys and revision before applying a pick.
pub struct MaskRay<'a> {
    view: MaskView<'a>,
    cells: CellRay,
    origin_mm: V3,
    direction_unit: V3,
}
impl<'a> MaskView<'a> {
    pub fn ray(
        self,
        origin_mm: V3,
        direction_unit: V3,
        range_mm: [f64; 2],
        clip: Option<&PhysicalClip>,
    ) -> Result<MaskRay<'a>, &'static str> {
        Ok(MaskRay {
            view: self,
            cells: CellRay::new(self.source(), origin_mm, direction_unit, range_mm, clip)?,
            origin_mm,
            direction_unit,
        })
    }
}
impl MaskRay<'_> {
    pub fn next_cell(&mut self) -> Result<Option<MaskInterval>, &'static str> {
        self.cells
            .next_cell()?
            .map(|cell| {
                self.view
                    .sample(cell.voxel)
                    .map(|membership| MaskInterval { cell, membership })
                    .ok_or("Mask traversal and source grid differ")
            })
            .transpose()
    }
    pub fn first_eligible_hit(
        &mut self,
        cancelled: impl Fn() -> bool,
    ) -> Result<Option<MaskHit>, &'static str> {
        loop {
            if cancelled() {
                return Err("Cancelled");
            }
            let Some(interval) = self.next_cell()? else {
                return Ok(None);
            };
            if interval.membership.eligible() {
                let position_mm = std::array::from_fn(|a| {
                    self.direction_unit[a].mul_add(interval.cell.range_mm[0], self.origin_mm[a])
                });
                if !finite(position_mm) {
                    return Err("Mask hit position is not finite");
                }
                return Ok(Some(MaskHit {
                    key: self.view.key(),
                    revision: self.view.revision().clone(),
                    voxel: interval.cell.voxel,
                    range_mm: interval.cell.range_mm,
                    position_mm,
                }));
            }
        }
    }
}
