// SPDX-License-Identifier: GPL-3.0-or-later
//! Header offsets follow the NIfTI reference headers; v2 retains f64 geometry.
use crate::load::LoadError;
pub(super) struct Fields<'a> {
    pub bytes: &'a [u8],
    pub big: bool,
    pub version: u8,
}
impl Fields<'_> {
    fn unsigned(&self, at: usize, length: usize) -> u64 {
        let mut b = [0u8; 8];
        for (i, value) in self.bytes[at..at + length].iter().enumerate() {
            b[if self.big { length - 1 - i } else { i }] = *value;
        }
        u64::from_le_bytes(b)
    }
    /// Common v1 field addresses, mapped to their exact v2 counterparts.
    pub fn u16(&self, at: usize) -> u16 {
        let (offset, length) = if self.version == 1 {
            (at, 2)
        } else {
            match at {
                70 => (12, 2),
                72 => (14, 2),
                252 => (344, 4),
                254 => (348, 4),
                _ => unreachable!("only fixed datatype/form fields are read"),
            }
        };
        // Unknown/negative form codes must never truncate to a valid reference.
        u16::try_from(self.unsigned(offset, length)).unwrap_or(u16::MAX)
    }
    pub fn dim(&self, axis: usize) -> Result<u64, LoadError> {
        let value = if self.version == 1 {
            self.unsigned(40 + axis * 2, 2)
        } else {
            self.unsigned(16 + axis * 8, 8)
        };
        let max = if self.version == 1 {
            i16::MAX as u64
        } else {
            i64::MAX as u64
        };
        if value > max {
            Err(LoadError::Unsupported)
        } else {
            Ok(value)
        }
    }
    pub fn units(&self) -> u32 {
        if self.version == 1 {
            self.bytes[123] as u32
        } else {
            self.unsigned(500, 4) as u32
        }
    }
    pub fn offset(&self) -> Result<u64, LoadError> {
        if self.version == 2 {
            let value = self.unsigned(168, 8);
            return if value <= i64::MAX as u64 {
                Ok(value)
            } else {
                Err(LoadError::Limit)
            };
        }
        let value = self.f32(108);
        if value.is_finite() && value >= 0. && value.fract() == 0. && value < u64::MAX as f64 {
            Ok(value as u64)
        } else {
            Err(LoadError::Limit)
        }
    }
    pub fn f32(&self, at: usize) -> f64 {
        if self.version == 1 {
            return f32::from_bits(self.unsigned(at, 4) as u32) as f64;
        }
        let offset = match at {
            76..=104 => 104 + (at - 76) * 2,
            112 => 176,
            116 => 184,
            124 => 192,
            128 => 200,
            256..=324 => 352 + (at - 256) * 2,
            _ => unreachable!("only fixed scaling/affine fields are read"),
        };
        f64::from_bits(self.unsigned(offset, 8))
    }
}
