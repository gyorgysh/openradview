// SPDX-License-Identifier: GPL-3.0-or-later
//! Bounded aligned texture readback; one-pixel calls keep their 56-byte layout.
pub(in super::super) struct Packing {
    pub offsets: [u64; 4],
    pub strides: [u32; 4],
    pub status: u64,
    pub bytes: u64,
    size: [u32; 2],
}
impl Packing {
    pub(super) fn new(size: [u32; 2]) -> Result<Self, &'static str> {
        if size.contains(&0)
            || size[0] > 2048
            || size[1] > 16
            || u64::from(size[0]) * u64::from(size[1]) > 4096
        {
            return Err("Section probe batch exceeds its working bounds");
        }
        if size == [1; 2] {
            return Ok(Self {
                offsets: [0, 16, 32, 48],
                strides: [256; 4],
                status: 20,
                bytes: 56,
                size,
            });
        }
        let strides = [16, 4, 16, 8].map(|bytes| (size[0] * bytes).next_multiple_of(256));
        let mut offsets = [0; 4];
        let mut bytes = 0;
        for field in 0..4 {
            offsets[field] = bytes;
            bytes += u64::from(strides[field]) * u64::from(size[1]);
        }
        Ok(Self {
            offsets,
            strides,
            status: bytes,
            bytes: bytes + 4,
            size,
        })
    }
    pub(in super::super) fn packet(
        &self,
        data: &[u8],
        pixel: [u32; 2],
    ) -> Result<[u8; 56], &'static str> {
        if (0..2).any(|a| pixel[a] >= self.size[a]) {
            return Err("Invalid section probe packet coordinate");
        }
        let mut packet = [0; 56];
        for (field, (begin, bytes)) in [(0, 16), (16, 4), (32, 16), (48, 8)]
            .into_iter()
            .enumerate()
        {
            let source = self.offsets[field]
                + u64::from(pixel[1]) * u64::from(self.strides[field])
                + u64::from(pixel[0]) * bytes as u64;
            let value = data
                .get(source as usize..source as usize + bytes)
                .ok_or("Invalid section probe readback length")?;
            packet[begin..begin + bytes].copy_from_slice(value);
        }
        packet[20..24].copy_from_slice(
            data.get(self.status as usize..self.status as usize + 4)
                .ok_or("Invalid section probe status length")?,
        );
        Ok(packet)
    }
}
