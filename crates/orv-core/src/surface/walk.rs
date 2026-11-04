// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use crate::mask::MaskView;

pub(super) const HALO_CELLS: usize = (CHUNK_SIDE + 2).pow(3);
const UNKNOWN: u8 = 0;
const EMPTY: u8 = 1;
const PRESENT: u8 = 2;
const OUTSIDE: u8 = 3;
pub(super) fn faces(
    view: MaskView<'_>,
    bounds: [[usize; 3]; 2],
    halo: &mut Vec<u8>,
    cancelled: &impl Fn() -> bool,
    mut emit: impl FnMut(CellFace) -> Result<(), SurfaceError>,
) -> Result<usize, SurfaceError> {
    let shape: [usize; 3] = std::array::from_fn(|a| bounds[1][a] - bounds[0][a] + 2);
    let [width, height, depth] = shape;
    let dimensions = view.source().dimensions();
    halo.resize(width * height * depth, UNKNOWN);
    for z in 0..depth {
        for y in 0..height {
            if cancelled() {
                return Err(SurfaceError::Cancelled);
            }
            for x in 0..width {
                let p: [isize; 3] =
                    std::array::from_fn(|a| bounds[0][a] as isize - 1 + [x, y, z][a] as isize);
                halo[(z * height + y) * width + x] = if p
                    .iter()
                    .zip(dimensions)
                    .any(|(p, d)| *p < 0 || *p as usize >= d)
                {
                    OUTSIDE
                } else {
                    let sample = view
                        .sample(p.map(|p| p as usize))
                        .ok_or(SurfaceError::Geometry)?;
                    if !sample.known || !sample.source_valid {
                        UNKNOWN
                    } else if sample.present {
                        PRESENT
                    } else {
                        EMPTY
                    }
                };
            }
        }
    }
    let mut visited = 0;
    for z in 1..depth - 1 {
        for y in 1..height - 1 {
            if cancelled() {
                return Err(SurfaceError::Cancelled);
            }
            for x in 1..width - 1 {
                visited += 1;
                let index = (z * height + y) * width + x;
                if halo[index] != PRESENT {
                    continue;
                }
                for (side, neighbor) in FaceSide::ALL.into_iter().zip([
                    index - 1,
                    index + 1,
                    index - width,
                    index + width,
                    index - width * height,
                    index + width * height,
                ]) {
                    let boundary = match halo[neighbor] {
                        EMPTY => FaceBoundary::Mask,
                        OUTSIDE => FaceBoundary::Grid,
                        _ => continue,
                    };
                    emit(CellFace {
                        cell: [
                            bounds[0][0] + x - 1,
                            bounds[0][1] + y - 1,
                            bounds[0][2] + z - 1,
                        ]
                        .map(|p| p as u32),
                        side,
                        boundary,
                    })?;
                }
            }
        }
    }
    Ok(visited)
}
