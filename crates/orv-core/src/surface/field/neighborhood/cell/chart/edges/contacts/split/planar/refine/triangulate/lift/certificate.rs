// SPDX-License-Identifier: GPL-3.0-or-later
//! Exact interior error bounds for candidate source-field triangles.
use super::*;
mod adaptive;
pub use adaptive::{
    AdaptiveFacetLimits, AdaptiveFacetMesh, AdaptiveFacetState, AdaptiveFacetTriangle,
    AdaptiveStopReason, AppliedFaceCell, AppliedFaceLimits, AppliedFacePair, AppliedFaceSpan,
    AppliedFaceTriangle, AppliedFaceVertex, AppliedTriangleState, CellFaceReport, CellFaceState,
    CellSeamBatch, CellSeamLimits, CellSeamStop, ConformingFacetState, ConformingStopReason,
    ConformingSurfaceLimits, ConformingSurfaceMesh, ConformingTriangle, ConformingVertex,
    FacePartitionLimits, FacePartitionPoint, FacePartitionSpan, FacePlacement, FacePromotionLimits,
    FaceSpanCoverage, PhysicalConformingLimits, SharedFacePartition,
};

/// An axial error bound applies only over the triangle's projected domain.
/// It does not establish curved-boundary topology or a neighboring seam.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SurfaceFacetCertificate {
    NotCandidate,
    NonPositiveProjection,
    DerivativeUnproven,
    EndpointSignsUnproven,
    /// Every triangle point is within this many source-cell units along the
    /// projection axis of a unique zero, and vice versa on the same projection.
    Certified {
        numerator: u128,
        denominator: u128,
    },
}

// A source-axis displacement maps to one affine basis vector. Its L1 norm is
// an upper bound for Euclidean length, including tilted and reflected grids.
// Round every operation outward; unrepresentable results must not look bounded.
fn physical_upper_bound(
    certificate: SurfaceFacetCertificate,
    basis: [f64; 3],
) -> Result<Option<f64>, SurfaceError> {
    let SurfaceFacetCertificate::Certified {
        numerator,
        denominator,
    } = certificate
    else {
        return Ok(None);
    };
    if denominator == 0 || basis.iter().any(|component| !component.is_finite()) {
        return Err(SurfaceError::Geometry);
    }
    if numerator == 0 {
        return Ok(Some(0.));
    }
    let upper = |value: f64| {
        let next = value.next_up();
        if next.is_finite() {
            Ok(next)
        } else {
            Err(SurfaceError::Limit)
        }
    };
    let mut length = 0.;
    for component in basis {
        let sum = length + component.abs();
        length = if sum == 0. { 0. } else { upper(sum)? };
    }
    if length == 0. {
        return Ok(Some(0.));
    }
    let numerator = upper(numerator as f64)?;
    let denominator = (denominator as f64).next_down();
    if denominator <= 0. || !denominator.is_finite() {
        return Err(SurfaceError::Geometry);
    }
    let ratio = upper(numerator / denominator)?;
    Ok(Some(upper(ratio * length)?))
}

fn axis_basis(volume: &crate::volume::Volume, axis: u8) -> Result<[f64; 3], SurfaceError> {
    if axis >= 3 {
        return Err(SurfaceError::Geometry);
    }
    Ok(volume.voxel_vector_to_world(std::array::from_fn(|i| {
        if i == usize::from(axis) { 1. } else { 0. }
    })))
}

// Three 16-bit coordinate enclosures have dyadic midpoints on this common grid.
const GRID: u32 = 1 << 17;

fn midpoint(vertex: BoundedSurfaceVertex) -> Result<Option<[u32; 3]>, SurfaceError> {
    let BoundedSurfaceVertex::Point { axes, .. } = vertex else {
        return Ok(None);
    };
    let mut point = [0; 3];
    for (out, b) in point.iter_mut().zip(axes) {
        let twice = b.denominator.checked_mul(2).ok_or(SurfaceError::Geometry)?;
        if b.denominator == 0
            || b.lower > b.upper
            || b.upper > b.denominator
            || !GRID.is_multiple_of(twice)
        {
            return Err(SurfaceError::Geometry);
        }
        *out = b
            .lower
            .checked_add(b.upper)
            .and_then(|n| n.checked_mul(GRID / twice))
            .ok_or(SurfaceError::Geometry)?;
    }
    Ok(Some(point))
}

