// SPDX-License-Identifier: GPL-3.0-or-later
use super::{BrushShape, MAX_PHYSICAL_BRUSH_RADIUS_MM, PhysicalBrush};
use crate::{vector::*, volume::Volume};

pub(in crate::labels) struct Segment {
    a: V3,
    delta: V3,
    length_squared: f64,
    pub bounds: [[usize; 3]; 2],
}
struct Plane {
    origin: V3,
    normal: V3,
    half_thickness: f64,
}
pub(in crate::labels) struct Footprint {
    source_origin: V3,
    radius_squared: f64,
    bounding_radius: f64,
    plane: Option<Plane>,
    clip: Option<[V3; 2]>,
}
fn local(point: V3, origin: V3) -> Result<V3, &'static str> {
    let p = sub(point, origin);
    if !finite(point) || !finite(p) || p.into_iter().any(|v| v.abs() > 1e9) {
        return Err("Physical brush coordinates exceed the supported finite range");
    }
    Ok(p)
}
impl Footprint {
    pub fn new(source: &Volume, settings: &PhysicalBrush) -> Result<Self, &'static str> {
        if !settings.radius_mm.is_finite()
            || settings.radius_mm < 1e-6
            || settings.radius_mm > MAX_PHYSICAL_BRUSH_RADIUS_MM
        {
            return Err("Physical brush radius must be from 0.000001 to 256 mm");
        }
        let source_origin = source.voxel_to_world([0.; 3]);
        for corner in 0..8 {
            let p = source.voxel_vector_to_world(std::array::from_fn(|a| {
                if corner & (1 << a) == 0 {
                    0.
                } else {
                    (source.dimensions()[a] - 1) as f64
                }
            }));
            if !finite(p) || p.into_iter().any(|v| v.abs() > 1e9) {
                return Err("Physical brush source extent exceeds the supported finite range");
            }
        }
        let plane = match settings.shape {
            BrushShape::Sphere => None,
            BrushShape::Plane {
                origin_mm,
                normal,
                thickness_mm,
            } => {
                let scale = normal.into_iter().map(f64::abs).fold(0., f64::max);
                if !finite(normal)
                    || scale == 0.
                    || !thickness_mm.is_finite()
                    || !(1e-6..=512.).contains(&thickness_mm)
                {
                    return Err("Plane brushes need a finite normal and a thickness up to 512 mm");
                }
                // Scaling first also preserves directions represented by
                // subnormals, or by finite components whose norm would overflow.
                let scaled = normal.map(|n| n / scale);
                let length = scaled[0].hypot(scaled[1]).hypot(scaled[2]);
                Some(Plane {
                    origin: local(origin_mm, source_origin)?,
                    normal: scaled.map(|n| n / length),
                    half_thickness: thickness_mm * 0.5,
                })
            }
        };
        let clip = settings
            .clip
            .as_ref()
            .map(|c| {
                c.validate()?;
                Ok([
                    local(c.minimum_mm, source_origin)?,
                    local(c.maximum_mm, source_origin)?,
                ])
            })
            .transpose()?;
        Ok(Self {
            source_origin,
            radius_squared: settings.radius_mm * settings.radius_mm,
            bounding_radius: settings.radius_mm + plane.as_ref().map_or(0., |p| p.half_thickness),
            plane,
            clip,
        })
    }
    fn project(&self, p: V3) -> V3 {
        if let Some(plane) = &self.plane {
            sub(
                p,
                mul(plane.normal, dot(sub(p, plane.origin), plane.normal)),
            )
        } else {
            p
        }
    }
    pub fn segment(&self, source: &Volume, a: V3, b: V3) -> Result<Segment, &'static str> {
        let local_a = local(a, self.source_origin)?;
        let local_b = local(b, self.source_origin)?;
        let magnitude = local_a
            .into_iter()
            .chain(local_b)
            .chain(self.plane.as_ref().map_or([0.; 3], |p| p.origin))
            .map(f64::abs)
            .fold(1., f64::max);
        let a = self.project(local_a);
        let b = self.project(local_b);
        let delta = sub(b, a);
        let length_squared = dot(delta, delta);
        if !finite(a) || !finite(b) || !length_squared.is_finite() {
            return Err("Physical stroke geometry is not finite");
        }
        Ok(Segment {
            a,
            delta,
            length_squared,
            bounds: bounds(source, a, b, self.bounding_radius, magnitude),
        })
    }
    pub fn contains(&self, segment: &Segment, p: V3) -> bool {
        if self
            .clip
            .is_some_and(|[lo, hi]| (0..3).any(|a| p[a] < lo[a] || p[a] > hi[a]))
        {
            return false;
        }
        if let Some(plane) = &self.plane
            && dot(sub(p, plane.origin), plane.normal).abs() > plane.half_thickness
        {
            return false;
        }
        let q = sub(self.project(p), segment.a);
        let t = if segment.length_squared > 0. {
            (dot(q, segment.delta) / segment.length_squared).clamp(0., 1.)
        } else {
            0.
        };
        let distance = sub(q, mul(segment.delta, t));
        dot(distance, distance) <= self.radius_squared
    }
}

