// SPDX-License-Identifier: GPL-3.0-or-later
//! Local image descriptions. Values and declared geometry do not establish medical meaning.
mod histogram;
mod series;
use crate::study::{Frame, Geometry};
pub use histogram::{ChannelHistogram, Histogram, histogram};
pub use series::{Declarations, RegularGrid, SeriesInfo, series_info};

/// Ephemeral one-based study groups for a local catalogue. No identifying text
/// leaves the core; missing declarations remain ungrouped.
pub fn study_groups(
    study: &crate::study::Study,
    cancelled: impl Fn() -> bool,
) -> Result<Vec<Option<usize>>, crate::load::LoadError> {
    if study.series.len() > 200_000 {
        return Err(crate::load::LoadError::Limit);
    }
    let mut identifiers = std::collections::HashMap::new();
    let mut groups = Vec::with_capacity(study.series.len());
    for series in &study.series {
        if cancelled() {
            return Err(crate::load::LoadError::Cancelled);
        }
        let group = series
            .source
            .as_ref()
            .and_then(|source| source.study_identifier())
            .map(|identifier| {
                let next = identifiers.len() + 1;
                *identifiers.entry(identifier).or_insert(next)
            });
        groups.push(group);
    }
    Ok(groups)
}

/// LPS directions outside the left, right, top and bottom image edges.
/// Small (<0.0001) direction components are suppressed as rounding noise.
pub fn edge_directions(geometry: &Geometry) -> Option<[String; 4]> {
    geometry.validate().ok()?;
    if geometry
        .nifti_transform
        .as_ref()
        .is_some_and(|n| n.code == 0)
    {
        return None;
    }
    let (column, row) = geometry.directions();
    Some([
        direction(column.map(|v| -v)),
        direction(column),
        direction(row.map(|v| -v)),
        direction(row),
    ])
}
fn direction(vector: [f64; 3]) -> String {
    let mut axes = [0, 1, 2];
    axes.sort_by(|&a, &b| vector[b].abs().total_cmp(&vector[a].abs()));
    axes.into_iter()
        .filter(|&axis| vector[axis].abs() > 1e-4)
        .map(|axis| match (axis, vector[axis].is_sign_positive()) {
            (0, true) => 'L',
            (0, false) => 'R',
            (1, true) => 'P',
            (1, false) => 'A',
            (2, true) => 'S',
            _ => 'I',
        })
        .collect()
}