// Coordinates are in [0, grid], with grid <= 2^25 and field corners in
// [-255, 255]. Products and six-term sums fit comfortably in i128.
fn field_value(values: [i16; 8], point: [u32; 3], grid: u32) -> i128 {
    let weights = point.map(|p| [i128::from(grid - p), i128::from(p)]);
    values
        .into_iter()
        .enumerate()
        .map(|(i, value)| {
            i128::from(value)
                * weights[0][i & 1]
                * weights[1][(i >> 1) & 1]
                * weights[2][(i >> 2) & 1]
        })
        .sum()
}

fn evaluate(
    values: [i16; 8],
    axis: usize,
    normal: i8,
    points: [[u32; 3]; 3],
    cancel: &impl Fn() -> bool,
) -> Result<SurfaceFacetCertificate, SurfaceError> {
    evaluate_grid(values, axis, normal, points, GRID, cancel)
}

fn evaluate_grid(
    values: [i16; 8],
    axis: usize,
    normal: i8,
    points: [[u32; 3]; 3],
    grid: u32,
    cancel: &impl Fn() -> bool,
) -> Result<SurfaceFacetCertificate, SurfaceError> {
    check(cancel)?;
    if axis >= 3
        || ![-1, 1].contains(&normal)
        || grid == 0
        || grid > (1 << 25)
        || points.iter().flatten().any(|p| *p > grid)
    {
        return Err(SurfaceError::Geometry);
    }
    let [u, v] = transverse(axis);
    let edge = |i: usize, j: usize, a: usize| i128::from(points[i][a]) - i128::from(points[j][a]);
    if edge(1, 0, u) * edge(2, 0, v) - edge(1, 0, v) * edge(2, 0, u) <= 0 {
        return Ok(SurfaceFacetCertificate::NonPositiveProjection);
    }
    let mut derivative_min = i128::MAX;
    let mut signs_ok = true;
    for i in 0..3 {
        for j in i..3 {
            check(cancel)?;
            let face_value = |upper: bool, a: usize, b: usize| {
                let mut p = [0; 3];
                p[axis] = if upper { grid } else { 0 };
                p[u] = points[a][u];
                p[v] = points[b][v];
                field_value(values, p, grid)
            };
            // Twice each quadratic Bernstein coefficient, on the grid^3 scale.
            let lo = face_value(false, i, j) + face_value(false, j, i);
            let hi = face_value(true, i, j) + face_value(true, j, i);
            let n = i128::from(normal);
            signs_ok &= n * lo <= 0 && n * hi >= 0;
            derivative_min = derivative_min.min(n * (hi - lo));
        }
    }
    if derivative_min <= 0 {
        return Ok(SurfaceFacetCertificate::DerivativeUnproven);
    }
    if !signs_ok {
        return Ok(SurfaceFacetCertificate::EndpointSignsUnproven);
    }
    let mut residual_max = 0u128;
    for i in 0..3 {
        for j in i..3 {
            for k in j..3 {
                check(cancel)?;
                let permutations = [
                    [i, j, k],
                    [i, k, j],
                    [j, i, k],
                    [j, k, i],
                    [k, i, j],
                    [k, j, i],
                ];
                let mut coefficient = 0i128;
                for [a, b, c] in permutations {
                    coefficient +=
                        field_value(values, [points[a][0], points[b][1], points[c][2]], grid);
                }
                residual_max = residual_max.max(coefficient.unsigned_abs());
            }
        }
    }
    check(cancel)?;
    // Cubic coefficients use 6*grid^3, derivative coefficients 2*grid^3.
    let denominator = u128::try_from(derivative_min)
        .map_err(|_| SurfaceError::Geometry)?
        .checked_mul(3)
        .ok_or(SurfaceError::Limit)?;
    Ok(SurfaceFacetCertificate::Certified {
        numerator: residual_max,
        denominator,
    })
}

impl EnclosedSurfaceGraph {
    /// Coordinate denominator shared by `vertex_lattice_point` and the certified
    /// triangle representatives. These are local lattice, not physical coordinates.
    pub const POSITION_DENOMINATOR: u32 = GRID;

    /// One deterministic representative of a uniquely defined source vertex.
    /// Empty and whole fibers do not have a representative point.
    pub fn vertex_lattice_point(&self, id: u32) -> Result<Option<[u32; 3]>, SurfaceError> {
        midpoint(
            *self
                .vertices
                .get(id as usize)
                .ok_or(SurfaceError::Geometry)?,
        )
    }

