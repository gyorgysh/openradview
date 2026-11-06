// SPDX-License-Identifier: GPL-3.0-or-later
//! Source-owned 3D vertex enclosures over a planar triangulation.
use super::*;
mod certificate;
pub use certificate::{
    AdaptiveFacetLimits, AdaptiveFacetMesh, AdaptiveFacetState, AdaptiveFacetTriangle,
    AdaptiveStopReason, SurfaceFacetCertificate,
};

const MAX_OUTPUT: u64 = 16 * 1024 * 1024;

/// Inclusive dyadic interval on a source-cell lattice axis.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellAxisBounds {
    pub lower: u32,
    pub upper: u32,
    pub denominator: u32,
}

/// The complete fiber remains explicit when it cannot be represented by a point.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoundedSurfaceVertex {
    Empty,
    Whole,
    Point {
        axes: [CellAxisBounds; 3],
        normal: i8,
    },
}

/// Classification of the retained planar triangle using its source region signs
/// and its exact vertex fibers. Candidate status does not certify its interior,
/// approximate curved boundary, orientation in physical space, or a shared seam.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoundedSurfaceFacet {
    SameSignRegion,
    WholeRegion,
    Candidate { normal: i8 },
    Singular,
    Inconsistent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SurfaceEnclosureLimits {
    /// Precision for each local coordinate; at most 16 binary subdivisions.
    pub precision_bits: u8,
    /// Additional typed output; input and temporary stack values are separate.
    pub max_bytes: u64,
}
impl Default for SurfaceEnclosureLimits {
    fn default() -> Self {
        Self {
            precision_bits: 16,
            max_bytes: MAX_OUTPUT,
        }
    }
}

/// Retains every planar vertex and triangle by its original index and source.
/// This is an enclosure directory, not a renderable or certified 3D surface.
pub struct EnclosedSurfaceGraph {
    input: Arc<TriangulatedPlanarGraph>,
    vertices: Vec<BoundedSurfaceVertex>,
    facets: Vec<BoundedSurfaceFacet>,
    bytes: u64,
    _reservation: Reservation,
}
impl EnclosedSurfaceGraph {
    pub fn input(&self) -> &Arc<TriangulatedPlanarGraph> {
        &self.input
    }
    pub fn vertices(&self) -> &[BoundedSurfaceVertex] {
        &self.vertices
    }
    /// Each entry corresponds to the same-index input planar triangle.
    pub fn facets(&self) -> &[BoundedSurfaceFacet] {
        &self.facets
    }
    pub fn referenced_bytes(&self) -> u64 {
        self.bytes
    }
}

fn coordinate_bounds(
    value: ExactCoordinate,
    bits: u8,
    cancel: &impl Fn() -> bool,
) -> Result<CellAxisBounds, SurfaceError> {
    let (mut lower, mut upper, mut denominator) = (0u32, 1u32, 1u32);
    for _ in 0..bits {
        check(cancel)?;
        lower *= 2;
        upper *= 2;
        denominator *= 2;
        let middle = lower + 1;
        let ratio = Rational::new(i128::from(middle), i128::from(denominator)).map_err(error)?;
        let coordinate = ExactCoordinate::from_rational(ratio)
            .map_err(error)?
            .ok_or(SurfaceError::Geometry)?;
        match value.compare(coordinate).map_err(error)? {
            Ordering::Less => upper = middle,
            Ordering::Greater => lower = middle,
            Ordering::Equal => {
                lower = middle;
                upper = middle;
                break;
            }
        }
    }
    check(cancel)?;
    Ok(CellAxisBounds {
        lower,
        upper,
        denominator,
    })
}

fn facet(
    signs: [i8; 2],
    vertices: [BoundedSurfaceVertex; 3],
) -> Result<BoundedSurfaceFacet, SurfaceError> {
    if signs.iter().any(|s| !(-1..=1).contains(s)) {
        return Err(SurfaceError::Geometry);
    }
    let [lo, hi] = signs;
    if lo == 0 && hi == 0 {
        return Ok(BoundedSurfaceFacet::WholeRegion);
    }
    if lo != 0 && lo == hi {
        return Ok(BoundedSurfaceFacet::SameSignRegion);
    }
    let normal = (hi - lo).signum();
    if vertices
        .iter()
        .any(|v| matches!(v, BoundedSurfaceVertex::Empty))
        || vertices
            .iter()
            .any(|v| matches!(v, BoundedSurfaceVertex::Point { normal: n, .. } if *n != normal))
    {
        return Ok(BoundedSurfaceFacet::Inconsistent);
    }
    if vertices
        .iter()
        .any(|v| matches!(v, BoundedSurfaceVertex::Whole))
    {
        Ok(BoundedSurfaceFacet::Singular)
    } else {
        Ok(BoundedSurfaceFacet::Candidate { normal })
    }
}

