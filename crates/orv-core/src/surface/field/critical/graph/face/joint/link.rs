// SPDX-License-Identifier: GPL-3.0-or-later
//! Exact local rotation systems, including equal-tangent regular contacts.
use super::*;
use crate::surface::field::critical::graph::compare::bounded_bilinear_sign;

#[derive(Clone, Copy)]
struct Ray {
    octant: u8,
    du: i8,
    orientation: i8,
    owner: Option<usize>,
}
fn octant(u: i8, v: i8) -> Result<u8, FieldError> {
    match (u, v) {
        (1, 0) => Ok(0),
        (1, 1) => Ok(1),
        (0, 1) => Ok(2),
        (-1, 1) => Ok(3),
        (-1, 0) => Ok(4),
        (-1, -1) => Ok(5),
        (0, -1) => Ok(6),
        (1, -1) => Ok(7),
        _ => Err(FieldError::Arithmetic),
    }
}
impl JointSubdivision {
    fn ray(&self, id: u8) -> Result<Ray, FieldError> {
        let half = self.half_edge(id).ok_or(FieldError::Arithmetic)?;
        let edge = self.edge(half.edge).ok_or(FieldError::Arithmetic)?;
        let at = self
            .node(half.from)
            .ok_or(FieldError::Arithmetic)?
            .coordinates;
        if matches!(edge.geometry, CommonZero::Axis { .. }) {
            let end = self
                .node(half.to)
                .ok_or(FieldError::Arithmetic)?
                .coordinates;
            let u = order_sign(end[0].compare(at[0])?);
            let v = order_sign(end[1].compare(at[1])?);
            if u != 0 && v != 0 {
                return Err(FieldError::Arithmetic);
            }
            return Ok(Ray {
                octant: octant(u, v)?,
                du: u,
                orientation: 0,
                owner: None,
            });
        }
        let owner = if edge.curves & 1 != 0 {
            0
        } else if edge.curves & 2 != 0 {
            1
        } else {
            return Err(FieldError::Arithmetic);
        };
        let orientation = if half.from == edge.from {
            edge.right_signs[owner]
        } else {
            -edge.right_signs[owner]
        };
        let [_, b, c, d] = coefficients(self.fields[owner]);
        let u = -bilinear_sign([c, d, 0, 0], at)? * orientation;
        let v = bilinear_sign([b, 0, d, 0], at)? * orientation;
        if u == 0 || v == 0 {
            return Err(FieldError::Arithmetic);
        }
        Ok(Ray {
            octant: octant(u, v)?,
            du: u,
            orientation,
            owner: Some(owner),
        })
    }
    pub(in crate::surface::field) fn half_octant(&self, id: u8) -> Result<u8, FieldError> {
        Ok(self.ray(id)?.octant)
    }
    pub(in crate::surface::field) fn angle_compare(
        &self,
        a: u8,
        b: u8,
    ) -> Result<Ordering, FieldError> {
        let ra = self.ray(a)?;
        let rb = self.ray(b)?;
        if ra.octant != rb.octant {
            return Ok(ra.octant.cmp(&rb.octant));
        }
        // Axis rays are unique after common-component merging. Irreducible
        // curved/oblique graph tangents have both components nonzero.
        let [Some(a_owner), Some(b_owner)] = [ra.owner, rb.owner] else {
            return Err(FieldError::Arithmetic);
        };
        let half = self.half_edge(a).ok_or(FieldError::Arithmetic)?;
        if self.half_edge(b).is_none_or(|h| h.from != half.from) {
            return Err(FieldError::Arithmetic);
        }
        let at = self
            .node(half.from)
            .ok_or(FieldError::Arithmetic)?
            .coordinates;
        let [_, f_u, f_v, f_uv] = coefficients(self.fields[a_owner]).map(i64::from);
        let [_, g_u, g_v, g_uv] = coefficients(self.fields[b_owner]).map(i64::from);
        let cross_coefficients = [
            f_u * g_v - f_v * g_u,
            f_u * g_uv - f_uv * g_u,
            f_uv * g_v - f_v * g_uv,
            0,
        ];
        let mut cross = [0i32; 4];
        for (out, value) in cross.iter_mut().zip(cross_coefficients) {
            *out = i32::try_from(value).map_err(|_| FieldError::Arithmetic)?;
        }
        let jacobian = bounded_bilinear_sign(cross, at)?;
        if jacobian != 0 {
            return Ok(0.cmp(&(jacobian * ra.orientation * rb.orientation)));
        }
        // At a regular tangent contact, vG-vF=R/(Fv*Gv), where R has a
        // double root. Its quadratic term orders equal-tangent rays. The
        // outgoing secant cross has sign(du^3*(vG''-vF'')); left/right reverse.
        let quadratic = f_u * g_uv - f_uv * g_u;
        let [_, _, c, d] = coefficients(self.fields[a_owner]);
        let f_sign = bilinear_sign([c, d, 0, 0], at)?;
        let [_, _, c, d] = coefficients(self.fields[b_owner]);
        let g_sign = bilinear_sign([c, d, 0, 0], at)?;
        let sign = ra.du * quadratic.signum() as i8 * f_sign * g_sign;
        if sign == 0 {
            return Err(FieldError::Arithmetic);
        }
        Ok(0.cmp(&sign))
    }
    pub(super) fn link(&mut self, cancel: &impl Fn() -> bool) -> Result<(), FieldError> {
        for i in 1..self.edge_count {
            let mut j = i;
            while j > 0 {
                check_cancel(cancel)?;
                let a = self.edges[j - 1].ok_or(FieldError::Arithmetic)?;
                let b = self.edges[j].ok_or(FieldError::Arithmetic)?;
                if (a.from, a.to, geometry_key(a.geometry))
                    <= (b.from, b.to, geometry_key(b.geometry))
                {
                    break;
                }
                self.edges.swap(j - 1, j);
                j -= 1;
            }
        }
        let mut outgoing = [[None::<u8>; 8]; JOINT_NODES];
        let mut degree = [0usize; JOINT_NODES];
        let mut signs = [None::<[i8; 2]>; JOINT_HALVES];
        for i in 0..self.edge_count {
            check_cancel(cancel)?;
            let edge = self.edges[i].ok_or(FieldError::Arithmetic)?;
            let inside = if edge.boundary.is_some() {
                Some([self.boundary_sign(edge, 0)?, self.boundary_sign(edge, 1)?])
            } else {
                None
            };
            for reverse in [false, true] {
                let id = 2 * i + usize::from(reverse);
                let (from, to) = if reverse {
                    (edge.to, edge.from)
                } else {
                    (edge.from, edge.to)
                };
                self.halves[id] = Some(PatchHalfEdge {
                    edge: i as u8,
                    from,
                    to,
                    twin: (id ^ 1) as u8,
                    next: 0,
                    region: None,
                });
                let mut left = [0i8; 2];
                for (field, sign) in left.iter_mut().enumerate() {
                    *sign = if edge.curves & (1 << field) != 0 {
                        if reverse {
                            edge.right_signs[field]
                        } else {
                            -edge.right_signs[field]
                        }
                    } else if edge.boundary.is_some() {
                        inside.ok_or(FieldError::Arithmetic)?[field]
                    } else {
                        self.along_sign(edge, field)?
                    };
                }
                signs[id] = if edge.boundary.is_some() {
                    if edge.inside_forward != reverse {
                        if Some(left) != inside {
                            return Err(FieldError::Arithmetic);
                        }
                        inside
                    } else {
                        None
                    }
                } else {
                    Some(left)
                };
                let node = usize::from(from);
                let count = degree[node];
                if count >= 8 {
                    return Err(FieldError::Capacity);
                }
                let mut at = count;
                while at > 0 {
                    let prior = outgoing[node][at - 1].ok_or(FieldError::Arithmetic)?;
                    if self.angle_compare(prior, id as u8)? == Ordering::Less {
                        break;
                    }
                    outgoing[node][at] = outgoing[node][at - 1];
                    at -= 1;
                }
                outgoing[node][at] = Some(id as u8);
                degree[node] += 1;
            }
        }
        for i in 0..2 * self.edge_count {
            check_cancel(cancel)?;
            let half = self.halves[i].ok_or(FieldError::Arithmetic)?;
            let node = usize::from(half.to);
            let count = degree[node];
            let at = outgoing[node][..count]
                .iter()
                .position(|h| *h == Some(half.twin))
                .ok_or(FieldError::Arithmetic)?;
            let next = outgoing[node][(at + count - 1) % count].ok_or(FieldError::Arithmetic)?;
            self.halves[i].as_mut().ok_or(FieldError::Arithmetic)?.next = next;
        }
        let mut visited = [false; JOINT_HALVES];
        for start in 0..2 * self.edge_count {
            if visited[start] {
                continue;
            }
            check_cancel(cancel)?;
            let sign = signs[start];
            let region = if let Some(s) = sign {
                if self.region_count >= JOINT_REGIONS {
                    return Err(FieldError::Capacity);
                }
                for (field, value) in self.fields.iter().zip(s) {
                    if value == 0 && field.values != [0; 4] {
                        return Err(FieldError::Arithmetic);
                    }
                }
                Some(self.region_count as u8)
            } else {
                None
            };
            let mut at = start;
            let mut count = 0;
            loop {
                check_cancel(cancel)?;
                if visited[at] || signs[at] != sign {
                    return Err(FieldError::Arithmetic);
                }
                visited[at] = true;
                count += 1;
                let half = self.halves[at].as_mut().ok_or(FieldError::Arithmetic)?;
                half.region = region;
                at = usize::from(half.next);
                if at == start {
                    break;
                }
                if count >= 2 * self.edge_count {
                    return Err(FieldError::Arithmetic);
                }
            }
            if let Some(signs) = sign {
                self.regions[self.region_count] = Some(JointRegion {
                    first: start as u8,
                    edge_count: count as u16,
                    signs,
                });
                self.region_count += 1;
            }
        }
        Ok(())
    }
}
