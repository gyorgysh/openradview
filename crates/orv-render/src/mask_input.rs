// SPDX-License-Identifier: GPL-3.0-or-later
//! Immutable owners for one selected categorical view, without flattening IDs.
use crate::{label_source::GpuLabels, segment_source::GpuSegment, volume_source::GpuVolumeSource};
use orv_core::{
    mask::{MaskObjectId, MaskRevision, MaskView},
    segments::SegmentEncoding,
};
use std::sync::Arc;
#[derive(Clone)]
pub enum MaskInput {
    Manual(Arc<GpuLabels>),
    Supplied(Arc<GpuSegment>),
}
impl MaskInput {
    pub fn source(&self) -> &Arc<GpuVolumeSource> {
        match self {
            Self::Manual(v) => v.source(),
            Self::Supplied(v) => v.source(),
        }
    }
    pub fn view(&self, number: u16) -> Result<MaskView<'_>, &'static str> {
        match self {
            Self::Manual(v) => MaskView::manual(
                v.labels(),
                u8::try_from(number).map_err(|_| "Manual label ID exceeds 255")?,
            ),
            Self::Supplied(v) => {
                if number != v.number() {
                    return Err("Supplied GPU input has another selected ID");
                }
                Ok(MaskView::supplied(
                    v.segments().mask(number).ok_or("Unknown supplied ID")?,
                ))
            }
        }
    }
    pub(crate) fn object_id(&self) -> &MaskObjectId {
        match self {
            Self::Manual(v) => v.labels().object_id(),
            Self::Supplied(v) => v.segments().object_id(),
        }
    }
    pub(crate) fn revision(&self) -> &MaskRevision {
        match self {
            Self::Manual(v) => v.labels().revision(),
            Self::Supplied(v) => v.segments().revision(),
        }
    }
    pub(crate) fn same(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Manual(a), Self::Manual(b)) => Arc::ptr_eq(a, b),
            (Self::Supplied(a), Self::Supplied(b)) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }
    pub(crate) fn sampled_view(&self) -> &wgpu::TextureView {
        match self {
            Self::Manual(v) => v.texture_view(),
            Self::Supplied(v) => v.sample_view(),
        }
    }
    pub(crate) fn address_view(&self) -> &wgpu::TextureView {
        match self {
            Self::Manual(v) => &v.source().validity,
            Self::Supplied(v) => v.address_view(),
        }
    }
    pub(crate) fn representation(&self) -> (f32, f32) {
        match self {
            Self::Manual(_) => (0., 1.),
            Self::Supplied(v) => (
                match v.encoding() {
                    SegmentEncoding::Binary => 1.,
                    SegmentEncoding::Fractional(_) => 2.,
                },
                f32::from(v.encoding().maximum()),
            ),
        }
    }
}
