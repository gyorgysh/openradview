// SPDX-License-Identifier: GPL-3.0-or-later
//! UI-independent image/view domain. No network access or implicit persistence.
pub mod color;
mod dicom_codec;
mod dicom_color;
mod dicom_geometry;
mod dicom_padding;
pub mod index;
pub mod inspection;
pub mod labels;
pub mod load;
pub mod measurements;
pub mod nifti_view;
pub mod oblique;
pub mod resources;
pub mod scene;
pub mod spatial;
pub mod study;
mod vector;
pub mod view;
pub mod view_mapping;
pub mod volume;
pub mod volume_view;
pub use view::{Plane, ViewState, WindowLevel};

#[cfg(test)]
#[path = "../tests/support/mod.rs"]
mod test_support;