impl TriangulatedPlanarGraph {
    /// Enclose source zero-fiber vertices and retain the disposition of every
    /// input triangle. The boxes do not bound the triangle's curved interior.
    pub fn enclose_surface(
        self: &Arc<Self>,
        limits: SurfaceEnclosureLimits,
        cancel: impl Fn() -> bool,
    ) -> Result<EnclosedSurfaceGraph, SurfaceError> {
        self.enclose_surface_with_budget(limits, MemoryBudget::volumes(), cancel)
    }
    pub(in crate::surface) fn enclose_surface_with_budget(
        self: &Arc<Self>,
        limits: SurfaceEnclosureLimits,
        budget: Arc<MemoryBudget>,
        cancel: impl Fn() -> bool,
    ) -> Result<EnclosedSurfaceGraph, SurfaceError> {
        check(&cancel)?;
        if limits.precision_bits > 16 || limits.max_bytes > MAX_OUTPUT {
            return Err(SurfaceError::Limit);
        }
        let count = self.input.vertex_count();
        let bytes = size(
            &[
                (count, std::mem::size_of::<BoundedSurfaceVertex>()),
                (
                    self.triangles.len(),
                    std::mem::size_of::<BoundedSurfaceFacet>(),
                ),
            ],
            std::mem::size_of::<EnclosedSurfaceGraph>(),
        )?;
        if bytes > limits.max_bytes {
            return Err(SurfaceError::Limit);
        }
        let reservation = budget.reserve(bytes).ok_or(SurfaceError::Memory)?;
        let mut vertices = exact(count)?;
        let mut facets = exact(self.triangles.len())?;
        let chart = self.input.input().input().edges().chart();
        let [u, v] = chart.transverse_axes();
        let axis = usize::from(chart.axis());
        for id in 0..count {
            check(&cancel)?;
            let id = u32::try_from(id).map_err(|_| SurfaceError::Limit)?;
            let point = *self.input.vertex(id).ok_or(SurfaceError::Geometry)?;
            let vertex = match chart
                .fiber_bounds(point, limits.precision_bits, &cancel)
                .map_err(error)?
            {
                CellFiberBounds::Empty => BoundedSurfaceVertex::Empty,
                CellFiberBounds::Whole => BoundedSurfaceVertex::Whole,
                CellFiberBounds::Point {
                    lower,
                    upper,
                    denominator,
                    normal,
                } => {
                    let mut axes = [CellAxisBounds {
                        lower: 0,
                        upper: 0,
                        denominator: 1,
                    }; 3];
                    axes[u] = coordinate_bounds(point[0], limits.precision_bits, &cancel)?;
                    axes[v] = coordinate_bounds(point[1], limits.precision_bits, &cancel)?;
                    axes[axis] = CellAxisBounds {
                        lower,
                        upper,
                        denominator,
                    };
                    BoundedSurfaceVertex::Point { axes, normal }
                }
            };
            vertices.push(vertex);
        }
        let graph = self.input.input();
        for triangle in &self.triangles {
            check(&cancel)?;
            let region = graph
                .regions()
                .get(triangle.region as usize)
                .ok_or(SurfaceError::Geometry)?;
            let [a, b, c] = triangle
                .vertices
                .map(|id| vertices.get(id as usize).copied());
            facets.push(facet(
                region.signs,
                [
                    a.ok_or(SurfaceError::Geometry)?,
                    b.ok_or(SurfaceError::Geometry)?,
                    c.ok_or(SurfaceError::Geometry)?,
                ],
            )?);
        }
        check(&cancel)?;
        Ok(EnclosedSurfaceGraph {
            input: Arc::clone(self),
            vertices,
            facets,
            bytes,
            _reservation: reservation,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn coordinate(n: i128, d: i128) -> ExactCoordinate {
        ExactCoordinate::from_rational(Rational::new(n, d).unwrap())
            .unwrap()
            .unwrap()
    }

    #[test]
    fn exact_coordinate_enclosures_cover_endpoints_and_close_rational_values() {
        for (n, d) in [
            (0, 1),
            (1, 1),
            (1, 3),
            (2, 3),
            (32767, 65535),
            (65534, 65535),
        ] {
            let point = coordinate(n, d);
            for bits in 0..=16 {
                let b = coordinate_bounds(point, bits, &|| false).unwrap();
                assert!(b.upper - b.lower <= 1);
                assert_ne!(
                    point
                        .compare(coordinate(i128::from(b.lower), i128::from(b.denominator)))
                        .unwrap(),
                    Ordering::Less
                );
                assert_ne!(
                    point
                        .compare(coordinate(i128::from(b.upper), i128::from(b.denominator)))
                        .unwrap(),
                    Ordering::Greater
                );
            }
        }
        assert!(matches!(
            coordinate_bounds(coordinate(1, 3), 16, &|| true),
            Err(SurfaceError::Cancelled)
        ));
    }

    #[test]
    fn singular_and_inconsistent_fibers_never_become_candidate_facets() {
        let box_axis = CellAxisBounds {
            lower: 0,
            upper: 1,
            denominator: 1,
        };
        let forward = BoundedSurfaceVertex::Point {
            axes: [box_axis; 3],
            normal: 1,
        };
        let reverse = BoundedSurfaceVertex::Point {
            axes: [box_axis; 3],
            normal: -1,
        };
        assert_eq!(
            facet([-1, 1], [forward; 3]),
            Ok(BoundedSurfaceFacet::Candidate { normal: 1 })
        );
        assert_eq!(
            facet([-1, 1], [forward, forward, BoundedSurfaceVertex::Whole]),
            Ok(BoundedSurfaceFacet::Singular)
        );
        assert_eq!(
            facet([-1, 1], [forward, forward, BoundedSurfaceVertex::Empty]),
            Ok(BoundedSurfaceFacet::Inconsistent)
        );
        assert_eq!(
            facet([-1, 1], [forward, forward, reverse]),
            Ok(BoundedSurfaceFacet::Inconsistent)
        );
        assert_eq!(
            facet([1, 1], [forward; 3]),
            Ok(BoundedSurfaceFacet::SameSignRegion)
        );
        assert_eq!(
            facet([0, 0], [BoundedSurfaceVertex::Whole; 3]),
            Ok(BoundedSurfaceFacet::WholeRegion)
        );
    }
}