    /// Prove an axial error bound for one retained candidate triangle. Returns an
    /// explicit unproven state if uniqueness or face signs cannot be certified.
    /// This does not certify curved boundaries, adjacent seams or physical error.
    pub fn facet_certificate(
        &self,
        id: u32,
        cancel: impl Fn() -> bool,
    ) -> Result<SurfaceFacetCertificate, SurfaceError> {
        check(&cancel)?;
        let index = id as usize;
        let state = self.facets.get(index).ok_or(SurfaceError::Geometry)?;
        let BoundedSurfaceFacet::Candidate { normal } = *state else {
            return Ok(SurfaceFacetCertificate::NotCandidate);
        };
        let triangle = self
            .input
            .triangles()
            .get(index)
            .ok_or(SurfaceError::Geometry)?;
        let [a, b, c] = triangle
            .vertices
            .map(|vertex| self.vertex_lattice_point(vertex));
        let points = [
            a?.ok_or(SurfaceError::Geometry)?,
            b?.ok_or(SurfaceError::Geometry)?,
            c?.ok_or(SurfaceError::Geometry)?,
        ];
        let chart = self.input.input().input().input().edges().chart();
        let field = chart
            .graph()
            .cell()
            .interpolant()
            .ok_or(SurfaceError::Geometry)?;
        evaluate(
            field.values(),
            usize::from(chart.axis()),
            normal,
            points,
            &cancel,
        )
    }

