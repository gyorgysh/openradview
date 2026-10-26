// SPDX-License-Identifier: GPL-3.0-or-later
use super::Labels;
use std::io::{self, Write};
impl Labels {
    /// NIfTI-2 uint8 labels, exact f64 sform; no source names, identifiers, pixels or extensions.
    pub fn write_nifti(&self, mut output: impl Write, cancel: impl Fn() -> bool) -> io::Result<()> {
        let mut h = [0u8; 544];
        h[..4].copy_from_slice(&540i32.to_le_bytes());
        h[4..12].copy_from_slice(b"n+2\0\r\n\x1a\n");
        h[12..14].copy_from_slice(&2i16.to_le_bytes());
        h[14..16].copy_from_slice(&8i16.to_le_bytes());
        h[16..24].copy_from_slice(&3i64.to_le_bytes());
        for (i, n) in self.source.dimensions().into_iter().enumerate() {
            h[24 + i * 8..32 + i * 8].copy_from_slice(&(n as i64).to_le_bytes());
        }
        for i in 4..8 {
            h[16 + i * 8..24 + i * 8].copy_from_slice(&1i64.to_le_bytes());
        }
        h[168..176].copy_from_slice(&544i64.to_le_bytes());
        h[176..184].copy_from_slice(&1f64.to_le_bytes());
        h[500..504].copy_from_slice(&2i32.to_le_bytes());
        h[504..508].copy_from_slice(&1002i32.to_le_bytes());
        h[508..519].copy_from_slice(b"User labels");
        let code = self.source.frames()[0]
            .geometry
            .as_ref()
            .and_then(|g| g.nifti_transform)
            .map_or(1, |t| i32::from(t.code));
        h[348..352].copy_from_slice(&code.to_le_bytes());
        let origin = self.source.voxel_to_world([0.; 3]);
        let basis: [[f64; 3]; 3] = std::array::from_fn(|axis| {
            let mut p = [0.; 3];
            p[axis] = 1.;
            let world = self.source.voxel_to_world(p);
            std::array::from_fn(|r| world[r] - origin[r])
        });
        h[104..112].copy_from_slice(&1f64.to_le_bytes());
        for (axis, step) in basis.iter().enumerate() {
            h[112 + axis * 8..120 + axis * 8]
                .copy_from_slice(&step.iter().map(|x| x * x).sum::<f64>().sqrt().to_le_bytes());
        }
        for (row, position) in origin.into_iter().enumerate() {
            let sign = if row < 2 { -1. } else { 1. };
            for (col, coordinate) in basis
                .iter()
                .map(|axis| axis[row])
                .chain(std::iter::once(position))
                .enumerate()
            {
                let value = sign * coordinate;
                let offset = 400 + row * 32 + col * 8;
                h[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
            }
        }
        if cancel() {
            return Err(io::ErrorKind::Interrupted.into());
        }
        output.write_all(&h)?;
        for chunk in self.values.chunks(65536) {
            if cancel() {
                return Err(io::ErrorKind::Interrupted.into());
            }
            output.write_all(chunk)?;
        }
        Ok(())
    }
}
