// SPDX-License-Identifier: GPL-3.0-or-later
//! UI-independent image/view domain. No network access or implicit persistence.
pub mod acquisition;
pub mod bricks;
pub mod calibration;
pub mod color;
pub mod curved;
mod dicom_codec;
mod dicom_color;
mod dicom_float;
mod dicom_geometry;
mod dicom_modality;
mod dicom_padding;
mod dicom_rwvm;
mod dicom_voi;
pub mod fusion;
pub mod import_report;
pub mod index;
pub mod inspection;
pub mod labels;
pub mod linked_display;
pub mod load;
pub mod mask;
mod mask_identity;
pub mod measurements;
pub mod navigation;
pub mod nifti_view;
pub mod oblique;
mod plane_sampling;
pub mod presentation;
pub mod reference_line;
pub mod resources;
pub mod scalar;
pub mod scene;
pub mod segments;
pub mod slab;
pub mod spatial;
pub mod study;
pub mod surface;
mod vector;
pub mod view;
pub mod view_mapping;
pub mod volume;
pub mod volume_material;
pub mod volume_ranges;
pub mod volume_view;
pub use view::{DisplayTransform, Plane, ViewState, VoiFunction, WindowLevel};

#[cfg(test)]
#[path = "../tests/support/mod.rs"]
mod test_support;
