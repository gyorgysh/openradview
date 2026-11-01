// SPDX-License-Identifier: GPL-3.0-or-later
//! Pixel-centre regions with explicit boundaries; no dense mask allocation.
use crate::study::Frame;

pub const MAX_POLYGON_VERTICES: usize = 256;
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RoiShape<'a> {
    /// Includes pixel centres on every boundary. One-row/one-cell rectangles are valid.
    Rectangle([[f64; 2]; 2]),
    /// Opposite corners of the axis-aligned bounds; numerically indistinguishable axes fail.
    Ellipse([[f64; 2]; 2]),
    /// Implicitly closed, simple polygon without holes. Winding may be reversed.
    Polygon(&'a [[f64; 2]]),
}
pub(super) struct Prepared<'a> {
    shape: RoiShape<'a>,
    min: [f64; 2],
    max: [f64; 2],
    pub area_pixels: f64,
}
fn cross(a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> f64 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}
fn orientation(a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> i8 {
    let left = (b[0] - a[0]) * (c[1] - a[1]);
    let right = (b[1] - a[1]) * (c[0] - a[0]);
    // A conservative roundoff band makes nearly touching edges fail validation.
    let tolerance = (left.abs() + right.abs()) * 64. * f64::EPSILON;
    let value = left - right;
    if value.abs() <= tolerance {
        0
    } else if value > 0. {
        1
    } else {
        -1
    }
}
fn on_segment(a: [f64; 2], b: [f64; 2], p: [f64; 2]) -> bool {
    orientation(a, b, p) == 0
        && (0..2).all(|axis| {
            let tolerance = a[axis].abs().max(b[axis].abs()).max(1.) * 64. * f64::EPSILON;
            p[axis] >= a[axis].min(b[axis]) - tolerance
                && p[axis] <= a[axis].max(b[axis]) + tolerance
        })
}
fn intersect(a: [f64; 2], b: [f64; 2], c: [f64; 2], d: [f64; 2]) -> bool {
    let (abc, abd, cda, cdb) = (
        orientation(a, b, c),
        orientation(a, b, d),
        orientation(c, d, a),
        orientation(c, d, b),
    );
    (abc * abd < 0 && cda * cdb < 0)
        || on_segment(a, b, c)
        || on_segment(a, b, d)
        || on_segment(c, d, a)
        || on_segment(c, d, b)
}
impl<'a> RoiShape<'a> {
    /// Geometry-only validation for a candidate region, without reading image samples.
    pub fn validate(
        self,
        size: [u32; 2],
        cancelled: impl Fn() -> bool,
    ) -> Result<(), &'static str> {
        self.prepare(size, &cancelled).map(|_| ())
    }
    pub(super) fn prepare(
        self,
        size: [u32; 2],
        cancel: &dyn Fn() -> bool,
    ) -> Result<Prepared<'a>, &'static str> {
        if cancel() {
            return Err("Cancelled");
        }
        let points = match &self {
            Self::Rectangle(points) | Self::Ellipse(points) => points.as_slice(),
            Self::Polygon(points) => points,
        };
        if matches!(self, Self::Polygon(_)) && !(3..=MAX_POLYGON_VERTICES).contains(&points.len()) {
            return Err("Polygons need three to 256 vertices");
        }
        if size.contains(&0) || points.iter().flatten().any(|v| !v.is_finite()) {
            return Err("ROI coordinates or image are invalid");
        }
        let min =
            std::array::from_fn(|axis| points.iter().fold(f64::INFINITY, |v, p| v.min(p[axis])));
        let max = std::array::from_fn(|axis| {
            points.iter().fold(f64::NEG_INFINITY, |v, p| v.max(p[axis]))
        });
        if (0..2).any(|axis| min[axis] < 0. || max[axis] > f64::from(size[axis] - 1)) {
            return Err("ROI must lie inside the image");
        }
        let box_area = (max[0] - min[0]) * (max[1] - min[1]);
        let area_pixels = match self {
            Self::Rectangle(_) => box_area,
            Self::Ellipse(_) => {
                if (0..2)
                    .any(|axis| max[axis] - min[axis] <= max[axis].max(1.) * 64. * f64::EPSILON)
                {
                    return Err("An ellipse needs two distinguishable nonzero axes");
                }
                let area = box_area * (std::f64::consts::PI / 4.);
                if !area.is_finite() || area <= 0. {
                    return Err("An ellipse needs two nonzero axes");
                }
                area
            }
            Self::Polygon(points) => {
                let n = points.len();
                let mut twice_area = 0.;
                for i in 0..n {
                    if cancel() {
                        return Err("Cancelled");
                    }
                    let (a, b) = (points[i], points[(i + 1) % n]);
                    if a == b {
                        return Err("A polygon cannot repeat adjacent vertices");
                    }
                    let c = points[(i + 2) % n];
                    if on_segment(a, b, c) || on_segment(b, c, a) {
                        return Err("A polygon cannot retrace an edge");
                    }
                    twice_area += cross(points[0], a, b);
                    for j in i + 1..n {
                        if j.is_multiple_of(32) && cancel() {
                            return Err("Cancelled");
                        }
                        if j == i + 1 || (i == 0 && j == n - 1) {
                            continue;
                        }
                        if intersect(a, b, points[j], points[(j + 1) % n]) {
                            return Err("Polygon edges cannot cross or touch");
                        }
                    }
                }
                let area = twice_area.abs() / 2.;
                // Refuse numerically negligible area instead of assigning a winding to it.
                if !area.is_finite() || area <= box_area.max(1.) * n as f64 * 64. * f64::EPSILON {
                    return Err("The polygon is degenerate or too close to a straight line");
                }
                area
            }
        };
        Ok(Prepared {
            shape: self,
            min,
            max,
            area_pixels,
        })
    }
}
impl Prepared<'_> {
    fn contains(&self, p: [f64; 2]) -> bool {
        match self.shape {
            RoiShape::Rectangle(_) => true,
            RoiShape::Ellipse(_) => {
                let normalized: [f64; 2] = std::array::from_fn(|axis| {
                    let radius = (self.max[axis] - self.min[axis]) / 2.;
                    (p[axis] - (self.min[axis] + radius)) / radius
                });
                normalized[0] * normalized[0] + normalized[1] * normalized[1]
                    <= 1. + 64. * f64::EPSILON
            }
            RoiShape::Polygon(points) => {
                let mut inside = false;
                for i in 0..points.len() {
                    let (a, b) = (points[i], points[(i + 1) % points.len()]);
                    if on_segment(a, b, p) {
                        return true;
                    }
                    if (a[1] > p[1]) != (b[1] > p[1]) {
                        let x = a[0] + (p[1] - a[1]) / (b[1] - a[1]) * (b[0] - a[0]);
                        if p[0] < x {
                            inside = !inside;
                        }
                    }
                }
                inside
            }
        }
    }
    pub fn visit(
        &self,
        width: u32,
        cancel: &dyn Fn() -> bool,
        mut sample: impl FnMut(usize) -> Result<(), &'static str>,
    ) -> Result<(), &'static str> {
        for y in self.min[1].ceil() as usize..=self.max[1].floor() as usize {
            if cancel() {
                return Err("Cancelled");
            }
            for x in self.min[0].ceil() as usize..=self.max[0].floor() as usize {
                if x.is_multiple_of(1024) && cancel() {
                    return Err("Cancelled");
                }
                if self.contains([x as f64, y as f64]) {
                    sample(y * width as usize + x)?;
                }
            }
        }
        if cancel() {
            return Err("Cancelled");
        }
        Ok(())
    }
}
pub(super) fn area_mm2(frame: &Frame, pixels: f64) -> Option<f64> {
    let geometry = frame.geometry.as_ref().filter(|g| g.validate().is_ok())?;
    let (column, row) = geometry.directions();
    let area = crate::vector::norm(crate::vector::cross(column, row))
        * geometry.spacing[0]
        * geometry.spacing[1]
        * pixels;
    (area.is_finite() && (area > 0. || pixels == 0.)).then_some(area)
}
