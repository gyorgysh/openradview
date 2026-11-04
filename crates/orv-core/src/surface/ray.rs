// SPDX-License-Identifier: GPL-3.0-or-later
//! Bounded membership reference for known interfaces, not closed-mask first cells.
use super::{CellFace, FaceBoundary, FaceSide};
use crate::{
    mask::{CellRay, MaskKey, MaskRevision, MaskView},
    vector::*,
    volume_view::PhysicalClip,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InterfaceRayError {
    Geometry(&'static str),
    Cancelled,
    Ambiguous,
}
impl std::fmt::Display for InterfaceRayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Geometry(message) => message,
            Self::Cancelled => "Cancelled",
            Self::Ambiguous => "This ray meets an ambiguous surface edge or tangent contact",
        })
    }
}
impl std::error::Error for InterfaceRayError {}

/// Source-membership geometry only. The caller must retain/revalidate the exact
/// source/map and complete current scene, including foreground visibility.
#[derive(Clone, Debug, PartialEq)]
pub struct InterfaceHit {
    pub key: MaskKey,
    pub revision: MaskRevision,
    pub face: CellFace,
    /// Signed physical parameter from the supplied original ray origin.
    pub parameter_mm: f64,
    pub position_mm: V3,
    /// Positive-length occupied cell interval containing this interface.
    pub cell_range_mm: [f64; 2],
}
pub struct InterfaceRay<'a> {
    view: MaskView<'a>,
    cells: CellRay,
    origin_mm: V3,
    direction_unit: V3,
    origin_voxel: V3,
    direction_voxel: V3,
}
impl<'a> InterfaceRay<'a> {
    pub fn new(
        view: MaskView<'a>,
        origin_mm: V3,
        direction_unit: V3,
        range_mm: [f64; 2],
        clip: Option<&PhysicalClip>,
    ) -> Result<Self, InterfaceRayError> {
        let source = view.source();
        let cells = CellRay::new(source, origin_mm, direction_unit, range_mm, clip)
            .map_err(InterfaceRayError::Geometry)?;
        Ok(Self {
            view,
            cells,
            origin_mm,
            direction_unit,
            origin_voxel: source.world_to_voxel(origin_mm),
            direction_voxel: source.world_vector_to_voxel(direction_unit),
        })
    }
    /// Consumes this bounded traversal. Clip/range entries inside a cell do not
    /// become faces. Exact quad-edge/corner or tangent contacts refuse a pick;
    /// nearby interior contacts use their actual f64 parameters, without nudges.
    pub fn first_hit(
        mut self,
        cancelled: impl Fn() -> bool,
    ) -> Result<Option<InterfaceHit>, InterfaceRayError> {
        loop {
            if cancelled() {
                return Err(InterfaceRayError::Cancelled);
            }
            let Some(interval) = self
                .cells
                .next_cell()
                .map_err(InterfaceRayError::Geometry)?
            else {
                return Ok(None);
            };
            let membership =
                self.view
                    .sample(interval.voxel)
                    .ok_or(InterfaceRayError::Geometry(
                        "Surface ray and source grid differ",
                    ))?;
            if !membership.eligible() {
                continue;
            }
            for parameter_mm in interval.range_mm {
                for side in FaceSide::ALL {
                    let axis = side.axis();
                    let direction = self.direction_voxel[axis];
                    if direction == 0. {
                        continue;
                    }
                    let plane =
                        interval.voxel[axis] as f64 + if side.positive() { 0.5 } else { -0.5 };
                    if (plane - self.origin_voxel[axis]) / direction != parameter_mm {
                        continue;
                    }
                    let Some(boundary) = self.boundary(interval.voxel, side)? else {
                        continue;
                    };
                    if self.edge_contact(interval.voxel, axis, parameter_mm) {
                        return Err(InterfaceRayError::Ambiguous);
                    }
                    let position_mm = std::array::from_fn(|a| {
                        self.direction_unit[a].mul_add(parameter_mm, self.origin_mm[a])
                    });
                    if !finite(position_mm) {
                        return Err(InterfaceRayError::Geometry(
                            "Surface hit position is not finite",
                        ));
                    }
                    if interval.voxel.iter().any(|v| *v > u32::MAX as usize) {
                        return Err(InterfaceRayError::Geometry(
                            "Surface cell coordinates exceed supported bounds",
                        ));
                    }
                    return Ok(Some(InterfaceHit {
                        key: self.view.key(),
                        revision: self.view.revision().clone(),
                        face: CellFace {
                            cell: interval.voxel.map(|v| v as u32),
                            side,
                            boundary,
                        },
                        parameter_mm,
                        position_mm,
                        cell_range_mm: interval.range_mm,
                    }));
                }
            }
        }
    }
    fn boundary(
        &self,
        voxel: [usize; 3],
        side: FaceSide,
    ) -> Result<Option<FaceBoundary>, InterfaceRayError> {
        let axis = side.axis();
        let dimensions = self.view.source().dimensions();
        if (!side.positive() && voxel[axis] == 0)
            || (side.positive() && voxel[axis] + 1 == dimensions[axis])
        {
            return Ok(Some(FaceBoundary::Grid));
        }
        let mut neighbor = voxel;
        if side.positive() {
            neighbor[axis] += 1;
        } else {
            neighbor[axis] -= 1;
        }
        let sample = self
            .view
            .sample(neighbor)
            .ok_or(InterfaceRayError::Geometry(
                "Surface neighbor is outside the source",
            ))?;
        Ok((sample.known && sample.source_valid && !sample.present).then_some(FaceBoundary::Mask))
    }
    fn edge_contact(&self, voxel: [usize; 3], face_axis: usize, parameter: f64) -> bool {
        (0..3).filter(|a| *a != face_axis).any(|a| {
            [-0.5, 0.5].into_iter().any(|offset| {
                let plane = voxel[a] as f64 + offset;
                if self.direction_voxel[a] == 0. {
                    self.origin_voxel[a] == plane
                } else {
                    (plane - self.origin_voxel[a]) / self.direction_voxel[a] == parameter
                }
            })
        })
    }
}
