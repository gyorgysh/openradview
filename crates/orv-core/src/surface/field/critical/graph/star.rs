// SPDX-License-Identifier: GPL-3.0-or-later
//! Oriented anchor incidence, preserving ordinary double contacts and clipping.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnchorRay {
    pub edge: u8,
    pub to: u8,
    pub axis: u8,
    pub positive: bool,
}
/// Cyclic order of retained anchor rays around the outward normal of F>=0.
/// Missing outside rays remain clipped; this is not a complete surface link.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnchorRing {
    rays: [AnchorRay; 4],
    count: usize,
}
impl AnchorRing {
    fn empty() -> Self {
        Self {
            rays: [AnchorRay {
                edge: 0,
                to: 0,
                axis: 0,
                positive: false,
            }; 4],
            count: 0,
        }
    }
    fn push(&mut self, ray: Option<AnchorRay>) -> Result<(), FieldError> {
        if let Some(ray) = ray {
            *self.rays.get_mut(self.count).ok_or(FieldError::Capacity)? = ray;
            self.count += 1;
        }
        Ok(())
    }
    pub fn rays(&self) -> &[AnchorRay] {
        &self.rays[..self.count]
    }
}
/// Local anchor ordering only. The original vertex is shared by both rings of
/// an ordinary double contact; no singularity is moved or regularized. Family
/// resolution, cell-face contours and complete patch connectivity are separate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnchorStar {
    UnresolvedFamily {
        node: u8,
        boundary: u8,
    },
    Regular {
        node: u8,
        boundary: u8,
        gradient_signs: [i8; 3],
        ring: AnchorRing,
    },
    OrdinaryDouble {
        node: u8,
        boundary: u8,
        /// Signs of Fxy, Fxz, Fyz at the original node.
        mixed_signs: [i8; 3],
        rings: [AnchorRing; 2],
    },
}
impl AnchorStar {
    /// Empty only for UnresolvedFamily, never a certified absence of a surface.
    pub fn rings(&self) -> &[AnchorRing] {
        match self {
            Self::UnresolvedFamily { .. } => &[],
            Self::Regular { ring, .. } => std::slice::from_ref(ring),
            Self::OrdinaryDouble { rings, .. } => rings,
        }
    }
}
fn power(field: Trilinear) -> [i32; 8] {
    let mut p = field.values.map(i32::from);
    for axis in 0..3 {
        for i in 0..8 {
            if i & (1 << axis) != 0 {
                p[i] -= p[i ^ (1 << axis)];
            }
        }
    }
    p
}
fn gradient(
    p: [i32; 8],
    at: [ExactCoordinate; 3],
    cancel: &impl Fn() -> bool,
) -> Result<[i8; 3], FieldError> {
    let mut signs = [0; 3];
    for (axis, sign) in signs.iter_mut().enumerate() {
        check_cancel(cancel)?;
        let [u, v] = transverse_axes(axis);
        let a = 1 << axis;
        // Each original power coefficient is bounded by 8*255=2040, including
        // the cubic term. The existing exact bilinear predicate admits these.
        *sign = compare::bilinear_sign(
            [p[a], p[a | (1 << u)], p[a | (1 << v)], p[7]],
            [at[u], at[v]],
        )?;
    }
    Ok(signs)
}
fn slot(axis: usize, positive: bool) -> usize {
    2 * axis + usize::from(!positive)
}
impl CriticalGraph {
    pub fn anchor_star(&self, id: u8, cancel: impl Fn() -> bool) -> Result<AnchorStar, FieldError> {
        check_cancel(&cancel)?;
        let node = self.node(id).ok_or(FieldError::Bounds)?;
        let boundary = node.boundary;
        if self.requires_family_resolution() {
            check_cancel(&cancel)?;
            return Ok(AnchorStar::UnresolvedFamily { node: id, boundary });
        }
        let p = power(self.field);
        let signs = gradient(p, node.coordinates, &cancel)?;
        let mut rays = [None; 6];
        for (index, edge) in self.edges.iter().enumerate() {
            check_cancel(&cancel)?;
            let Some(edge) = edge else { continue };
            let to = if edge.from == id {
                edge.to
            } else if edge.to == id {
                edge.from
            } else {
                continue;
            };
            let next = self.node(to).ok_or(FieldError::Arithmetic)?;
            let line = self
                .lines
                .get(usize::from(edge.line))
                .and_then(Option::as_ref)
                .ok_or(FieldError::Arithmetic)?;
            let axis = line.axis();
            if axis >= 3 || signs[axis] != 0 || !line.contains(node)? || !line.contains(next)? {
                return Err(FieldError::Arithmetic);
            }
            let positive =
                match node.coordinates[axis].compare_same_field(next.coordinates[axis])? {
                    Ordering::Less => true,
                    Ordering::Greater => false,
                    Ordering::Equal => return Err(FieldError::Arithmetic),
                };
            let place = &mut rays[slot(axis, positive)];
            if place.is_some() {
                return Err(FieldError::Arithmetic);
            }
            *place = Some(AnchorRay {
                edge: index as u8,
                to,
                axis: axis as u8,
                positive,
            });
        }
        // A zero derivative at a zero node proves the complete axial line. All
        // of its in-cell directions must survive the finite graph construction.
        for (axis, sign) in signs.into_iter().enumerate() {
            for positive in [false, true] {
                check_cancel(&cancel)?;
                let outside = boundary & (1 << (2 * axis + usize::from(positive))) != 0;
                if rays[slot(axis, positive)].is_some() != (sign == 0 && !outside) {
                    return Err(FieldError::Arithmetic);
                }
            }
        }
        let mut rings = [AnchorRing::empty(); 2];
        if signs != [0; 3] {
            let order = RegularTangents::new(signs)?;
            let axis = signs
                .iter()
                .position(|s| *s == 0)
                .ok_or(FieldError::Arithmetic)?;
            let reference = TangentRay::Axis {
                axis: axis as u8,
                positive: true,
            };
            let tangent = |ray: AnchorRay| TangentRay::Axis {
                axis: ray.axis,
                positive: ray.positive,
            };
            for ray in rays.into_iter().flatten() {
                check_cancel(&cancel)?;
                rings[0].push(Some(ray))?;
                let mut at = rings[0].count - 1;
                while at > 0 {
                    check_cancel(&cancel)?;
                    match order.compare(
                        reference,
                        tangent(rings[0].rays[at]),
                        tangent(rings[0].rays[at - 1]),
                    )? {
                        Ordering::Less => rings[0].rays.swap(at, at - 1),
                        Ordering::Equal => return Err(FieldError::Arithmetic),
                        Ordering::Greater => break,
                    }
                    at -= 1;
                }
            }
            check_cancel(&cancel)?;
            return Ok(AnchorStar::Regular {
                node: id,
                boundary,
                gradient_signs: signs,
                ring: rings[0],
            });
        }
        let mut mixed = [0; 3];
        for (i, (a, b, c)) in [(0, 1, 2), (0, 2, 1), (1, 2, 0)].into_iter().enumerate() {
            check_cancel(&cancel)?;
            mixed[i] = compare::bilinear_sign(
                [p[(1 << a) | (1 << b)], p[7], 0, 0],
                [node.coordinates[c], ExactCoordinate::endpoint(false)],
            )?;
        }
        if mixed.contains(&0) {
            return Err(FieldError::Arithmetic);
        }
        // In translated coordinates F=e*xy+f*xz+g*yz+h*xyz. The local map
        // X=x/(1+h*x/g) reduces it to the quadratic cone with a positive scale
        // and Jacobian. It fixes anchor rays and retains both sheets at the node.
        let sigma = mixed[0] * mixed[1] * mixed[2];
        let group = [1, sigma * mixed[0], sigma * mixed[1]];
        for (ring, multiplier) in rings.iter_mut().zip([1, -1]) {
            check_cancel(&cancel)?;
            let directions = group.map(|s| s * multiplier > 0);
            // (-grad F) cross the x radial ray has y/z signs -f and e.
            let next_y = mixed[1] * group[1] * multiplier < 0;
            for axis in if next_y { [0, 1, 2] } else { [0, 2, 1] } {
                ring.push(rays[slot(axis, directions[axis])])?;
            }
        }
        check_cancel(&cancel)?;
        Ok(AnchorStar::OrdinaryDouble {
            node: id,
            boundary,
            mixed_signs: mixed,
            rings,
        })
    }
}

#[cfg(test)]
mod tests;