fn bounds(
    source: &Volume,
    a: V3,
    b: V3,
    radius: f64,
    arithmetic_magnitude: f64,
) -> [[usize; 3]; 2] {
    let dims = source.dimensions();
    let full = [[0; 3], dims];
    let columns: [V3; 3] = std::array::from_fn(|axis| {
        let mut e = [0.; 3];
        e[axis] = 1.;
        source.world_vector_to_voxel(e)
    });
    let basis: [V3; 3] = std::array::from_fn(|axis| {
        let mut e = [0.; 3];
        e[axis] = 1.;
        source.voxel_vector_to_world(e)
    });
    let grid_magnitude: f64 = (0..3)
        .map(|world| {
            (0..3)
                .map(|axis| basis[axis][world].abs() * dims[axis] as f64)
                .sum::<f64>()
        })
        .fold(0., f64::max);
    let magnitude = a.into_iter().chain(b).map(f64::abs).fold(
        grid_magnitude.max(radius).max(arithmetic_magnitude),
        f64::max,
    );
    let start = source.world_vector_to_voxel(a);
    let end = source.world_vector_to_voxel(b);
    let mut result = full;
    for axis in 0..3 {
        let row = [columns[0][axis], columns[1][axis], columns[2][axis]];
        let row_sum: f64 = row.into_iter().map(f64::abs).sum();
        let mut residual = 0.;
        for j in 0..3 {
            let terms: f64 = (0..3).map(|w| (row[w] * basis[j][w]).abs()).sum();
            residual += ((dot(row, basis[j]) - f64::from(axis == j)).abs()
                + 16. * f64::EPSILON * terms)
                * dims[j] as f64;
        }
        // A unit-cell margin covers inverse residual and rounding in the small
        // bounded arithmetic above. Ill-conditioned/large terms retain a full
        // grid scan instead of trusting an inverse-derived pruning box.
        if !row_sum.is_finite()
            || !magnitude.is_finite()
            || 32. * row_sum * magnitude > 1e10
            || !residual.is_finite()
            || residual > 0.125
            || !start[axis].is_finite()
            || !end[axis].is_finite()
        {
            return full;
        }
        let extent = row_sum * radius;
        let lo = start[axis].min(end[axis]) - extent - 1.;
        let hi = start[axis].max(end[axis]) + extent + 1.;
        if hi < 0. || lo > (dims[axis] - 1) as f64 {
            return [[0; 3]; 2];
        }
        result[0][axis] = lo.ceil().max(0.) as usize;
        result[1][axis] = (hi.floor().max(0.) as usize + 1).min(dims[axis]);
    }
    result
}
