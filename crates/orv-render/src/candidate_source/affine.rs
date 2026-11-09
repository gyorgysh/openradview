// SPDX-License-Identifier: GPL-3.0-or-later
//! Bounded integer representation of an exact affine source-cell polygon.
use orv_core::{
    surface::field::{AffineCell, AffineDisposition, ConformingSurfaceMesh, Rational},
    volume::Volume,
};

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct AffineGeometry {
    pub(super) vertices: [[u32; 3]; 6],
    pub(super) count: u8,
    pub(super) samples: [i16; 8],
    /// Conservative source-space L1 displacement of any point in the polygon.
    pub(super) error_mm: f64,
}
impl AffineGeometry {
    pub(crate) fn triangles(&self) -> impl Iterator<Item = [[u32; 3]; 3]> + '_ {
        (1..self.count - 1).map(|i| {
            [
                self.vertices[0],
                self.vertices[usize::from(i)],
                self.vertices[usize::from(i + 1)],
            ]
        })
    }
    pub(crate) fn triangle_count(&self) -> usize {
        usize::from(self.count) - 2
    }
}
fn difference(a: Rational, b: Rational) -> (i128, i128) {
    (
        i128::from(a.numerator()) * i128::from(b.denominator())
            - i128::from(b.numerator()) * i128::from(a.denominator()),
        i128::from(a.denominator()) * i128::from(b.denominator()),
    )
}
fn exact_turn(a: [Rational; 3], b: [Rational; 3], c: [Rational; 3], axes: [usize; 2]) -> i128 {
    let (x, xd) = difference(b[axes[0]], a[axes[0]]);
    let (y, yd) = difference(b[axes[1]], a[axes[1]]);
    let (u, ud) = difference(c[axes[0]], a[axes[0]]);
    let (v, vd) = difference(c[axes[1]], a[axes[1]]);
    x * v * yd * ud - y * u * xd * vd
}
fn turn(a: [u32; 3], b: [u32; 3], c: [u32; 3], axes: [usize; 2]) -> i128 {
    let x = i128::from(b[axes[0]]) - i128::from(a[axes[0]]);
    let y = i128::from(b[axes[1]]) - i128::from(a[axes[1]]);
    let u = i128::from(c[axes[0]]) - i128::from(a[axes[0]]);
    let v = i128::from(c[axes[1]]) - i128::from(a[axes[1]]);
    x * v - y * u
}
fn intersects(a: [u32; 3], b: [u32; 3], c: [u32; 3], d: [u32; 3], axes: [usize; 2]) -> bool {
    let (ab_c, ab_d, cd_a, cd_b) = (
        turn(a, b, c, axes),
        turn(a, b, d, axes),
        turn(c, d, a, axes),
        turn(c, d, b, axes),
    );
    let overlaps = (0..2).all(|i| {
        let j = axes[i];
        a[j].min(b[j]) <= c[j].max(d[j]) && c[j].min(d[j]) <= a[j].max(b[j])
    });
    overlaps && ab_c.signum() * ab_d.signum() <= 0 && cd_a.signum() * cd_b.signum() <= 0
}
/// The original exact polygon is retained by the assembly. Return None when
/// rounding would exceed tolerance, collapse a feature or change its topology.
pub(super) fn quantize(
    cell: &AffineCell,
    source: &Volume,
    tolerance: f64,
) -> Result<Option<AffineGeometry>, &'static str> {
    if cell.disposition() != AffineDisposition::Plane {
        return Ok(None);
    }
    let original = cell.vertices();
    if !(3..=6).contains(&original.len()) || !tolerance.is_finite() || tolerance < 0. {
        return Err("Invalid affine candidate");
    }
    let denom = u64::from(ConformingSurfaceMesh::POSITION_DENOMINATOR);
    let mut geometry = AffineGeometry {
        vertices: [[0; 3]; 6],
        count: original.len() as u8,
        samples: [0; 8],
        error_mm: 0.,
    };
    for (target, sample) in geometry.samples.iter_mut().zip(cell.cell().corners()) {
        if !sample.source_valid {
            return Err("Affine candidate has invalid source data");
        }
        *target = sample
            .value
            .ok_or("Affine candidate has unknown mask coverage")?;
    }
    let mut basis = [0.; 3];
    for (axis, bound) in basis.iter_mut().enumerate() {
        let mut unit = [0.; 3];
        unit[axis] = 1.;
        for value in source.voxel_vector_to_world(unit) {
            *bound = (*bound + value.abs()).next_up();
        }
        if !bound.is_finite() {
            return Err("Affine candidate physical basis is not finite");
        }
    }
    for (index, vertex) in original.iter().enumerate() {
        let mut error = 0f64;
        for axis in 0..3 {
            let ratio = vertex[axis];
            let num = u64::try_from(ratio.numerator())
                .map_err(|_| "Affine vertex is outside its cell")?;
            let den = ratio.denominator();
            if den == 0 || num > den {
                return Err("Affine vertex is outside its cell");
            }
            let scaled = num.checked_mul(denom).ok_or("Affine position overflow")?;
            let rounded = scaled
                .checked_mul(2)
                .and_then(|n| n.checked_add(den))
                .map(|n| n / (2 * den))
                .ok_or("Affine position overflow")?;
            geometry.vertices[index][axis] =
                u32::try_from(rounded).map_err(|_| "Affine position overflow")?;
            let remainder = rounded
                .checked_mul(den)
                .ok_or("Affine position overflow")?
                .abs_diff(scaled);
            if remainder != 0 {
                let fraction = ((remainder as f64) / (den as f64 * denom as f64)).next_up();
                error = (error + (fraction * basis[axis]).next_up()).next_up();
            }
        }
        geometry.error_mm = geometry.error_mm.max(error);
    }
    if !geometry.error_mm.is_finite() || geometry.error_mm > tolerance {
        return Ok(None);
    }
    let gradient = cell.gradient().ok_or("Affine candidate has no gradient")?;
    let projection = (0..3)
        .max_by_key(|i| gradient[*i].abs())
        .ok_or("Affine candidate has no gradient")?;
    let axes = match projection {
        0 => [1, 2],
        1 => [0, 2],
        _ => [0, 1],
    };
    let orientation = exact_turn(original[0], original[1], original[2], axes).signum();
    if orientation == 0 {
        return Err("Affine candidate has degenerate exact geometry");
    }
    let n = original.len();
    for i in 0..n {
        if turn(
            geometry.vertices[i],
            geometry.vertices[(i + 1) % n],
            geometry.vertices[(i + 2) % n],
            axes,
        )
        .signum()
            != orientation
        {
            return Ok(None);
        }
        for j in i + 1..n {
            if (i + 1) % n == j || (j + 1) % n == i {
                continue;
            }
            if intersects(
                geometry.vertices[i],
                geometry.vertices[(i + 1) % n],
                geometry.vertices[j],
                geometry.vertices[(j + 1) % n],
                axes,
            ) {
                return Ok(None);
            }
        }
    }
    Ok(Some(geometry))
}
