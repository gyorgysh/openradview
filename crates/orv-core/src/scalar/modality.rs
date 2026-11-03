// SPDX-License-Identifier: GPL-3.0-or-later
//! Stored integer codes to modality values, before any spatial interpolation.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModalityLutInfo {
    pub first: i32,
    pub entries: u32,
    pub bits: u16,
    pub sha256: [u8; 32],
}
impl ModalityLutInfo {
    pub fn validate(self) -> bool {
        (-32768..=65535).contains(&self.first)
            && (1..=65536).contains(&self.entries)
            && matches!(self.bits, 8 | 16)
    }
}
#[derive(Clone)]
pub struct ModalityLut {
    info: ModalityLutInfo,
    values: Arc<[u16]>,
}
impl std::fmt::Debug for ModalityLut {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModalityLut")
            .field("info", &self.info)
            .finish_non_exhaustive()
    }
}
impl ModalityLut {
    pub fn new(first: i32, bits: u16, values: Arc<[u16]>) -> Result<Arc<Self>, &'static str> {
        if !(-32768..=65535).contains(&first)
            || !matches!(bits, 8 | 16)
            || values.is_empty()
            || values.len() > 65536
            || (bits == 8 && values.iter().any(|value| *value > 255))
        {
            return Err("Unsupported modality lookup table");
        }
        let mut hash = Sha256::new();
        hash.update(b"OpenRadView modality table v1\0");
        hash.update(first.to_le_bytes());
        hash.update(bits.to_le_bytes());
        hash.update((values.len() as u32).to_le_bytes());
        for value in values.iter() {
            hash.update(value.to_le_bytes());
        }
        Ok(Arc::new(Self {
            info: ModalityLutInfo {
                first,
                entries: values.len() as u32,
                bits,
                sha256: hash.finalize().into(),
            },
            values,
        }))
    }
    pub fn info(&self) -> ModalityLutInfo {
        self.info
    }
    pub fn map(&self, stored: i64) -> f32 {
        let index = stored
            .saturating_sub(i64::from(self.info.first))
            .clamp(0, i64::from(self.info.entries) - 1) as usize;
        f32::from(self.values[index])
    }
    pub(crate) fn byte_len(&self) -> u64 {
        std::mem::size_of::<Self>() as u64 + self.values.len() as u64 * 2
    }
}
