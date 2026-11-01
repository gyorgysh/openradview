// SPDX-License-Identifier: GPL-3.0-or-later
//! Numeric axes phantom: each nonspatial coordinate and source slice has its own value.
pub fn fixture(
    version: u8,
    big: bool,
    complex: bool,
    units: u32,
    step: f64,
    offset: f64,
) -> Vec<u8> {
    let mut bytes = vec![0u8; if version == 1 { 352 } else { 544 }];
    let mut put = |at: usize, value: u64, length: usize| {
        for i in 0..length {
            bytes[at + i] = (value >> ((if big { length - 1 - i } else { i }) * 8)) as u8;
        }
    };
    let dims = [16, 12, 3, 3, 2, 2, 2];
    put(0, if version == 1 { 348 } else { 540 }, 4);
    if version == 1 {
        put(40, 7, 2);
        for (axis, count) in dims.into_iter().enumerate() {
            put(42 + axis * 2, count, 2);
        }
        put(70, if complex { 32 } else { 16 }, 2);
        put(72, if complex { 64 } else { 32 }, 2);
        put(108, 352f32.to_bits() as u64, 4);
        put(112, 1f32.to_bits() as u64, 4);
        put(92, (step as f32).to_bits() as u64, 4);
        put(136, (offset as f32).to_bits() as u64, 4);
        put(254, 1, 2);
        for (at, value) in [(280, 1f32), (300, 2.), (320, 3.)] {
            put(at, value.to_bits() as u64, 4);
        }
        bytes[123] = units as u8;
        bytes[344..348].copy_from_slice(b"n+1\0");
    } else {
        put(12, if complex { 32 } else { 16 }, 2);
        put(14, if complex { 64 } else { 32 }, 2);
        put(16, 7, 8);
        for (axis, count) in dims.into_iter().enumerate() {
            put(24 + axis * 8, count, 8);
        }
        put(168, 544, 8);
        put(176, 1f64.to_bits(), 8);
        put(136, step.to_bits(), 8);
        put(216, offset.to_bits(), 8);
        put(348, 1, 4);
        put(500, units as u64, 4);
        for (at, value) in [(400, 1f64), (440, 2.), (480, 3.)] {
            put(at, value.to_bits(), 8);
        }
        bytes[4..12].copy_from_slice(b"n+2\0\r\n\x1a\n");
    }
    for axis7 in 0..2 {
        for axis6 in 0..2 {
            for component in 0..2 {
                for time in 0..3 {
                    for z in 0..3 {
                        for y in 0..12 {
                            for x in 0..16 {
                                let value = (axis7 * 10000
                                    + axis6 * 1000
                                    + component * 100
                                    + time * 10
                                    + z) as f32
                                    + y as f32 * 0.25
                                    + x as f32 / 64.;
                                for part in [value, value + 50000.].into_iter().take(if complex {
                                    2
                                } else {
                                    1
                                }) {
                                    bytes.extend_from_slice(
                                        &(if big {
                                            part.to_be_bytes()
                                        } else {
                                            part.to_le_bytes()
                                        }),
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    bytes
}
