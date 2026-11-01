// SPDX-License-Identifier: GPL-3.0-or-later
//! Physical pan/zoom candidates and conservative scalar-window compatibility.
use crate::{
    ViewState, WindowLevel,
    scalar::Unit,
    study::{Frame, Geometry},
    vector::*,
    view_mapping::ImageMapping,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DisplayLinks {
    pub pan: bool,
    pub zoom: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkError {
    Geometry,
    Reference,
    Orientation,
    View,
    Limits,
    Representation,
}
impl std::fmt::Display for LinkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Geometry => {
                "Physical display linking needs finite thin-image geometry and spacing."
            }
            Self::Reference => "Physical display linking needs the same known spatial reference.",
            Self::Orientation => {
                "This display link needs parallel image planes; no registration is performed."
            }
            Self::View => "The completed view or viewport dimensions are unavailable.",
            Self::Limits => {
                "The linked display would exceed supported pan/zoom limits. Its view is unchanged."
            }
            Self::Representation => {
                "Window linking needs compatible scalar representations, mappings and known units."
            }
        })
    }
}
impl std::error::Error for LinkError {}

/// An immediate, borrowed action value. It retains no pixel buffer or private
/// reference copy, and cannot outlive the immutable geometry that produced it.
pub struct PhysicalDisplay<'a> {
    geometry: &'a Geometry,
    centre_pixel: [f64; 2],
    vertical_fov_mm: f64,
}
impl<'a> PhysicalDisplay<'a> {
    pub fn capture(
        frame: &'a Frame,
        view: &ViewState,
        viewport_aspect: f64,
    ) -> Result<Self, LinkError> {
        let geometry = geometry(frame)?;
        let map = mapping(frame, view, viewport_aspect)?;
        let centre_pixel = map
            .view_to_pixel_unbounded([0.5; 2])
            .ok_or(LinkError::View)?;
        let vertical_fov_mm = vertical_extent(frame, view, viewport_aspect)? / f64::from(view.zoom);
        if !vertical_fov_mm.is_finite() || vertical_fov_mm <= 0. {
            return Err(LinkError::Geometry);
        }
        Ok(Self {
            geometry,
            centre_pixel,
            vertical_fov_mm,
        })
    }
    pub fn vertical_fov_mm(&self) -> f64 {
        self.vertical_fov_mm
    }

