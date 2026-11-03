// SPDX-License-Identifier: GPL-3.0-or-later
//! Standalone overlay planes (PS3.3 C.9.2, C.9.3) and display shutters
//! (C.7.6.11) supplied with a source image. Both are display layers only: they
//! never change samples, validity or statistics, and a shutter is not a privacy
//! redaction. Overlays may contain burned-in text, so labels and descriptions
//! are not kept and callers show layers only on explicit request.
use crate::load::LoadError;
use dicom_core::{PrimitiveValue, Tag};
use dicom_object::DefaultDicomObject;
use std::sync::Arc;

const MAX_OVERLAY_BITS: u64 = 64 * 1024 * 1024;
/// Layers are composed into one RGBA plane the size of the source image.
pub const MAX_LAYER_PIXELS: u64 = 16 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq)]
struct Overlay {
    rows: u32,
    columns: u32,
    /// One-based row and column of the overlay's first pixel in the image.
    origin: [i32; 2],
    /// Zero-based first image frame and frame count, or None for all frames.
    frames: Option<(u32, u32)>,
    bits: Arc<[u8]>,
}
impl Overlay {
    fn bit(&self, frame: u32, row: i64, column: i64) -> bool {
        let plane = match self.frames {
            None => 0,
            Some((first, count)) => match frame.checked_sub(first) {
                Some(plane) if plane < count => u64::from(plane),
                _ => return false,
            },
        };
        let r = row - i64::from(self.origin[0]);
        let c = column - i64::from(self.origin[1]);
        if r < 0 || c < 0 || r >= i64::from(self.rows) || c >= i64::from(self.columns) {
            return false;
        }
        let index = plane * u64::from(self.rows) * u64::from(self.columns)
            + r as u64 * u64::from(self.columns)
            + c as u64;
        self.bits
            .get((index / 8) as usize)
            .is_some_and(|byte| byte >> (index % 8) & 1 == 1)
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Shape {
    /// One-based inclusive left, right columns and upper, lower rows.
    Rectangle([i64; 4]),
    /// One-based centre row, column and radius in pixels.
    Circle([i64; 3]),
    /// One-based row, column vertices.
    Polygon(Vec<[f64; 2]>),
}
impl Shape {
    fn exposes(&self, row: i64, column: i64) -> bool {
        match self {
            Self::Rectangle([left, right, upper, lower]) => {
                (*left..=*right).contains(&column) && (*upper..=*lower).contains(&row)
            }
            Self::Circle([center_row, center_column, radius]) => {
                (row - center_row).pow(2) + (column - center_column).pow(2) <= radius.pow(2)
            }
            Self::Polygon(vertices) => {
                let (y, x) = (row as f64, column as f64);
                let mut inside = false;
                for (i, a) in vertices.iter().enumerate() {
                    let b = vertices[(i + 1) % vertices.len()];
                    if (a[0] > y) != (b[0] > y)
                        && x < (b[1] - a[1]) * (y - a[0]) / (b[0] - a[0]) + a[1]
                    {
                        inside = !inside;
                    }
                }
                inside
            }
        }
    }
}

/// Supported layers for one source object, with the count of overlay groups
/// or shutter declarations that are present but outside the supported subset.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Layers {
    overlays: Vec<Overlay>,
    shapes: Vec<Shape>,
    /// Display gray for obscured pixels, 0 (black) to 65535.
    shutter_value: u16,
    pub unsupported: u32,
}
impl Layers {
    pub fn overlay_count(&self) -> usize {
        self.overlays.len()
    }
    pub fn has_shutter(&self) -> bool {
        !self.shapes.is_empty()
    }
    pub fn is_empty(&self) -> bool {
        self.overlays.is_empty() && self.shapes.is_empty() && self.unsupported == 0
    }
    /// Straight-alpha RGBA layer for one source frame. Overlay bits are opaque
    /// white; obscured pixels take the shutter gray. Other pixels are clear.
    pub fn rgba(
        &self,
        frame: u32,
        width: u32,
        height: u32,
        overlays: bool,
        shutter: bool,
    ) -> Option<Vec<u8>> {
        let count = u64::from(width) * u64::from(height);
        if count == 0 || count > MAX_LAYER_PIXELS {
            return None;
        }
        let gray = (u32::from(self.shutter_value) * 255 / 65535) as u8;
        let mut rgba = Vec::new();
        rgba.try_reserve_exact(count as usize * 4).ok()?;
        for row in 1..=i64::from(height) {
            for column in 1..=i64::from(width) {
                let obscured =
                    shutter && self.shapes.iter().any(|shape| !shape.exposes(row, column));
                let marked = overlays
                    && self
                        .overlays
                        .iter()
                        .any(|overlay| overlay.bit(frame, row, column));
                rgba.extend_from_slice(&if marked {
                    [255, 255, 255, 255]
                } else if obscured {
                    [gray, gray, gray, 255]
                } else {
                    [0, 0, 0, 0]
                });
            }
        }
        Some(rgba)
    }
}

fn integers(object: &DefaultDicomObject, tag: Tag) -> Option<Vec<i64>> {
    object.element(tag).ok()?.to_multi_int::<i64>().ok()
}

fn overlay(
    object: &DefaultDicomObject,
    group: u16,
    image_frames: u32,
) -> Result<Option<Overlay>, ()> {
    let tag = |element| Tag(group, element);
    let present = object
        .iter()
        .any(|element| element.header().tag.group() == group);
    if !present {
        return Ok(None);
    }
    let single = |element| match integers(object, tag(element)).as_deref() {
        Some([value]) => Ok(*value),
        _ => Err(()),
    };
    let rows = single(0x0010)?;
    let columns = single(0x0011)?;
    if !(1..=8192).contains(&rows)
        || !(1..=8192).contains(&columns)
        || single(0x0100)? != 1
        || single(0x0102)? != 0
        || !matches!(
            object
                .element(tag(0x0040))
                .ok()
                .and_then(|value| value.to_str().ok())
                .as_deref()
                .map(str::trim),
            Some("G" | "R")
        )
    {
        return Err(());
    }
    let origin = match integers(object, tag(0x0050)).as_deref() {
        Some([row, column]) => [
            i32::try_from(*row).map_err(|_| ())?,
            i32::try_from(*column).map_err(|_| ())?,
        ],
        _ => return Err(()),
    };
    let frames = match (
        integers(object, tag(0x0015)).as_deref(),
        integers(object, tag(0x0051)).as_deref(),
    ) {
        (None, None) => None,
        (Some([count]), origin) => {
            let first = match origin {
                None => 1,
                Some([first]) => *first,
                Some(_) => return Err(()),
            };
            if *count < 1 || first < 1 || first - 1 + count > i64::from(image_frames) {
                return Err(());
            }
            Some(((first - 1) as u32, *count as u32))
        }
        _ => return Err(()),
    };
    let bits = (rows as u64)
        .checked_mul(columns as u64)
        .and_then(|n| n.checked_mul(u64::from(frames.map_or(1, |(_, count)| count))))
        .filter(|n| *n <= MAX_OVERLAY_BITS)
        .ok_or(())?;
    let bytes: Vec<u8> = match object
        .element(tag(0x3000))
        .map_err(|_| ())?
        .value()
        .primitive()
        .ok_or(())?
    {
        PrimitiveValue::U8(values) => values.to_vec(),
        // OW cells start in the low bit of each native word.
        PrimitiveValue::U16(values) => values.iter().flat_map(|w| w.to_le_bytes()).collect(),
        _ => return Err(()),
    };
    if (bytes.len() as u64) < bits.div_ceil(8) {
        return Err(());
    }
    Ok(Some(Overlay {
        rows: rows as u32,
        columns: columns as u32,
        origin,
        frames,
        bits: bytes.into(),
    }))
}

fn shapes(object: &DefaultDicomObject) -> Result<Vec<Shape>, ()> {
    let Ok(element) = object.element(Tag(0x0018, 0x1600)) else {
        return Ok(Vec::new());
    };
    let names = element.to_str().map_err(|_| ())?;
    let mut shapes = Vec::new();
    for name in names.split('\\').map(str::trim) {
        let values = |element| integers(object, Tag(0x0018, element)).ok_or(());
        let single = |element| match values(element)?.as_slice() {
            [value] => Ok(*value),
            _ => Err(()),
        };
        shapes.push(match name {
            "RECTANGULAR" => {
                let edges = [
                    single(0x1602)?,
                    single(0x1604)?,
                    single(0x1606)?,
                    single(0x1608)?,
                ];
                if edges[0] > edges[1] || edges[2] > edges[3] {
                    return Err(());
                }
                Shape::Rectangle(edges)
            }
            "CIRCULAR" => match (values(0x1610)?.as_slice(), single(0x1612)?) {
                // Bounded so squared distances cannot overflow.
                ([row, column], radius)
                    if (0..=65536).contains(&radius)
                        && [*row, *column].iter().all(|v| v.abs() <= 65536) =>
                {
                    Shape::Circle([*row, *column, radius])
                }
                _ => return Err(()),
            },
            "POLYGONAL" => {
                let vertices = values(0x1620)?;
                if vertices.len() < 6 || vertices.len() % 2 != 0 || vertices.len() > 2048 {
                    return Err(());
                }
                Shape::Polygon(
                    vertices
                        .chunks(2)
                        .map(|pair| [pair[0] as f64, pair[1] as f64])
                        .collect(),
                )
            }
            _ => return Err(()),
        });
    }
    if shapes.is_empty() || shapes.len() > 3 {
        return Err(());
    }
    Ok(shapes)
}

/// Supported layers, or None when the object declares none. Unsupported layer
/// declarations are counted rather than refusing the image.
pub(crate) fn read(
    object: &DefaultDicomObject,
    image_frames: u32,
) -> Result<Option<Arc<Layers>>, LoadError> {
    let mut layers = Layers::default();
    for group in (0x6000..=0x601e).step_by(2) {
        match overlay(object, group, image_frames) {
            Ok(Some(overlay)) => layers.overlays.push(overlay),
            Ok(None) => {}
            Err(()) => layers.unsupported += 1,
        }
    }
    match shapes(object) {
        // Color shutter presentation needs a CIELab conversion; not supported.
        Ok(shapes) if !shapes.is_empty() && object.element(Tag(0x0018, 0x1624)).is_ok() => {
            layers.unsupported += 1;
        }
        Ok(shapes) => {
            layers.shapes = shapes;
            if let Some(value) = integers(object, Tag(0x0018, 0x1622)) {
                match value.as_slice() {
                    [value] if (0..=65535).contains(value) => {
                        layers.shutter_value = *value as u16;
                    }
                    _ => {
                        layers.shapes.clear();
                        layers.unsupported += 1;
                    }
                }
            }
        }
        Err(()) => layers.unsupported += 1,
    }
    Ok((!layers.is_empty()).then(|| Arc::new(layers)))
}
