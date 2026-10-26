// SPDX-License-Identifier: GPL-3.0-or-later
//! Direct/segmented palette expansion with bounded output and no recursive indirection.
use crate::load::{LoadError, integer};
use dicom_core::{DicomValue, PrimitiveValue, Tag};
use dicom_object::DefaultDicomObject;
pub(super) struct Palette {
    channels: [Vec<u16>; 3],
    first: i32,
    pub bits: u16,
}
impl Palette {
    pub fn read(object: &DefaultDicomObject) -> Result<Self, LoadError> {
        let mut descriptor = None;
        let mut channels = Vec::new();
        for channel in 1..=3 {
            let d = object
                .element(Tag(0x0028, 0x1100 + channel))
                .map_err(|_| LoadError::Unsupported)?
                .to_multi_int::<i32>()
                .map_err(|_| LoadError::Unsupported)?;
            if d.len() != 3 {
                return Err(LoadError::Unsupported);
            }
            let count = if d[0] as u16 == 0 {
                65536
            } else {
                d[0] as u16 as usize
            };
            let first = if integer(object, "PixelRepresentation") == Some(1) {
                d[1] as i16 as i32
            } else {
                d[1] as u16 as i32
            };
            let bits = d[2] as u16;
            if !matches!(bits, 8 | 16)
                || descriptor.is_some_and(|previous| previous != (count, first, bits))
            {
                return Err(LoadError::Unsupported);
            }
            descriptor = Some((count, first, bits));
            let direct = object.element(Tag(0x0028, 0x1200 + channel)).ok();
            let segmented = object.element(Tag(0x0028, 0x1220 + channel)).ok();
            let values = match (direct, segmented) {
                (Some(element), None) => {
                    let bytes = element.to_bytes().map_err(|_| LoadError::Unsupported)?;
                    let words = || -> Result<Vec<u16>, LoadError> {
                        match element.value() {
                            DicomValue::Primitive(PrimitiveValue::U16(values)) => {
                                Ok(values.to_vec())
                            }
                            DicomValue::Primitive(PrimitiveValue::U8(values))
                                if values.len().is_multiple_of(2) =>
                            {
                                Ok(values
                                    .as_chunks::<2>()
                                    .0
                                    .iter()
                                    .map(|b| {
                                        if object.meta().transfer_syntax() == "1.2.840.10008.1.2.2"
                                        {
                                            u16::from_be_bytes(*b)
                                        } else {
                                            u16::from_le_bytes(*b)
                                        }
                                    })
                                    .collect())
                            }
                            _ => Err(LoadError::Unsupported),
                        }
                    };
                    if bits == 16 || bytes.len() == count * 2 {
                        let values = words()?;
                        if values.len() != count {
                            return Err(LoadError::Unsupported);
                        }
                        values
                    } else if bits == 8 && bytes.len() == count + count % 2 {
                        let mut bytes = bytes.to_vec();
                        if matches!(
                            element.value(),
                            DicomValue::Primitive(PrimitiveValue::U16(_))
                        ) && cfg!(target_endian = "little")
                            && object.meta().transfer_syntax() == "1.2.840.10008.1.2.2"
                        {
                            for b in bytes.as_chunks_mut::<2>().0 {
                                b.swap(0, 1);
                            }
                        }
                        bytes[..count].iter().map(|b| *b as u16).collect()
                    } else {
                        return Err(LoadError::Unsupported);
                    }
                }
                (None, Some(element)) => {
                    let data = element
                        .to_multi_int::<u16>()
                        .map_err(|_| LoadError::Unsupported)?;
                    expand(&data, count)?
                }
                _ => return Err(LoadError::Unsupported),
            };
            let maximum = (1u32 << bits) - 1;
            if values.iter().any(|v| *v as u32 > maximum) {
                return Err(LoadError::Unsupported);
            }
            channels.push(values);
        }
        let (_, first, bits) = descriptor.ok_or(LoadError::Unsupported)?;
        Ok(Self {
            channels: channels.try_into().map_err(|_| LoadError::Unsupported)?,
            first,
            bits,
        })
    }
    pub fn at(&self, value: i32) -> [u16; 3] {
        let index = (value - self.first).clamp(0, self.channels[0].len() as i32 - 1) as usize;
        std::array::from_fn(|c| self.channels[c][index])
    }
}
#[derive(Clone, Copy)]
enum Segment {
    Discrete(usize, usize),
    Linear(usize, u16),
    Indirect(usize, usize),
}
fn expand(data: &[u16], count: usize) -> Result<Vec<u16>, LoadError> {
    let mut segments = Vec::new();
    let mut positions = std::collections::HashMap::new();
    let mut at = 0;
    while at < data.len() {
        if segments.len() >= count || at + 2 > data.len() {
            return Err(LoadError::Unsupported);
        }
        let (opcode, n) = (data[at], data[at + 1] as usize);
        if n == 0 {
            return Err(LoadError::Unsupported);
        }
        positions.insert(at, segments.len());
        let (segment, end) = match opcode {
            0 if at + 2 + n <= data.len() => (Segment::Discrete(at + 2, n), at + 2 + n),
            1 if at + 3 <= data.len() => (Segment::Linear(n, data[at + 2]), at + 3),
            2 if at + 4 <= data.len() => {
                let offset = data[at + 2] as usize | ((data[at + 3] as usize) << 16);
                if !offset.is_multiple_of(2) {
                    return Err(LoadError::Unsupported);
                }
                (Segment::Indirect(n, offset / 2), at + 4)
            }
            _ => return Err(LoadError::Unsupported),
        };
        segments.push(segment);
        at = end;
    }
    let mut out = Vec::with_capacity(count);
    for segment in &segments {
        if let Segment::Indirect(n, start) = segment {
            let index = *positions.get(start).ok_or(LoadError::Unsupported)?;
            let copied = segments
                .get(index..index.checked_add(*n).ok_or(LoadError::Limit)?)
                .ok_or(LoadError::Unsupported)?;
            for segment in copied {
                append(*segment, data, count, &mut out)?;
            }
        } else {
            append(*segment, data, count, &mut out)?;
        }
    }
    if out.len() != count {
        return Err(LoadError::Unsupported);
    }
    Ok(out)
}
fn append(
    segment: Segment,
    data: &[u16],
    count: usize,
    out: &mut Vec<u16>,
) -> Result<(), LoadError> {
    let length = match segment {
        Segment::Discrete(_, n) | Segment::Linear(n, _) => n,
        _ => return Err(LoadError::Unsupported),
    };
    if length > count.saturating_sub(out.len()) {
        return Err(LoadError::Unsupported);
    }
    match segment {
        Segment::Discrete(start, n) => out.extend_from_slice(&data[start..start + n]),
        Segment::Linear(n, end) => {
            let begin = *out.last().ok_or(LoadError::Unsupported)? as f64;
            for i in 1..=n {
                out.push((begin + (end as f64 - begin) * i as f64 / n as f64).round() as u16);
            }
        }
        Segment::Indirect(_, _) => return Err(LoadError::Unsupported),
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn segmented_discrete_linear_and_indirect_have_exact_bounds() {
        assert_eq!(
            expand(&[0, 1, 0, 1, 3, 300, 2, 2, 0, 0], 8).unwrap(),
            [0, 100, 200, 300, 0, 100, 200, 300]
        );
        for data in [
            vec![1, 2, 1],
            vec![2, 1, 0, 0],
            vec![0, 1, 0, 2, 1, 6, 0],
            vec![0, 4, 1, 2],
            vec![3, 1, 0],
        ] {
            assert!(expand(&data, 4).is_err());
        }
    }
}