    /// Conservative millimetre separation for a certified facet, using the
    /// retained volume's affine basis. `None` means the axial certificate was
    /// not proved. This does not bound world-coordinate rounding, GPU precision,
    /// curved boundaries, seams or noncandidate surface regions.
    pub fn facet_physical_error_upper_mm(
        &self,
        id: u32,
        cancel: impl Fn() -> bool,
    ) -> Result<Option<f64>, SurfaceError> {
        let certificate = self.facet_certificate(id, cancel)?;
        let chart = self.input.input().input().input().edges().chart();
        let basis = axis_basis(chart.graph().field().source(), chart.axis())?;
        physical_upper_bound(certificate, basis)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn physical_bound_rounds_outward_and_refuses_unrepresentable_inputs() {
        let certificate = SurfaceFacetCertificate::Certified {
            numerator: 1,
            denominator: 2,
        };
        let upper = physical_upper_bound(certificate, [3., -4., 0.])
            .unwrap()
            .unwrap();
        assert!((3.5..3.500000000001).contains(&upper));
        let large = SurfaceFacetCertificate::Certified {
            numerator: u128::MAX,
            denominator: u128::MAX,
        };
        assert!(physical_upper_bound(large, [3., 4., 0.]).unwrap().unwrap() >= 7.);
        let small = SurfaceFacetCertificate::Certified {
            numerator: 1,
            denominator: u128::MAX,
        };
        assert!(
            physical_upper_bound(small, [f64::from_bits(1), 0., 0.])
                .unwrap()
                .unwrap()
                > 0.
        );
        assert_eq!(
            physical_upper_bound(
                SurfaceFacetCertificate::Certified {
                    numerator: 0,
                    denominator: 1
                },
                [3., 4., 0.]
            )
            .unwrap(),
            Some(0.)
        );
        assert_eq!(
            physical_upper_bound(SurfaceFacetCertificate::DerivativeUnproven, [3., 4., 0.])
                .unwrap(),
            None
        );
        assert_eq!(
            physical_upper_bound(
                SurfaceFacetCertificate::Certified {
                    numerator: 1,
                    denominator: 0
                },
                [3., 4., 0.]
            ),
            Err(SurfaceError::Geometry)
        );
        assert_eq!(
            physical_upper_bound(certificate, [f64::NAN, 0., 0.]),
            Err(SurfaceError::Geometry)
        );
        assert_eq!(
            physical_upper_bound(certificate, [f64::MAX, 0., 0.]),
            Err(SurfaceError::Limit)
        );
    }

    fn points(axis: usize) -> [[u32; 3]; 3] {
        let [u, v] = transverse(axis);
        let mut p = [[0; 3]; 3];
        p[1][u] = GRID;
        p[2][v] = GRID;
        for point in &mut p {
            point[axis] = GRID / 2;
        }
        p
    }

    #[test]
    fn bernstein_certificate_encloses_independent_interior_zeroes_on_every_axis() {
        for axis in 0..3 {
            let [u, v] = transverse(axis);
            let values = std::array::from_fn(|i| {
                if i & (1 << axis) == 0 {
                    -20
                } else {
                    20 + 4 * ((i >> u) & 1) as i16 + 6 * ((i >> v) & 1) as i16
                }
            });
            let field = Trilinear::new(values).unwrap();
            for (data, normal) in [(values, 1), (values.map(|x| -x), -1)] {
                let SurfaceFacetCertificate::Certified {
                    numerator,
                    denominator,
                } = evaluate(data, axis, normal, points(axis), &|| false).unwrap()
                else {
                    panic!("expected certificate")
                };
                assert!(numerator > 0 && denominator > 0);
                let bound = numerator as f64 / denominator as f64;
                for i in 0..=20 {
                    for j in 0..=20 - i {
                        let mut point = [0.; 3];
                        point[u] = f64::from(i) / 20.;
                        point[v] = f64::from(j) / 20.;
                        let lo = field.evaluate(point).unwrap();
                        point[axis] = 1.;
                        let hi = field.evaluate(point).unwrap();
                        let root = -lo / (hi - lo);
                        assert!((root - 0.5).abs() <= bound + 1e-12);
                    }
                }
            }
        }
    }

    #[test]
    fn generated_triangles_bound_independently_sampled_zeroes() {
        let mut seed = 0x9e3779b9u32;
        let mut random = || {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            seed
        };
        for case in 0..1200 {
            let axis = case % 3;
            let [u, v] = transverse(axis);
            let values = std::array::from_fn(|i| {
                let magnitude = 2 + (random() % 250) as i16;
                if i & (1 << axis) == 0 {
                    -magnitude
                } else {
                    magnitude
                }
            });
            let mut triangle = [[0u32; 3]; 3];
            let base_u = random() % (GRID / 2);
            let base_v = random() % (GRID / 2);
            let du = 1 + random() % (GRID - base_u);
            let dv = 1 + random() % (GRID - base_v);
            for point in &mut triangle {
                point[u] = base_u;
                point[v] = base_v;
                point[axis] = random() % (GRID + 1);
            }
            triangle[1][u] += du;
            triangle[2][v] += dv;
            let result = evaluate(values, axis, 1, triangle, &|| false).unwrap();
            let SurfaceFacetCertificate::Certified {
                numerator,
                denominator,
            } = result
            else {
                panic!("case {case}: {result:?}")
            };
            let bound = numerator as f64 / denominator as f64;
            let field = Trilinear::new(values).unwrap();
            for i in 0..=4 {
                for j in 0..=4 - i {
                    let weights = [
                        f64::from(i) / 4.,
                        f64::from(j) / 4.,
                        f64::from(4 - i - j) / 4.,
                    ];
                    let mut p: [f64; 3] = std::array::from_fn(|d| {
                        (0..3)
                            .map(|k| weights[k] * f64::from(triangle[k][d]) / f64::from(GRID))
                            .sum()
                    });
                    let z = p[axis];
                    p[axis] = 0.;
                    let lo = field.evaluate(p).unwrap();
                    p[axis] = 1.;
                    let hi = field.evaluate(p).unwrap();
                    let root = -lo / (hi - lo);
                    assert!(
                        (z - root).abs() <= bound + 1e-10,
                        "case {case}: actual {} bound {bound}",
                        (z - root).abs()
                    );
                }
            }
        }
    }

    #[test]
    fn exact_flat_surface_is_zero_error_and_unproved_cases_are_not_certified() {
        let flat = [-2, -2, -2, -2, 2, 2, 2, 2];
        assert!(matches!(
            evaluate(flat, 2, 1, points(2), &|| false),
            Ok(SurfaceFacetCertificate::Certified {
                numerator: 0,
                denominator: 1..
            })
        ));
        let endpoint_failure = [-2, 1, -2, 1, 3, 3, 3, 3];
        assert_eq!(
            evaluate(endpoint_failure, 2, 1, points(2), &|| false),
            Ok(SurfaceFacetCertificate::EndpointSignsUnproven)
        );
        assert_eq!(
            evaluate(flat.map(|x| -x), 2, 1, points(2), &|| false),
            Ok(SurfaceFacetCertificate::DerivativeUnproven)
        );
        let mut reversed = points(2);
        reversed.swap(1, 2);
        assert_eq!(
            evaluate(flat, 2, 1, reversed, &|| false),
            Ok(SurfaceFacetCertificate::NonPositiveProjection)
        );
        assert!(matches!(
            evaluate(flat, 2, 1, points(2), &|| true),
            Err(SurfaceError::Cancelled)
        ));
    }
}
