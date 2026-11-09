// SPDX-License-Identifier: GPL-3.0-or-later
//! Immutable local map owners, without persistence or disclosure authorization.
use super::*;
use crate::segments::Segments;
use std::sync::Arc;

#[derive(Clone)]
pub enum MaskOwner {
    Manual(Arc<Labels>),
    Supplied(Arc<Segments>),
}
impl MaskOwner {
    pub fn source(&self) -> Arc<Volume> {
        match self {
            Self::Manual(v) => v.source_arc(),
            Self::Supplied(v) => v.source_arc(),
        }
    }
    pub fn view(&self, number: u16) -> Result<MaskView<'_>, &'static str> {
        match self {
            Self::Manual(v) => {
                MaskView::manual(v, u8::try_from(number).map_err(|_| "Manual IDs are 1–255")?)
            }
            Self::Supplied(v) => Ok(MaskView::supplied(
                v.mask(number).ok_or("Choose a supplied segment ID")?,
            )),
        }
    }
}
