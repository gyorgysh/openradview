// SPDX-License-Identifier: GPL-3.0-or-later
//! Complete bilinear zero contours with exact, bounded insertion unions.
use super::*;
use compare::bilinear_sign;

// A source face unions two <=18-node graphs and four <=10-point edge
// partitions: at most76 raw insertions, before numerical deduplication.
const INSERTIONS: usize = 80;
// At most four distinct boundary zeros, one singular centre, and all insertions.
const NODES: usize = INSERTIONS + 5;

#[derive(Clone, Copy, Debug)]
pub struct ContourNode {
    coordinates: [ExactCoordinate; 2],
    boundary: u8,
}
impl ContourNode {
    pub fn coordinates(&self) -> &[ExactCoordinate; 2] {
        &self.coordinates
    }
    /// Bits 0/1 are u=0/1, bits 2/3 are v=0/1.
    pub fn boundary(&self) -> u8 {
        self.boundary
    }
}
/// Directed with F>0 on the right in the face's ascending (u,v) coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContourSegment {
    pub from: u8,
    pub to: u8,
}
impl ContourSegment {
    /// Boundary orientation for the cell's surface normal pointing out of F>0.
    /// Adjacent cells reverse the shared contour's direction, not its geometry.
    pub fn for_cell_face(self, axis: usize, upper: bool) -> Option<Self> {
        if axis >= 3 {
            return None;
        }
        Some(if upper == (axis == 1) {
            Self {
                from: self.to,
                to: self.from,
            }
        } else {
            self
        })
    }
}
/// Exact points and connections on a bilinear face. Curved segments follow the
/// face's zero curve, not the straight chord between their endpoints. A whole
/// zero face is two-dimensional and has no invented one-dimensional contour.
pub struct FaceArrangement {
    field: Bilinear,
    nodes: [Option<ContourNode>; NODES],
    count: usize,
    segments: [Option<ContourSegment>; NODES],
    segment_count: usize,
    whole: bool,
}
impl FaceArrangement {
    pub const MAX_INSERTIONS: usize = INSERTIONS;
    pub fn field(&self) -> Bilinear {
        self.field
    }
    pub fn is_whole_zero(&self) -> bool {
        self.whole
    }
    pub fn nodes(&self) -> impl Iterator<Item = (u8, &ContourNode)> {
        self.nodes
            .iter()
            .enumerate()
            .filter_map(|(i, v)| v.as_ref().map(|v| (i as u8, v)))
    }
    pub fn node(&self, id: u8) -> Option<&ContourNode> {
        self.nodes.get(usize::from(id))?.as_ref()
    }
    pub fn segments(&self) -> impl Iterator<Item = &ContourSegment> {
        self.segments.iter().flatten()
    }
    fn add(
        &mut self,
        coordinates: [ExactCoordinate; 2],
        cancel: &impl Fn() -> bool,
    ) -> Result<(), FieldError> {
        let normalize = |value: ExactCoordinate| {
            value
                .rational_value()
                .map_or(Ok(value), rational_coordinate)
        };
        let coordinates = [normalize(coordinates[0])?, normalize(coordinates[1])?];
        for index in 0..self.count {
            check_cancel(cancel)?;
            let node = self.nodes[index].as_mut().ok_or(FieldError::Arithmetic)?;
            if same_point(node.coordinates, coordinates)? {
                // Equality has already been proved numerically. Choosing the
                // same supplied expression also makes later approximations
                // independent of which incident cell was visited first.
                for (prior, incoming) in node.coordinates.iter_mut().zip(coordinates) {
                    if representation(incoming) < representation(*prior) {
                        *prior = incoming;
                    }
                }
                return Ok(());
            }
        }
        let mut boundary = 0;
        for (axis, value) in coordinates.into_iter().enumerate() {
            for upper in [false, true] {
                boundary |=
                    u8::from(value.compare(ExactCoordinate::endpoint(upper))? == Ordering::Equal)
                        << (2 * axis + usize::from(upper));
            }
        }
        *self.nodes.get_mut(self.count).ok_or(FieldError::Capacity)? = Some(ContourNode {
            coordinates,
            boundary,
        });
        self.count += 1;
        Ok(())
    }
    fn canonicalize(&mut self, cancel: &impl Fn() -> bool) -> Result<(), FieldError> {
        for i in 1..self.count {
            let mut j = i;
            while j > 0 {
                check_cancel(cancel)?;
                let left = self.nodes[j - 1].ok_or(FieldError::Arithmetic)?.coordinates;
                let right = self.nodes[j].ok_or(FieldError::Arithmetic)?.coordinates;
                let first = left[0].compare(right[0])?;
                if first == Ordering::Less
                    || (first == Ordering::Equal && left[1].compare(right[1])? != Ordering::Greater)
                {
                    break;
                }
                self.nodes.swap(j - 1, j);
                j -= 1;
            }
        }
        Ok(())
    }
    fn connect(
        &mut self,
        mut member: impl FnMut([ExactCoordinate; 2]) -> Result<bool, FieldError>,
        tangent: [i32; 4],
        cancel: &impl Fn() -> bool,
    ) -> Result<(), FieldError> {
        let mut previous: Option<usize> = None;
        for id in 0..self.count {
            check_cancel(cancel)?;
            let point = self.nodes[id].ok_or(FieldError::Arithmetic)?.coordinates;
            if !member(point)? {
                continue;
            }
            if let Some(prior) = previous {
                let before = self.nodes[prior].ok_or(FieldError::Arithmetic)?.coordinates;
                let a = bilinear_sign(tangent, before)?;
                let b = bilinear_sign(tangent, point)?;
                if (a == 0 && b == 0) || (a != 0 && b != 0 && a != b) {
                    return Err(FieldError::Arithmetic);
                }
                let (from, to) = if a + b > 0 { (prior, id) } else { (id, prior) };
                *self
                    .segments
                    .get_mut(self.segment_count)
                    .ok_or(FieldError::Capacity)? = Some(ContourSegment {
                    from: from as u8,
                    to: to as u8,
                });
                self.segment_count += 1;
            }
            previous = Some(id);
        }
        Ok(())
    }
}
fn same_point(a: [ExactCoordinate; 2], b: [ExactCoordinate; 2]) -> Result<bool, FieldError> {
    Ok(a[0].compare(b[0])? == Ordering::Equal && a[1].compare(b[1])? == Ordering::Equal)
}
fn rational_coordinate(value: Rational) -> Result<ExactCoordinate, FieldError> {
    ExactCoordinate::parameter(AlgebraicRoot(Root::Rational(value)))?.ok_or(FieldError::Arithmetic)
}
fn representation(value: ExactCoordinate) -> [i128; 10] {
    let mut key = match value.parameter.0 {
        Root::Rational(r) => [
            0,
            i128::from(r.numerator),
            i128::from(r.denominator),
            0,
            0,
            0,
            0,
            0,
            0,
            0,
        ],
        Root::Surd { a, b, c, upper } => [
            1,
            i128::from(a),
            i128::from(b),
            i128::from(c),
            i128::from(upper),
            0,
            0,
            0,
            0,
            0,
        ],
    };
    key[6..].copy_from_slice(&[
        i128::from(value.numerator.constant),
        i128::from(value.numerator.slope),
        i128::from(value.denominator.constant),
        i128::from(value.denominator.slope),
    ]);
    key
}
fn coefficients(field: Bilinear) -> [i32; 4] {
    let [a, b, c, d] = field.values.map(i32::from);
    [a, b - a, d - a, a - b + c - d]
}
impl Bilinear {
    /// Insert all required points together. The caller owns source identity,
    /// adjacency and the union around entire zero lattice edges. Raw insertion
    /// count is bounded before deduplication; cancellation publishes no partial
    /// result. Points must lie exactly on this face's zero set.
    pub fn arrangement(
        self,
        insertions: &[[ExactCoordinate; 2]],
        cancel: impl Fn() -> bool,
    ) -> Result<FaceArrangement, FieldError> {
        check_cancel(&cancel)?;
        if insertions.len() > INSERTIONS {
            return Err(FieldError::Capacity);
        }
        let [a, b, c, d] = coefficients(self);
        let mut result = FaceArrangement {
            field: self,
            nodes: [None; NODES],
            count: 0,
            segments: [None; NODES],
            segment_count: 0,
            whole: a == 0 && b == 0 && c == 0 && d == 0,
        };
        // Cyclic boundary coordinates agree with Bilinear::values/FaceTopology.
        let corners = [[false, false], [true, false], [true, true], [false, true]];
        for edge in 0..4 {
            check_cancel(&cancel)?;
            let start = corners[edge].map(ExactCoordinate::endpoint);
            let end = corners[(edge + 1) % 4].map(ExactCoordinate::endpoint);
            match edge_zero(self.values[edge], self.values[(edge + 1) % 4])
                .ok_or(FieldError::Arithmetic)?
            {
                EdgeZero::Empty => {}
                EdgeZero::Entire => {
                    result.add(start, &cancel)?;
                    result.add(end, &cancel)?;
                }
                EdgeZero::Point(t) => {
                    let axis = usize::from(!edge.is_multiple_of(2));
                    let t = if edge >= 2 {
                        Rational::new(
                            i128::from(t.denominator) - i128::from(t.numerator),
                            i128::from(t.denominator),
                        )?
                    } else {
                        t
                    };
                    let mut point = start;
                    point[axis] = rational_coordinate(t)?;
                    result.add(point, &cancel)?;
                }
            }
        }
        let determinant = i64::from(a) * i64::from(d) - i64::from(b) * i64::from(c);
        let centre = if d != 0 && determinant == 0 {
            let u = ExactCoordinate::parameter(AlgebraicRoot(Root::Rational(Rational::new(
                -i128::from(c),
                i128::from(d),
            )?)))?;
            let v = ExactCoordinate::parameter(AlgebraicRoot(Root::Rational(Rational::new(
                -i128::from(b),
                i128::from(d),
            )?)))?;
            if let (Some(u), Some(v)) = (u, v) {
                result.add([u, v], &cancel)?;
            }
            Some([u, v])
        } else {
            None
        };
        for point in insertions {
            check_cancel(&cancel)?;
            if bilinear_sign([a, b, c, d], *point)? != 0 {
                return Err(FieldError::Contour);
            }
            result.add(*point, &cancel)?;
        }
        result.canonicalize(&cancel)?;
        if result.whole {
            check_cancel(&cancel)?;
            return Ok(result);
        }
        if let Some([u, v]) = centre {
            if let Some(v) = v {
                result.connect(
                    |point| Ok(point[1].compare(v)? == Ordering::Equal),
                    [-c, -d, 0, 0],
                    &cancel,
                )?;
            }
            if let Some(u) = u {
                result.connect(
                    |point| Ok(point[0].compare(u)? == Ordering::Equal),
                    [b, 0, d, 0],
                    &cancel,
                )?;
            }
        } else if d != 0 {
            for sign in [-1, 1] {
                result.connect(
                    |point| Ok(bilinear_sign([c, d, 0, 0], point)? == sign),
                    [-c, -d, 0, 0],
                    &cancel,
                )?;
            }
        } else if c != 0 {
            result.connect(|_| Ok(true), [-c, 0, 0, 0], &cancel)?;
        } else if b != 0 {
            result.connect(|_| Ok(true), [b, 0, 0, 0], &cancel)?;
        }
        check_cancel(&cancel)?;
        Ok(result)
    }
}
impl CriticalGraph {
    /// Build one face from both incident graphs, validating the opposite face's
    /// samples. Additional points include the owning extractor's full zero-edge
    /// insertion union. This local mathematical object cannot prove source
    /// adjacency, close unknown boundaries or resolve a whole zero face in 3D.
    pub fn face_arrangement(
        &self,
        axis: usize,
        upper: bool,
        neighbor: Option<&Self>,
        additional: &[[ExactCoordinate; 2]],
        cancel: impl Fn() -> bool,
    ) -> Result<FaceArrangement, FieldError> {
        check_cancel(&cancel)?;
        let face = self.field.face(axis, upper).ok_or(FieldError::Bounds)?;
        if neighbor.is_some_and(|n| n.field.face(axis, !upper) != Some(face)) {
            return Err(FieldError::Contour);
        }
        let mut insertions = [[ExactCoordinate::endpoint(false); 2]; INSERTIONS];
        let mut count = 0;
        for (graph, side) in [(Some(self), upper), (neighbor, !upper)] {
            let Some(graph) = graph else {
                continue;
            };
            for (_, node) in graph.nodes() {
                check_cancel(&cancel)?;
                if node.boundary() & (1 << (2 * axis + usize::from(side))) == 0 {
                    continue;
                }
                let [u, v] = transverse_axes(axis);
                *insertions.get_mut(count).ok_or(FieldError::Capacity)? =
                    [node.coordinates[u], node.coordinates[v]];
                count += 1;
            }
        }
        for point in additional {
            check_cancel(&cancel)?;
            *insertions.get_mut(count).ok_or(FieldError::Capacity)? = *point;
            count += 1;
        }
        face.arrangement(&insertions[..count], cancel)
    }
}

#[cfg(test)]
mod tests;
