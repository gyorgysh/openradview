// SPDX-License-Identifier: GPL-3.0-or-later
//! Color code values are distinct from scalar modality/physical measurements.
use serde::{Deserialize, Serialize};
use std::sync::Arc;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColorEncoding {
    Rgb,
    YbrFull,
    YbrFull422,
    YbrRct,
    YbrIct,
    Palette,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColorInfo {
    pub source_encoding: ColorEncoding,
    /// RGB code precision after palette/YBR conversion and before ICC/display.
    pub bits: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icc_sha256: Option<String>,
}
impl ColorInfo {
    pub fn presentation(&self) -> &'static str {
        if self.icc_sha256.is_some() {
            "ICC converted to sRGB; display calibration not verified"
        } else {
            "sRGB assumed; display calibration not verified"
        }
    }
    pub fn maximum(&self) -> u16 {
        if (1..=16).contains(&self.bits) {
            ((1u32 << self.bits) - 1) as u16
        } else {
            0
        }
    }
}
#[derive(Clone, Debug)]
pub struct ColorData {
    pub info: ColorInfo,
    /// Interleaved RGB code values before ICC and 8-bit presentation conversion.
    pub rgb: Arc<[u16]>,
}
impl ColorData {
    pub(crate) fn hash_into(
        &self,
        hash: &mut sha2::Sha256,
        cancelled: &impl Fn() -> bool,
    ) -> Result<(), &'static str> {
        use sha2::Digest;
        hash.update(b"OpenRadView RGB code values v1\0");
        hash.update(serde_json::to_vec(&self.info).map_err(|_| "Invalid color context")?);
        let mut block = Vec::with_capacity(8192);
        for chunk in self.rgb.chunks(4096) {
            if cancelled() {
                return Err("Cancelled");
            }
            block.clear();
            for value in chunk {
                block.extend_from_slice(&value.to_le_bytes());
            }
            hash.update(&block);
        }
        Ok(())
    }
}
