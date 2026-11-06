// SPDX-License-Identifier: GPL-3.0-or-later
//! Bounded source-field subdivision of a single certified-candidate facet.
use super::*;

const MAX_DEPTH: u8 = 8;
const MAX_TRIANGLES: usize = 1 << MAX_DEPTH;
const MAX_BYTES: u64 = 1024 * 1024;
const REFINED_GRID: u32 = GRID << MAX_DEPTH;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AdaptiveFacetLimits {
    /// Required axial error is at most 2^-tolerance_bits local cell units.
    pub tolerance_bits: u8,
    pub max_depth: u8,
    pub max_triangles: usize,
    /// Additional typed output; the input and scratch are admitted separately.
    pub max_bytes: u64,
}
impl Default for AdaptiveFacetLimits {
    fn default() -> Self {
        Self {
            tolerance_bits: 8,
            max_depth: MAX_DEPTH,
            max_triangles: MAX_TRIANGLES,
            max_bytes: MAX_BYTES,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdaptiveStopReason {
    DepthLimit,
    TriangleLimit,
    NonPositiveProjection,
    EmptyMidpoint,
    WholeMidpoint,
    OppositeMidpointNormal,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdaptiveFacetState {
    WithinTolerance {
        numerator: u128,
        denominator: u128,
    },
    Unresolved {
        certificate: SurfaceFacetCertificate,
        reason: AdaptiveStopReason,
    },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AdaptiveFacetTriangle {
    /// Local coordinates on `AdaptiveFacetMesh::POSITION_DENOMINATOR`.
    pub vertices: [[u32; 3]; 3],
    pub depth: u8,
    pub state: AdaptiveFacetState,
}
/// One source-bound facet, including every unresolved leaf. No conformity with
/// other facets or cells, physical-space error or rendering authority is implied.
pub struct AdaptiveFacetMesh {
    input: Arc<EnclosedSurfaceGraph>,
    facet: u32,
    triangles: Vec<AdaptiveFacetTriangle>,
    bytes: u64,
    _reservation: Reservation,
}
impl AdaptiveFacetMesh {
    pub const POSITION_DENOMINATOR: u32 = REFINED_GRID;
    pub fn input(&self) -> &Arc<EnclosedSurfaceGraph> {
        &self.input
    }
    pub fn source_facet(&self) -> u32 {
        self.facet
    }
    pub fn triangles(&self) -> &[AdaptiveFacetTriangle] {
        &self.triangles
    }
    pub fn referenced_bytes(&self) -> u64 {
        self.bytes
    }
}
#[derive(Clone, Copy, Default)]
struct Node {
    vertices: [[u32; 3]; 3],
    depth: u8,
}

fn longest_edge(vertices: [[u32; 3]; 3], u: usize, v: usize) -> (usize, usize, usize) {
    let mut best = (0, 1, 2);
    let mut distance = 0u64;
    for (a, b, c) in [(0, 1, 2), (1, 2, 0), (2, 0, 1)] {
        let x = u64::from(vertices[a][u].abs_diff(vertices[b][u]));
        let y = u64::from(vertices[a][v].abs_diff(vertices[b][v]));
        let next = x * x + y * y;
        if next > distance {
            best = (a, b, c);
            distance = next;
        }
    }
    best
}

fn midpoint(
    chart: &CellProjectionChart,
    a: [u32; 3],
    b: [u32; 3],
    normal: i8,
    cancel: &impl Fn() -> bool,
) -> Result<Result<[u32; 3], AdaptiveStopReason>, SurfaceError> {
    check(cancel)?;
    let axis = usize::from(chart.axis());
    let [u, v] = chart.transverse_axes();
    let mut p = [0u32; 3];
    for d in [u, v] {
        let sum = a[d].checked_add(b[d]).ok_or(SurfaceError::Limit)?;
        if sum % 2 != 0 {
            return Err(SurfaceError::Geometry);
        }
        p[d] = sum / 2;
    }
    let coordinate = |n: u32| -> Result<ExactCoordinate, SurfaceError> {
        let ratio = Rational::new(i128::from(n), i128::from(REFINED_GRID)).map_err(error)?;
        ExactCoordinate::from_rational(ratio)
            .map_err(error)?
            .ok_or(SurfaceError::Geometry)
    };
    let fiber = chart
        .fiber_bounds([coordinate(p[u])?, coordinate(p[v])?], 16, cancel)
        .map_err(error)?;
    match midpoint_height(fiber, normal)? {
        Ok(height) => {
            p[axis] = height;
            Ok(Ok(p))
        }
        Err(reason) => Ok(Err(reason)),
    }
}

fn midpoint_height(
    fiber: CellFiberBounds,
    normal: i8,
) -> Result<Result<u32, AdaptiveStopReason>, SurfaceError> {
    match fiber {
        CellFiberBounds::Empty => Ok(Err(AdaptiveStopReason::EmptyMidpoint)),
        CellFiberBounds::Whole => Ok(Err(AdaptiveStopReason::WholeMidpoint)),
        CellFiberBounds::Point { normal: n, .. } if n != normal => {
            Ok(Err(AdaptiveStopReason::OppositeMidpointNormal))
        }
        CellFiberBounds::Point {
            lower,
            upper,
            denominator,
            ..
        } => {
            let twice = denominator.checked_mul(2).ok_or(SurfaceError::Geometry)?;
            if twice == 0
                || lower > upper
                || upper > denominator
                || !REFINED_GRID.is_multiple_of(twice)
            {
                return Err(SurfaceError::Geometry);
            }
            let height = lower
                .checked_add(upper)
                .and_then(|n| n.checked_mul(REFINED_GRID / twice))
                .ok_or(SurfaceError::Geometry)?;
            Ok(Ok(height))
        }
    }
}

impl EnclosedSurfaceGraph {
    /// Subdivide one candidate facet until each proved leaf meets the requested
    /// local axial tolerance or carries an explicit unresolved reason. Neighboring
    /// facets/cells are not conformed by this operation.
    pub fn refine_facet(
        self: &Arc<Self>,
        facet: u32,
        limits: AdaptiveFacetLimits,
        cancel: impl Fn() -> bool,
    ) -> Result<AdaptiveFacetMesh, SurfaceError> {
        self.refine_facet_with_budget(facet, limits, MemoryBudget::volumes(), cancel)
    }
    pub(in crate::surface) fn refine_facet_with_budget(
        self: &Arc<Self>,
        facet: u32,
        limits: AdaptiveFacetLimits,
        budget: Arc<MemoryBudget>,
        cancel: impl Fn() -> bool,
    ) -> Result<AdaptiveFacetMesh, SurfaceError> {
        check(&cancel)?;
        if limits.tolerance_bits > 16
            || limits.max_depth > MAX_DEPTH
            || limits.max_triangles == 0
            || limits.max_triangles > MAX_TRIANGLES
            || limits.max_bytes > MAX_BYTES
        {
            return Err(SurfaceError::Limit);
        }
        let BoundedSurfaceFacet::Candidate { normal } = *self
            .facets
            .get(facet as usize)
            .ok_or(SurfaceError::Geometry)?
        else {
            return Err(SurfaceError::Geometry);
        };
        let triangle = self
            .input
            .triangles()
            .get(facet as usize)
            .ok_or(SurfaceError::Geometry)?;
        let [a, b, c] = triangle.vertices.map(|id| self.vertex_lattice_point(id));
        let initial = [
            a?.ok_or(SurfaceError::Geometry)?,
            b?.ok_or(SurfaceError::Geometry)?,
            c?.ok_or(SurfaceError::Geometry)?,
        ]
        .map(|p| p.map(|v| v << MAX_DEPTH));
        let bytes = size(
            &[(
                limits.max_triangles,
                std::mem::size_of::<AdaptiveFacetTriangle>(),
            )],
            std::mem::size_of::<AdaptiveFacetMesh>(),
        )?;
        if bytes > limits.max_bytes {
            return Err(SurfaceError::Limit);
        }
        let reservation = budget.reserve(bytes).ok_or(SurfaceError::Memory)?;
        let _scratch = budget
            .reserve(std::mem::size_of::<[Node; MAX_DEPTH as usize + 1]>() as u64)
            .ok_or(SurfaceError::Memory)?;
        let mut triangles = exact(limits.max_triangles)?;
        let mut stack = [Node::default(); MAX_DEPTH as usize + 1];
        stack[0] = Node {
            vertices: initial,
            depth: 0,
        };
        let mut pending = 1usize;
        let chart = self.input.input().input().input().edges().chart();
        let axis = usize::from(chart.axis());
        let [u, v] = chart.transverse_axes();
        let field = chart
            .graph()
            .cell()
            .interpolant()
            .ok_or(SurfaceError::Geometry)?;
        while pending > 0 {
            check(&cancel)?;
            pending -= 1;
            let node = stack[pending];
            let certificate = evaluate_grid(
                field.values(),
                axis,
                normal,
                node.vertices,
                REFINED_GRID,
                &cancel,
            )?;
            let within = if let SurfaceFacetCertificate::Certified {
                numerator,
                denominator,
            } = certificate
            {
                numerator
                    .checked_mul(1u128 << limits.tolerance_bits)
                    .ok_or(SurfaceError::Limit)?
                    <= denominator
            } else {
                false
            };
            let state = if within {
                let SurfaceFacetCertificate::Certified {
                    numerator,
                    denominator,
                } = certificate
                else {
                    return Err(SurfaceError::Geometry);
                };
                Some(AdaptiveFacetState::WithinTolerance {
                    numerator,
                    denominator,
                })
            } else if certificate == SurfaceFacetCertificate::NonPositiveProjection {
                Some(AdaptiveFacetState::Unresolved {
                    certificate,
                    reason: AdaptiveStopReason::NonPositiveProjection,
                })
            } else if node.depth == limits.max_depth {
                Some(AdaptiveFacetState::Unresolved {
                    certificate,
                    reason: AdaptiveStopReason::DepthLimit,
                })
            } else if triangles
                .len()
                .checked_add(pending)
                .and_then(|n| n.checked_add(2))
                .ok_or(SurfaceError::Limit)?
                > limits.max_triangles
            {
                Some(AdaptiveFacetState::Unresolved {
                    certificate,
                    reason: AdaptiveStopReason::TriangleLimit,
                })
            } else {
                let (a, b, c) = longest_edge(node.vertices, u, v);
                match midpoint(chart, node.vertices[a], node.vertices[b], normal, &cancel)? {
                    Ok(m) => {
                        let depth = node.depth + 1;
                        stack[pending] = Node {
                            vertices: [m, node.vertices[b], node.vertices[c]],
                            depth,
                        };
                        stack[pending + 1] = Node {
                            vertices: [node.vertices[a], m, node.vertices[c]],
                            depth,
                        };
                        pending += 2;
                        None
                    }
                    Err(reason) => Some(AdaptiveFacetState::Unresolved {
                        certificate,
                        reason,
                    }),
                }
            };
            if let Some(state) = state {
                triangles.push(AdaptiveFacetTriangle {
                    vertices: node.vertices,
                    depth: node.depth,
                    state,
                });
            }
        }
        check(&cancel)?;
        Ok(AdaptiveFacetMesh {
            input: Arc::clone(self),
            facet,
            triangles,
            bytes,
            _reservation: reservation,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn midpoint_fibers_preserve_distinct_singular_cases() {
        assert_eq!(
            midpoint_height(CellFiberBounds::Empty, 1),
            Ok(Err(AdaptiveStopReason::EmptyMidpoint))
        );
        assert_eq!(
            midpoint_height(CellFiberBounds::Whole, 1),
            Ok(Err(AdaptiveStopReason::WholeMidpoint))
        );
        let point = CellFiberBounds::Point {
            lower: 1,
            upper: 1,
            denominator: 2,
            normal: -1,
        };
        assert_eq!(
            midpoint_height(point, 1),
            Ok(Err(AdaptiveStopReason::OppositeMidpointNormal))
        );
        assert_eq!(midpoint_height(point, -1), Ok(Ok(REFINED_GRID / 2)));
    }
}
