// SPDX-License-Identifier: GPL-3.0-or-later
use super::SurfaceError;
use crate::{vector::*, volume::Volume};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum FaceSide {
    XMinus,
    XPlus,
    YMinus,
    YPlus,
    ZMinus,
    ZPlus,
}
impl FaceSide {
    pub const ALL: [Self; 6] = [
        Self::XMinus,
        Self::XPlus,
        Self::YMinus,
        Self::YPlus,
        Self::ZMinus,
        Self::ZPlus,
    ];
    pub fn axis(self) -> usize {
        self as usize / 2
    }
    pub fn positive(self) -> bool {
        (self as u8) % 2 == 1
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum FaceBoundary {
    Mask,
    Grid,
}
/// Compact quad descriptor. Source-grid coordinates are exact; no shared-edge
/// float rounding or duplicated expanded vertex/index buffers are required.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct CellFace {
    pub(super) cell: [u32; 3],
    pub(super) side: FaceSide,
    pub(super) boundary: FaceBoundary,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FaceGeometry {
    /// Wound outward in physical coordinates, including reflected/sheared grids.
    pub corners_mm: [[f64; 3]; 4],
    pub normal: [f64; 3],
    pub area_mm2: f64,
}
impl CellFace {
    pub fn cell(&self) -> [u32; 3] {
        self.cell
    }
    pub fn side(&self) -> FaceSide {
        self.side
    }
    pub fn boundary(&self) -> FaceBoundary {
        self.boundary
    }
    /// Doubled source coordinates: shared corners have the same integer key,
    /// including the outer half-cell face at -1. Winding here is in index space.
    pub fn corners_twice(&self) -> [[i32; 3]; 4] {
        let axis = self.side.axis();
        let u = (axis + 1) % 3;
        let v = (axis + 2) % 3;
        let mut result = [[0; 3]; 4];
        for (corner, [a, b]) in result.iter_mut().zip([[-1, -1], [1, -1], [1, 1], [-1, 1]]) {
            *corner = self.cell.map(|c| 2 * c as i32);
            corner[axis] += if self.side.positive() { 1 } else { -1 };
            corner[u] += a;
            corner[v] += b;
        }
        if !self.side.positive() {
            result.swap(1, 3);
        }
        result
    }
    pub(super) fn physical(&self, source: &Volume) -> Result<FaceGeometry, SurfaceError> {
        if self
            .cell
            .iter()
            .zip(source.dimensions())
            .any(|(c, d)| *c as usize >= d)
        {
            return Err(SurfaceError::Geometry);
        }
        let axis = self.side.axis();
        let normal = std::array::from_fn(|world_axis| {
            let mut unit = [0.; 3];
            unit[world_axis] = 1.;
            source.world_vector_to_voxel(unit)[axis] * if self.side.positive() { 1. } else { -1. }
        });
        let normal =
            crate::volume_material::gradient_normal(normal).ok_or(SurfaceError::Geometry)?;
        let mut corners_mm = self
            .corners_twice()
            .map(|c| source.voxel_to_world(c.map(|v| f64::from(v) * 0.5)));
        if corners_mm.iter().any(|v| !finite(*v)) {
            return Err(SurfaceError::Geometry);
        }
        let area = cross(
            sub(corners_mm[1], corners_mm[0]),
            sub(corners_mm[2], corners_mm[0]),
        );
        let area_mm2 = norm(area);
        let winding = dot(area, normal);
        if !area_mm2.is_finite() || area_mm2 <= 0. || !winding.is_finite() || winding == 0. {
            return Err(SurfaceError::Geometry);
        }
        if winding < 0. {
            corners_mm.swap(1, 3);
        }
        Ok(FaceGeometry {
            corners_mm,
            normal,
            area_mm2,
        })
    }
}
