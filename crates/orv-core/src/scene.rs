// SPDX-License-Identifier: GPL-3.0-or-later
//! Portable image provenance. No raw DICOM objects, identifiers, paths or free text.
use crate::{
    Plane, ViewState,
    oblique::ObliquePlane,
    study::{Frame, Series},
    volume::Volume,
    volume_view::VolumeView,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Everything needed to reproduce one image, against the same approved source.
/// For MPR, `crosshair_mm` is the rendered LPS point and `view.slice` must be zero.
/// This avoids confusing pending UI navigation with a completed reformat.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SceneState {
    pub schema_version: u32,
    pub source_sha256: String,
    pub series_index: u32,
    pub view: ViewState,
    pub crosshair_mm: Option<[f64; 3]>,
    pub reformat_side: u32,
    pub output_size: [u32; 2],
    /// Schema 2 only. Omission preserves the exact schema 1 serialization/hash.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reconstruction: Option<Reconstruction>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", content = "parameters", deny_unknown_fields)]
pub enum Reconstruction {
    Oblique(ObliquePlane),
    Volume(VolumeView),
}

impl SceneState {
    pub fn validate(&self) -> Result<(), &'static str> {
        self.view.validate()?;
        if self.schema_version != if self.reconstruction.is_some() { 2 } else { 1 }
            || !is_sha256(&self.source_sha256)
        {
            return Err("Unsupported scene schema or source fingerprint");
        }
        if self.output_size.iter().any(|v| !(1..=1024).contains(v))
            || !(2..=1024).contains(&self.reformat_side)
        {
            return Err("Snapshot dimensions exceed the supported limits");
        }
        if self.view.window.width > 1_000_000.0 || self.view.window.center.abs() > 1_000_000_000.0 {
            return Err("Snapshot window exceeds the supported range");
        }
        if let Some(reconstruction) = &self.reconstruction {
            if self.view.plane != Plane::Acquired
                || self.view.slice != 0
                || self.crosshair_mm.is_some()
            {
                return Err(
                    "Explicit reconstruction cannot contain a source slice or anatomical plane",
                );
            }
            return match reconstruction {
                Reconstruction::Oblique(plane) => {
                    plane.validate().map_err(|_| "Invalid oblique plane")
                }
                Reconstruction::Volume(state) => {
                    if self.reformat_side != 2 {
                        return Err("Volume scenes use the canonical unused reformat side of two");
                    }
                    state.validate()
                }
            };
        }
        match (self.view.plane, self.crosshair_mm) {
            (Plane::Acquired, None) => {}
            (Plane::Axial | Plane::Coronal | Plane::Sagittal, Some(p))
                if self.view.slice == 0
                    && p.iter().all(|v| v.is_finite() && v.abs() <= 10_000_000.0) => {}
            _ => return Err("Scene plane and physical position are inconsistent"),
        }
        Ok(())
    }

    pub fn hash(&self) -> Result<String, &'static str> {
        self.validate()?;
        let bytes = serde_json::to_vec(self).map_err(|_| "Could not serialize scene")?;
        Ok(hex(&Sha256::digest(bytes)))
    }
}

pub fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut result = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        // Formatting into a String is infallible.
        let _ = write!(result, "{byte:02x}");
    }
    result
}

