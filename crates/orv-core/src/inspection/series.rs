// SPDX-License-Identifier: GPL-3.0-or-later
//! Local header summaries; geometry readiness does not validate pixels or GPU admission.
use crate::{
    navigation::SpatialFrame,
    spatial::{GeometryError, RegularGeometry},
    study::Series,
    vector::*,
};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Declarations {
    pub yes: usize,
    pub no: usize,
    pub unknown: usize,
}

impl Declarations {
    fn add(&mut self, value: Option<bool>) {
        match value {
            Some(true) => self.yes += 1,
            Some(false) => self.no += 1,
            None => self.unknown += 1,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RegularGrid {
    pub dimensions: [usize; 3],
    /// Length of the source affine basis vectors, not acquisition slice thickness.
    pub steps_mm: [f64; 3],
    pub tilted: bool,
}

#[derive(Clone, Debug)]
pub struct SeriesInfo {
    pub frames: usize,
    pub decoded_bytes: u64,
    pub geometry: Result<RegularGrid, GeometryError>,
    pub lossy: Declarations,
    pub derived: Declarations,
}

pub fn series_info(
    series: &Series,
    cancelled: impl Fn() -> bool,
) -> Result<SeriesInfo, GeometryError> {
    let mut lossy = Declarations::default();
    let mut derived = Declarations::default();
    let mut decoded_bytes = 0u64;
    let frames = series.frame_count();
    for index in 0..frames {
        if cancelled() {
            return Err(GeometryError::Cancelled);
        }
        let (loss, derivation) = series
            .source
            .as_ref()
            .map_or((None, None), |source| source.declarations(index));
        lossy.add(loss);
        derived.add(derivation);
        let bytes = series
            .source
            .as_ref()
            .map_or_else(
                || series.frames.get(index).map(|frame| frame.byte_len()),
                |source| source.frame_decoded_bytes(index),
            )
            .ok_or(GeometryError::Missing)?;
        decoded_bytes = decoded_bytes
            .checked_add(bytes)
            .ok_or(GeometryError::Limit)?;
    }
    let geometry = RegularGeometry::prepare(
        frames,
        |index| {
            series.source.as_ref().map_or_else(
                || {
                    series.frames.get(index).map(|frame| SpatialFrame {
                        size: [frame.width, frame.height],
                        geometry: frame.geometry.clone(),
                    })
                },
                |source| source.navigation_frame(index),
            )
        },
        &cancelled,
    )
    .map(|geometry| {
        let normal = cross(geometry.basis[0], geometry.basis[1]);
        let tilted =
            norm(cross(normal, geometry.basis[2])) > 1e-5 * norm(normal) * norm(geometry.basis[2]);
        RegularGrid {
            dimensions: geometry.dimensions,
            steps_mm: geometry.basis.map(norm),
            tilted,
        }
    });
    if cancelled() || geometry == Err(GeometryError::Cancelled) {
        return Err(GeometryError::Cancelled);
    }
    Ok(SeriesInfo {
        frames,
        decoded_bytes,
        geometry,
        lossy,
        derived,
    })
}