/// Projection labels a 2D image declares, such as a mammogram or radiograph.
/// They are copied text, never derived from or checked against geometry.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProjectionLabels {
    pub laterality: Option<&'static str>,
    pub view: Option<String>,
    /// Patient directions toward increasing column, then increasing row.
    pub orientation: Option<[String; 2]>,
}
impl ProjectionLabels {
    pub fn is_empty(&self) -> bool {
        self.laterality.is_none() && self.view.is_none() && self.orientation.is_none()
    }
    /// Left, right, top and bottom edge letters, in `edge_directions` order.
    pub fn edges(&self) -> Option<[String; 4]> {
        let [row, column] = self.orientation.as_ref()?;
        let opposite = |text: &str| -> String {
            text.chars()
                .map(|c| match c {
                    'A' => 'P',
                    'P' => 'A',
                    'R' => 'L',
                    'L' => 'R',
                    'H' => 'F',
                    _ => 'H',
                })
                .collect()
        };
        Some([opposite(row), row.clone(), opposite(column), column.clone()])
    }
    /// Short text for image information, such as "R · MLO".
    pub fn summary(&self) -> Option<String> {
        let parts: Vec<&str> = [self.laterality, self.view.as_deref()]
            .into_iter()
            .flatten()
            .collect();
        (!parts.is_empty()).then(|| parts.join(" · "))
    }
}
/// Declared labels of an image object, or None when it states none of them.
pub(crate) fn projection_labels(
    object: &dicom_object::InMemDicomObject,
) -> Option<std::sync::Arc<ProjectionLabels>> {
    let text = |group: u16, element: u16| {
        object
            .element(dicom_core::Tag(group, element))
            .ok()
            .and_then(|e| e.to_str().ok())
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
    };
    let laterality = text(0x20, 0x62)
        .or_else(|| text(0x20, 0x60))
        .and_then(|value| match value.as_str() {
            "R" => Some("R"),
            "L" => Some("L"),
            "B" => Some("Both sides"),
            "U" => Some("Unpaired"),
            _ => None,
        });
    let view = text(0x18, 0x5101)
        .filter(|value| {
            value.len() <= 16
                && value
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_' || c == ' ')
        })
        .or_else(|| coded_view(object));
    let orientation = text(0x20, 0x20)
        .and_then(|value| patient_orientation(&value.split('\\').collect::<Vec<_>>()));
    let labels = ProjectionLabels {
        laterality,
        view,
        orientation,
    };
    (!labels.is_empty()).then(|| std::sync::Arc::new(labels))
}
/// Mammography views coded in View Code Sequence (PS3.16 CID 4014), as SNOMED CT
/// or the retired SNOMED RT values. Other codes are ignored rather than shown.
fn coded_view(object: &dicom_object::InMemDicomObject) -> Option<String> {
    let items = object
        .element(dicom_core::Tag(0x54, 0x220))
        .ok()?
        .value()
        .items()?;
    let [item] = items else {
        return None;
    };
    let code = item
        .element(dicom_core::Tag(8, 0x100))
        .ok()?
        .to_str()
        .ok()?;
    let view = match code.trim() {
        "399162004" | "R-10242" => "CC",
        "399368009" | "R-10226" => "MLO",
        "399260004" | "R-10224" => "ML",
        "399352003" | "R-10228" => "LM",
        _ => return None,
    };
    Some(view.to_owned())
}
/// Parse declared patient orientation: two values of one to three distinct-axis
/// letters from A, P, R, L, H and F. Anything else is ignored rather than guessed.
pub fn patient_orientation(values: &[&str]) -> Option<[String; 2]> {
    let [row, column] = values else {
        return None;
    };
    let axis = |c: char| match c {
        'A' | 'P' => Some(0),
        'R' | 'L' => Some(1),
        'H' | 'F' => Some(2),
        _ => None,
    };
    let valid = |text: &str| {
        let mut seen = [false; 3];
        (1..=3).contains(&text.len())
            && text
                .chars()
                .all(|c| axis(c).is_some_and(|a| !std::mem::replace(&mut seen[a], true)))
    };
    let (row, column) = (row.trim(), column.trim());
    (valid(row) && valid(column)).then(|| [row.to_owned(), column.to_owned()])
}

pub fn coordinate_reference(geometry: &Geometry) -> &'static str {
    match geometry.nifti_transform.as_ref().map(|n| n.code) {
        None | Some(1) => "Scanner LPS · mm",
        Some(2) => "Aligned LPS · mm",
        Some(3) => "Talairach LPS · mm",
        Some(4) => "MNI152 LPS · mm",
        Some(5) => "Template LPS · mm",
        Some(0) => "Unknown NIfTI reference",
        Some(_) => "Other NIfTI reference · LPS mm",
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Sample {
    Scalar(f32),
    RgbCodes { values: [u16; 3], bits: u16 },
    SrgbDisplay([u8; 3]),
}
/// Nearest source pixel centre, without interpolation or display window/inversion.
pub fn sample(frame: &Frame, pixel: [f64; 2]) -> Option<Sample> {
    if pixel.iter().any(|v| !v.is_finite()) || frame.width == 0 || frame.height == 0 {
        return None;
    }
    let [x, y] = pixel.map(f64::round);
    if x < 0. || y < 0. || x >= f64::from(frame.width) || y >= f64::from(frame.height) {
        return None;
    }
    let index = (y as usize)
        .checked_mul(frame.width as usize)?
        .checked_add(x as usize)?;
    if !frame.has_sample(index) {
        return None;
    }
    if frame.color.is_some() && frame.rgba.is_none() {
        return None;
    }
    if let Some(color) = &frame.color {
        let channels = color
            .rgb
            .get(index.checked_mul(3)?..index.checked_mul(3)?.checked_add(3)?)?;
        if !(1..=16).contains(&color.info.bits)
            || channels.iter().any(|v| *v > color.info.maximum())
        {
            return None;
        }
        Some(Sample::RgbCodes {
            values: channels.try_into().ok()?,
            bits: color.info.bits,
        })
    } else if let Some(rgba) = &frame.rgba {
        let channels = rgba.get(index.checked_mul(4)?..index.checked_mul(4)?.checked_add(3)?)?;
        Some(Sample::SrgbDisplay(channels.try_into().ok()?))
    } else {
        Some(Sample::Scalar(*frame.pixels.get(index)?))
    }
}

#[cfg(test)]
mod tests;