/// Content identity, computed on a worker. Includes scalar data, order and spatial
/// geometry; excludes patient/series/reference UIDs, instance numbers and paths.
/// Reference equality is encoded as local ordinals, preserving volume eligibility
/// without publishing the reference identifiers themselves.
pub struct SourceFingerprint {
    pub source_sha256: String,
    /// Indexed sources are verified again when read after approval.
    pub frame_sha256: Option<Vec<[u8; 32]>>,
}
pub fn series_fingerprint(
    series: &Series,
    cancelled: impl Fn() -> bool,
) -> Result<String, &'static str> {
    fingerprint_series(series, cancelled).map(|f| f.source_sha256)
}
pub fn fingerprint_series(
    series: &Series,
    cancelled: impl Fn() -> bool,
) -> Result<SourceFingerprint, &'static str> {
    if series.frame_count() == 0 {
        return Err("The series has no images");
    }
    let mut hash = Sha256::new();
    hash.update(b"OpenRadView source v1\0");
    hash.update((series.frame_count() as u64).to_le_bytes());
    hash.update((series.modality.len() as u64).to_le_bytes());
    hash.update(series.modality.as_bytes());
    let mut references: Vec<String> = Vec::new();
    let mut frame_sha256 = series
        .source
        .as_ref()
        .map(|_| Vec::with_capacity(series.frame_count()));
    for index in 0..series.frame_count() {
        let frame = series
            .read_frame(index, &cancelled)
            .map_err(|_| "Source unavailable, changed or cancelled")?;
        let mut pixels_hash = frame_sha256.as_ref().map(|_| Sha256::new());
        if cancelled() {
            return Err("Cancelled");
        }
        if frame.width == 0 || frame.height == 0 || !frame.valid_dimensions() {
            return Err("Invalid source dimensions");
        }
        hash.update(frame.width.to_le_bytes());
        hash.update(frame.height.to_le_bytes());
        hash.update([u8::from(frame.monochrome1)]);
        if let Some(view) = &frame.nifti {
            hash.update(b"NIfTI subvolume v1\0");
            hash.update(serde_json::to_vec(view).map_err(|_| "Invalid NIfTI context")?);
        }
        match &frame.geometry {
            None => hash.update([0]),
            Some(g) => {
                hash.update([1]);
                if let Some(transform) = g.nifti_transform {
                    hash.update(b"NIfTI reference v1\0");
                    hash.update([u8::from(
                        transform.method == crate::study::NiftiMethod::Sform,
                    )]);
                    hash.update(transform.code.to_le_bytes());
                }
                for v in g.position.iter().chain(&g.orientation).chain(&g.spacing) {
                    if !v.is_finite() {
                        return Err("Invalid source geometry");
                    }
                    hash.update(v.to_le_bytes());
                }
                let reference = g
                    .frame_of_reference
                    .as_deref()
                    .map(|id| {
                        if let Some(index) = references.iter().position(|known| *known == id) {
                            index + 1
                        } else {
                            references.push(id.to_owned());
                            references.len()
                        }
                    })
                    .unwrap_or(0);
                hash.update((reference as u64).to_le_bytes());
            }
        }
        if let Some(rgba) = &frame.rgba {
            let domain = b"OpenRadView RGBA8 sRGB v1\0";
            hash.update(domain);
            if let Some(digest) = &mut pixels_hash {
                digest.update(domain);
            }
            for chunk in rgba.chunks(16384) {
                if cancelled() {
                    return Err("Cancelled");
                }
                hash.update(chunk);
                if let Some(digest) = &mut pixels_hash {
                    digest.update(chunk);
                }
            }
        }
        if let Some(color) = &frame.color {
            color.hash_into(&mut hash, &cancelled)?;
            if let Some(digest) = &mut pixels_hash {
                color.hash_into(digest, &cancelled)?;
            }
        }
        // A bounded scratch block avoids millions of individual digest updates.
        let mut block = Vec::with_capacity(16_384);
        for chunk in frame.pixels.chunks(4096) {
            if cancelled() {
                return Err("Cancelled");
            }
            block.clear();
            for value in chunk {
                if !value.is_finite() {
                    return Err("Source pixels must be finite");
                }
                block.extend_from_slice(&value.to_le_bytes());
            }
            hash.update(&block);
            if let Some(digest) = &mut pixels_hash {
                digest.update(&block);
            }
        }
        hash_validity(&frame, &mut hash, &cancelled)?;
        if let Some(digest) = &mut pixels_hash {
            hash_validity(&frame, digest, &cancelled)?;
        }
        if let Some(digests) = &mut frame_sha256
            && let Some(digest) = pixels_hash
        {
            digests.push(digest.finalize().into());
        }
    }
    Ok(SourceFingerprint {
        source_sha256: hex(&hash.finalize()),
        frame_sha256,
    })
}

