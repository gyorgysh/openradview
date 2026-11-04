// SPDX-License-Identifier: GPL-3.0-or-later
//! Read-only source-cell masks. Membership, coverage and source validity stay separate.
mod ray;
pub use crate::mask_identity::{MaskObjectId, MaskRevision};
use crate::{labels::Labels, segments::SegmentMask, volume::Volume};
pub use ray::{CellInterval, CellRay, MaskHit, MaskInterval, MaskRay};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MaskKey {
    pub object: MaskObjectId,
    pub number: u16,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Membership {
    pub known: bool,
    pub present: bool,
    pub source_valid: bool,
}
impl Membership {
    pub fn eligible(self) -> bool {
        self.known && self.present && self.source_valid
    }
}
#[derive(Clone, Copy)]
enum View<'a> {
    Manual(&'a Labels, u8),
    Supplied(SegmentMask<'a>),
}
/// A selected ID, without flattening overlapping segments into a winning label.
/// Callers retain the owning immutable map/segments across asynchronous work.
#[derive(Clone, Copy)]
pub struct MaskView<'a>(View<'a>);
impl<'a> MaskView<'a> {
    pub fn manual(labels: &'a Labels, number: u8) -> Result<Self, &'static str> {
        if number == 0 {
            return Err("Background is not a selectable mask segment");
        }
        Ok(Self(View::Manual(labels, number)))
    }
    pub fn supplied(mask: SegmentMask<'a>) -> Self {
        Self(View::Supplied(mask))
    }
    pub fn source(&self) -> &'a Volume {
        match self.0 {
            View::Manual(labels, _) => labels.source(),
            View::Supplied(mask) => mask.source(),
        }
    }
    pub fn key(&self) -> MaskKey {
        let (object, number) = match self.0 {
            View::Manual(labels, number) => (labels.object_id(), u16::from(number)),
            View::Supplied(mask) => (mask.object_id(), mask.number()),
        };
        MaskKey {
            object: object.clone(),
            number,
        }
    }
    pub fn revision(&self) -> &'a MaskRevision {
        match self.0 {
            View::Manual(labels, _) => labels.revision(),
            View::Supplied(mask) => mask.revision(),
        }
    }
    /// None is outside the source. Unknown coverage is an explicit sample inside it.
    pub fn sample(&self, voxel: [usize; 3]) -> Option<Membership> {
        let [width, height, depth] = self.source().dimensions();
        let [x, y, z] = voxel;
        if x >= width || y >= height || z >= depth {
            return None;
        }
        let value = match self.0 {
            View::Manual(labels, number) => labels.value(voxel).map(|v| v == number),
            View::Supplied(mask) => mask.value(voxel),
        };
        Some(Membership {
            known: value.is_some(),
            present: value == Some(true),
            source_valid: self.source().frames()[z].has_sample(y * width + x),
        })
    }
}

#[cfg(test)]
mod tests;
