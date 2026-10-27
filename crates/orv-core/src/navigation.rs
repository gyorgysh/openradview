// SPDX-License-Identifier: GPL-3.0-or-later
//! Header-only physical slice navigation. No pixel materialization or registration.
use crate::{
    resources::{MemoryBudget, Reservation},
    spatial::GeometryError,
    study::{Geometry, Series},
    vector::*,
};

pub(crate) struct SpatialFrame {
    pub size: [u32; 2],
    pub geometry: Option<Geometry>,
}
pub struct NavigationStack {
    reference: Geometry,
    size: [u32; 2],
    normal: V3,
    positions: Vec<V3>,
    sorted: Vec<(f64, usize)>,
    step: f64,
    _memory: Reservation,
}
fn unit_normal(geometry: &Geometry) -> V3 {
    let normal = geometry.normal();
    mul(normal, 1. / norm(normal))
}
impl NavigationStack {
    /// Worker-only: validate every declared plane without decoding its pixels.
    pub fn prepare(series: &Series, cancelled: impl Fn() -> bool) -> Result<Self, GeometryError> {
        Self::build(
            series.frame_count(),
            |index| {
                if let Some(source) = &series.source {
                    source.navigation_frame(index)
                } else {
                    series.frames.get(index).map(|frame| SpatialFrame {
                        size: [frame.width, frame.height],
                        geometry: frame.geometry.clone(),
                    })
                }
            },
            cancelled,
        )
    }
    fn build(
        count: usize,
        frame: impl Fn(usize) -> Option<SpatialFrame>,
        cancelled: impl Fn() -> bool,
    ) -> Result<Self, GeometryError> {
        crate::resources::refresh_memory_policy();
        Self::build_with_budget(count, frame, cancelled, MemoryBudget::volumes())
    }
    fn build_with_budget(
        count: usize,
        frame: impl Fn(usize) -> Option<SpatialFrame>,
        cancelled: impl Fn() -> bool,
        budget: std::sync::Arc<MemoryBudget>,
    ) -> Result<Self, GeometryError> {
        if cancelled() {
            return Err(GeometryError::Cancelled);
        }
        if count < 2 {
            return Err(GeometryError::TooFew);
        }
        let bytes = (count as u64)
            .checked_mul(40)
            .and_then(|bytes| bytes.checked_add(1024))
            .filter(|bytes| *bytes <= 32 * 1024 * 1024)
            .ok_or(GeometryError::Limit)?;
        let memory = budget.reserve(bytes).ok_or(GeometryError::Limit)?;
        let first = frame(0).ok_or(GeometryError::Missing)?;
        if first.size.contains(&0) {
            return Err(GeometryError::Dimensions);
        }
        let reference = first.geometry.ok_or(GeometryError::Missing)?;
        reference.validate()?;
        if reference
            .frame_of_reference
            .as_ref()
            .is_none_or(|value| value.is_empty() || value.len() > 128)
        {
            return Err(GeometryError::Reference);
        }
        let normal = unit_normal(&reference);
        let mut positions = Vec::with_capacity(count);
        let mut sorted = Vec::with_capacity(count);
        for index in 0..count {
            if cancelled() {
                return Err(GeometryError::Cancelled);
            }
            let item = frame(index).ok_or(GeometryError::Missing)?;
            if item.size != first.size {
                return Err(GeometryError::Dimensions);
            }
            let geometry = item.geometry.ok_or(GeometryError::Missing)?;
            geometry.validate()?;
            if geometry.frame_of_reference != reference.frame_of_reference
                || geometry.nifti_transform != reference.nifti_transform
            {
                return Err(GeometryError::Reference);
            }
            if geometry
                .orientation
                .iter()
                .zip(reference.orientation)
                .any(|(a, b)| (a - b).abs() > 1e-4)
            {
                return Err(GeometryError::Orientation);
            }
            if geometry
                .spacing
                .iter()
                .zip(reference.spacing)
                .any(|(a, b)| (a - b).abs() > 1e-5 * b.max(1.))
            {
                return Err(GeometryError::Spacing);
            }
            positions.push(geometry.position);
            let location = dot(geometry.position, normal);
            if !location.is_finite() {
                return Err(GeometryError::Missing);
            }
            sorted.push((location, index));
        }
        sorted.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
        let origin = positions[sorted[0].1];
        let delta = mul(
            sub(positions[sorted[count - 1].1], origin),
            1. / (count - 1) as f64,
        );
        let step = dot(delta, normal);
        if !step.is_finite() || !norm(delta).is_finite() || step <= 1e-5 {
            return Err(GeometryError::Irregular);
        }
        for (order, (_, index)) in sorted.iter().enumerate() {
            if cancelled() {
                return Err(GeometryError::Cancelled);
            }
            if norm(sub(
                positions[*index],
                add(origin, mul(delta, order as f64)),
            )) > 1e-3 * norm(delta).max(1.)
            {
                return Err(GeometryError::Irregular);
            }
        }
        Ok(Self {
            reference,
            size: first.size,
            normal,
            positions,
            sorted,
            step,
            _memory: memory,
        })
    }
    pub fn compatible(&self, other: &Self) -> bool {
        self.reference.frame_of_reference == other.reference.frame_of_reference
            && self.reference.nifti_transform == other.reference.nifti_transform
            && norm(cross(self.normal, other.normal)) <= 1e-4
    }
    pub fn center(&self, index: usize) -> Option<V3> {
        let position = *self.positions.get(index)?;
        let (u, v) = self.reference.directions();
        let center = add(
            position,
            add(
                mul(
                    u,
                    f64::from(self.size[0] - 1) / 2. * self.reference.spacing[1],
                ),
                mul(
                    v,
                    f64::from(self.size[1] - 1) / 2. * self.reference.spacing[0],
                ),
            ),
        );
        finite(center).then_some(center)
    }
    /// Nearest plane containing the projected point; no index matching or extrapolation.
    pub fn nearest(&self, point: V3) -> Option<usize> {
        if !finite(point) {
            return None;
        }
        let normal = self.normal;
        let location = dot(point, normal);
        if location < self.sorted.first()?.0 - self.step / 2.
            || location > self.sorted.last()?.0 + self.step / 2.
        {
            return None;
        }
        let next = self
            .sorted
            .partition_point(|row| row.0 < location)
            .min(self.sorted.len() - 1);
        let previous = next.saturating_sub(1);
        let index = if (self.sorted[previous].0 - location).abs()
            <= (self.sorted[next].0 - location).abs()
        {
            self.sorted[previous].1
        } else {
            self.sorted[next].1
        };
        let delta = sub(point, self.positions[index]);
        let projected = sub(delta, mul(normal, dot(delta, normal)));
        let (u, v) = self.reference.directions();
        let pixel = [
            dot(projected, u) / self.reference.spacing[1],
            dot(projected, v) / self.reference.spacing[0],
        ];
        pixel
            .iter()
            .zip(self.size)
            .all(|(p, side)| *p >= -1e-5 && *p <= f64::from(side - 1) + 1e-5)
            .then_some(index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn stack(
        positions: &[[f64; 3]],
        size: [u32; 2],
        spacing: [f64; 2],
        reference: &str,
    ) -> Result<NavigationStack, GeometryError> {
        NavigationStack::build(
            positions.len(),
            |i| {
                Some(SpatialFrame {
                    size,
                    geometry: Some(Geometry {
                        position: positions[i],
                        orientation: [1., 0., 0., 0., 1., 0.],
                        spacing,
                        frame_of_reference: Some(reference.into()),
                        nifti_transform: None,
                    }),
                })
            },
            || false,
        )
    }
    #[test]
    fn linking_uses_physical_points_across_reverse_order_spacing_and_partial_overlap() {
        let source = stack(
            &[[0., 0., 0.], [0., 0., 2.], [0., 0., 4.]],
            [5, 5],
            [1.; 2],
            "generated",
        )
        .unwrap();
        let target = stack(
            &[
                [0., 0., 4.],
                [0., 0., 3.],
                [0., 0., 2.],
                [0., 0., 1.],
                [0., 0., 0.],
            ],
            [3, 3],
            [2.; 2],
            "generated",
        )
        .unwrap();
        assert!(source.compatible(&target));
        assert_eq!(target.nearest(source.center(1).unwrap()), Some(2));
        assert_eq!(target.nearest(source.center(0).unwrap()), Some(4));
        assert_eq!(source.nearest(target.center(1).unwrap()), Some(1));
        assert_eq!(target.nearest([100., 2., 2.]), None);
        assert_eq!(target.nearest([2., 2., 5.]), None);
        let different = stack(&[[0.; 3], [0., 0., 1.]], [3; 2], [1.; 2], "unrelated").unwrap();
        assert!(!source.compatible(&different));
    }
    #[test]
    fn tilted_planes_are_supported_and_missing_irregular_or_duplicate_geometry_is_refused() {
        let tilted = stack(
            &[[0., 0., 0.], [0.5, 0., 1.], [1., 0., 2.]],
            [5; 2],
            [1.; 2],
            "generated",
        )
        .unwrap();
        assert_eq!(tilted.nearest([2.5, 2., 1.]), Some(1));
        assert!(matches!(
            stack(
                &[[0.; 3], [0., 0., 1.], [0., 0., 3.]],
                [5; 2],
                [1.; 2],
                "generated"
            ),
            Err(GeometryError::Irregular)
        ));
        assert!(matches!(
            stack(&[[0.; 3], [0.; 3]], [5; 2], [1.; 2], "generated"),
            Err(GeometryError::Irregular)
        ));
        assert!(matches!(
            stack(&[[0.; 3], [0., 0., 1.]], [5; 2], [1.; 2], ""),
            Err(GeometryError::Reference)
        ));
        assert!(matches!(
            NavigationStack::build(3, |_| None, || true),
            Err(GeometryError::Cancelled)
        ));
        assert!(matches!(
            NavigationStack::build(
                usize::MAX,
                |_| panic!("oversized metadata must be refused before reading frames"),
                || false
            ),
            Err(GeometryError::Limit)
        ));
    }
    #[test]
    fn nonparallel_and_different_reference_contexts_do_not_link() {
        let base = stack(&[[0.; 3], [0., 0., 2.]], [5; 2], [1.; 2], "generated").unwrap();
        let angle = 0.1f64.to_radians();
        let tilted = NavigationStack::build(
            2,
            |index| {
                Some(SpatialFrame {
                    size: [5; 2],
                    geometry: Some(Geometry {
                        position: [0., 0., index as f64 * 2.],
                        orientation: [1., 0., 0., 0., angle.cos(), angle.sin()],
                        spacing: [1.; 2],
                        frame_of_reference: Some("generated".into()),
                        nifti_transform: None,
                    }),
                })
            },
            || false,
        )
        .unwrap();
        assert!(!base.compatible(&tilted));
        let mut other = stack(&[[0.; 3], [0., 0., 2.]], [5; 2], [1.; 2], "generated").unwrap();
        other.reference.nifti_transform = Some(crate::study::NiftiTransform {
            method: crate::study::NiftiMethod::Sform,
            code: 4,
        });
        assert!(!base.compatible(&other));
    }
    #[test]
    fn navigation_tables_share_admission_and_release_their_exact_reservation() {
        let budget = MemoryBudget::new(1104);
        let frame = |index| {
            Some(SpatialFrame {
                size: [2; 2],
                geometry: Some(Geometry {
                    position: [0., 0., index as f64],
                    orientation: [1., 0., 0., 0., 1., 0.],
                    spacing: [1.; 2],
                    frame_of_reference: Some("generated".into()),
                    nifti_transform: None,
                }),
            })
        };
        let first = NavigationStack::build_with_budget(2, frame, || false, budget.clone()).unwrap();
        assert_eq!(budget.used(), 1104);
        assert!(matches!(
            NavigationStack::build_with_budget(2, frame, || false, budget.clone()),
            Err(GeometryError::Limit)
        ));
        drop(first);
        assert_eq!(budget.used(), 0);
        budget.set_limit(1103);
        assert!(matches!(
            NavigationStack::build_with_budget(2, frame, || false, budget.clone()),
            Err(GeometryError::Limit)
        ));
        assert_eq!(budget.used(), 0);
    }
}
