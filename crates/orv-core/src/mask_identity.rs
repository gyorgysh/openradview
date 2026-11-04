// SPDX-License-Identifier: GPL-3.0-or-later
//! Opaque local mask revisions without serialized source identifiers.
use crate::resources::{MemoryBudget, Reservation};
use std::sync::Arc;

pub(crate) struct Identity {
    _reservation: Reservation,
}
fn identity(budget: &Arc<MemoryBudget>) -> Result<Arc<Identity>, &'static str> {
    let bytes = std::mem::size_of::<Identity>() + 2 * std::mem::size_of::<usize>();
    let reservation = budget
        .reserve(bytes as u64)
        .ok_or("Not enough memory for mask revision metadata")?;
    Ok(Arc::new(Identity {
        _reservation: reservation,
    }))
}

/// Opaque RAM-only object key; distinct maps on the same source remain distinct.
#[derive(Clone)]
pub struct MaskObjectId(Arc<Identity>);
/// Opaque RAM-only data revision. It contains no source identifier or pixel data.
#[derive(Clone)]
pub struct MaskRevision(Arc<Identity>);
macro_rules! opaque_identity {
    ($name:ident) => {
        impl PartialEq for $name {
            fn eq(&self, other: &Self) -> bool {
                Arc::ptr_eq(&self.0, &other.0)
            }
        }
        impl Eq for $name {}
        impl std::fmt::Debug for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(stringify!($name))
            }
        }
    };
}
opaque_identity!(MaskObjectId);
opaque_identity!(MaskRevision);
impl MaskObjectId {
    pub(crate) fn new(budget: &Arc<MemoryBudget>) -> Result<(Self, MaskRevision), &'static str> {
        let id = identity(budget)?;
        Ok((Self(id.clone()), MaskRevision(id)))
    }
}
impl MaskRevision {
    pub(crate) fn next(budget: &Arc<MemoryBudget>) -> Result<Self, &'static str> {
        identity(budget).map(Self)
    }
}