/// Hash immutable pixel values and validity; geometry/display identity is already
/// held in the prepared source. Uses bounded canonical f32/mask block encoding.
pub fn frame_pixel_hash(
    frame: &Frame,
    cancelled: impl Fn() -> bool,
) -> Result<[u8; 32], &'static str> {
    let mut digest = Sha256::new();
    if !frame.valid_dimensions() {
        return Err("Invalid source dimensions");
    }
    if let Some(rgba) = &frame.rgba {
        digest.update(b"OpenRadView RGBA8 sRGB v1\0");
        for chunk in rgba.chunks(16384) {
            if cancelled() {
                return Err("Cancelled");
            }
            digest.update(chunk);
        }
    }
    if let Some(color) = &frame.color {
        color.hash_into(&mut digest, &cancelled)?;
    }
    let mut block = Vec::with_capacity(16_384);
    for chunk in frame.pixels.chunks(4096) {
        if cancelled() {
            return Err("Cancelled");
        }
        block.clear();
        for value in chunk {
            if !value.is_finite() {
                return Err("Source pixels must be finite");
            }
            block.extend_from_slice(&value.to_le_bytes());
        }
        digest.update(&block);
    }
    hash_validity(frame, &mut digest, &cancelled)?;
    Ok(digest.finalize().into())
}

fn hash_validity(
    frame: &Frame,
    digest: &mut Sha256,
    cancelled: &impl Fn() -> bool,
) -> Result<(), &'static str> {
    if let Some(mask) = &frame.validity {
        digest.update(b"OpenRadView scalar validity v1\0");
        for chunk in mask.chunks(16384) {
            if cancelled() {
                return Err("Cancelled");
            }
            digest.update(chunk);
        }
    }
    Ok(())
}

/// Allowlist of spatial values only. Removing tags cannot remove burned-in text.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageGeometry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nifti_transform: Option<crate::study::NiftiTransform>,
    pub position_mm: [f64; 3],
    pub orientation: [f64; 6],
    pub spacing_mm: [f64; 2],
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<crate::color::ColorInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nifti: Option<crate::nifti_view::NiftiView>,
    pub dimensions: [u32; 2],
    pub monochrome1: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_space: Option<String>,
    pub geometry: Option<ImageGeometry>,
}

impl ImageMetadata {
    pub fn from_frame(frame: &Frame) -> Self {
        Self {
            color: frame.color.as_ref().map(|data| data.info.clone()),
            nifti: frame.nifti.clone(),
            dimensions: [frame.width, frame.height],
            color_space: frame.rgba.as_ref().map(|_| {
                frame
                    .color
                    .as_ref()
                    .map(|data| data.info.presentation())
                    .unwrap_or("sRGB assumed; no ICC calibration")
                    .into()
            }),
            monochrome1: frame.monochrome1,
            geometry: frame
                .geometry
                .as_ref()
                .filter(|g| g.validate().is_ok())
                .map(|g| ImageGeometry {
                    nifti_transform: g.nifti_transform,
                    position_mm: g.position,
                    orientation: g.orientation,
                    spacing_mm: g.spacing,
                }),
        }
    }
}

/// Full affine source geometry for projections; no 2D plane is implied.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VolumeMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<crate::color::ColorInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_rendering: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nifti: Option<crate::nifti_view::NiftiView>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nifti_transform: Option<crate::study::NiftiTransform>,
    pub dimensions: [usize; 3],
    pub origin_mm: [f64; 3],
    /// LPS displacement for one column, row and slice index, respectively.
    pub voxel_steps_mm: [[f64; 3]; 3],
    pub source_monochrome1: bool,
}
impl VolumeMetadata {
    pub fn from_volume(volume: &Volume) -> Self {
        let origin_mm = volume.voxel_to_world([0.; 3]);
        Self {
            color: volume.color_info().cloned(),
            color_rendering: volume.is_color().then(|| "Linear-light sRGB; MIP takes independent RGB channel maxima; opacity uses linear-light brightness 0..255; display calibration unverified".into()),
            nifti: volume.frames().first().and_then(|f| f.nifti.clone()),
            dimensions: volume.dimensions(),
            nifti_transform: volume
                .frames()
                .first()
                .and_then(|f| f.geometry.as_ref())
                .and_then(|g| g.nifti_transform),
            origin_mm,
            voxel_steps_mm: std::array::from_fn(|axis| {
                let p =
                    volume.voxel_to_world(std::array::from_fn(|i| if i == axis { 1. } else { 0. }));
                std::array::from_fn(|i| p[i] - origin_mm[i])
            }),
            source_monochrome1: volume.frames()[0].monochrome1,
        }
    }
}