    /// Each call proposes one target update. Pan-only preserves its zoom;
    /// zoom-only preserves its own physical centre. Other view fields stay intact.
    /// No link forwards itself: action ownership/generation belongs to the caller.
    pub fn apply(
        &self,
        target: &Frame,
        view: &ViewState,
        viewport_aspect: f64,
        links: DisplayLinks,
    ) -> Result<ViewState, LinkError> {
        if !links.pan && !links.zoom {
            return Ok(view.clone());
        }
        let target_display = PhysicalDisplay::capture(target, view, viewport_aspect)?;
        let target_geometry = target_display.geometry;
        if self.geometry.frame_of_reference != target_geometry.frame_of_reference
            || self.geometry.nifti_transform != target_geometry.nifti_transform
        {
            return Err(LinkError::Reference);
        }
        let normal = |geometry: &Geometry| {
            let n = geometry.normal();
            mul(n, 1. / norm(n))
        };
        if norm(cross(normal(self.geometry), normal(target_geometry))) > 1e-4 {
            return Err(LinkError::Orientation);
        }
        let mut candidate = view.clone();
        if links.zoom {
            let zoom = vertical_extent(target, view, viewport_aspect)? / self.vertical_fov_mm;
            if !zoom.is_finite() || zoom <= 0. {
                return Err(LinkError::Limits);
            }
            candidate.zoom = zoom as f32;
            candidate.validate().map_err(|_| LinkError::Limits)?;
        }
        let anchor = if links.pan {
            let (u, v) = self.geometry.directions();
            let delta = add(
                sub(self.geometry.position, target_geometry.position),
                add(
                    mul(u, self.centre_pixel[0] * self.geometry.spacing[1]),
                    mul(v, self.centre_pixel[1] * self.geometry.spacing[0]),
                ),
            );
            projected_pixel(target_geometry, delta)?
        } else {
            target_display.centre_pixel
        };
        candidate.pan = [0.; 2];
        let projected = mapping(target, &candidate, viewport_aspect)?
            .pixel_to_view(anchor)
            .ok_or(LinkError::View)?;
        candidate.pan = projected.map(|value| (0.5 - value) as f32);
        candidate.validate().map_err(|_| LinkError::Limits)?;
        Ok(candidate)
    }
}
fn geometry(frame: &Frame) -> Result<&Geometry, LinkError> {
    if frame.slab.is_some() || frame.width == 0 || frame.height == 0 {
        return Err(LinkError::Geometry);
    }
    let geometry = frame.geometry.as_ref().ok_or(LinkError::Geometry)?;
    geometry.validate().map_err(|_| LinkError::Geometry)?;
    if geometry
        .frame_of_reference
        .as_ref()
        .is_none_or(|value| value.is_empty() || value.len() > 128)
    {
        return Err(LinkError::Reference);
    }
    Ok(geometry)
}
fn mapping(frame: &Frame, view: &ViewState, aspect: f64) -> Result<ImageMapping, LinkError> {
    ImageMapping::new(
        [frame.width, frame.height],
        f64::from(frame.aspect_ratio()),
        aspect,
        view,
    )
    .ok_or(LinkError::View)
}
/// Height at zoom1 divided by its letterbox fit, matching the native display.
fn vertical_extent(
    frame: &Frame,
    view: &ViewState,
    viewport_aspect: f64,
) -> Result<f64, LinkError> {
    mapping(frame, view, viewport_aspect)?;
    let geometry = geometry(frame)?;
    let (u, v) = geometry.directions();
    let odd = view.transform.quarter_turns & 1 == 1;
    let height = if odd {
        f64::from(frame.width) * geometry.spacing[1] * norm(u)
    } else {
        f64::from(frame.height) * geometry.spacing[0] * norm(v)
    };
    let aspect = view.transform.aspect(f64::from(frame.aspect_ratio()));
    let fit = (viewport_aspect / aspect).min(1.);
    let extent = height / fit;
    if extent.is_finite() && extent > 0. {
        Ok(extent)
    } else {
        Err(LinkError::Geometry)
    }
}
fn projected_pixel(geometry: &Geometry, delta: V3) -> Result<[f64; 2], LinkError> {
    if !finite(delta) {
        return Err(LinkError::Geometry);
    }
    let (u, v) = geometry.directions();
    let uu = dot(u, u);
    let vv = dot(v, v);
    let uv = dot(u, v);
    let determinant = uu * vv - uv * uv;
    let du = dot(delta, u);
    let dv = dot(delta, v);
    let pixel = [
        (vv * du - uv * dv) / determinant / geometry.spacing[1],
        (uu * dv - uv * du) / determinant / geometry.spacing[0],
    ];
    if pixel.iter().all(|value| value.is_finite()) {
        Ok(pixel)
    } else {
        Err(LinkError::Geometry)
    }
}

/// This initial subset requires equal known units, scalar code/LUT identity and
/// matching source mapping. A numeric LUT index alone is never a shared table.
pub fn scalar_window(
    source: &Frame,
    target: &Frame,
    window: WindowLevel,
) -> Result<WindowLevel, LinkError> {
    if source.rgba.is_some() || target.rgba.is_some() {
        return Err(LinkError::Representation);
    }
    let source_scalar = source.scalar.as_ref().ok_or(LinkError::Representation)?;
    let target_scalar = target.scalar.as_ref().ok_or(LinkError::Representation)?;
    let a = source_scalar.info();
    let b = target_scalar.info();
    if a.unit == Unit::Unknown
        || !source_scalar.compatible(target_scalar)
        || a.rescale != b.rescale
        || a.resampled != b.resampled
        || !source_scalar.supports(window)
        || !target_scalar.supports(window)
    {
        return Err(LinkError::Representation);
    }
    Ok(window)
}

#[cfg(test)]
mod tests;
