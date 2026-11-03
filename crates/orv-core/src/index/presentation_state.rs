// SPDX-License-Identifier: GPL-3.0-or-later
//! An explicitly selected Grayscale Softcopy Presentation State (PS3.3 A.33.1),
//! applied whole or not at all. The supported subset is the spatial rotation and
//! horizontal flip, one softcopy window and the presentation LUT shape, with a
//! displayed area equal to the whole image. Source references stay private here.
use super::{IndexedSeries, source::Stamp};
use crate::{VoiFunction, WindowLevel, study::Series, view::DisplayTransform};
use dicom_core::Tag;
use dicom_object::InMemDicomObject as Object;
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::Read,
    path::Path,
};

const CLASS: &str = "1.2.840.10008.5.1.4.1.1.11.1";
const MAX_BYTES: u64 = 8 * 1024 * 1024;
const MAX_REFERENCES: usize = 65536;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PresentationError {
    Cancelled,
    Changed,
    Io,
    Kind,
    Metadata,
    Association,
    Limit,
    /// A declared feature outside the supported subset; nothing is applied.
    Unsupported(&'static str),
}
impl std::fmt::Display for PresentationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => f.write_str("Presentation state loading cancelled."),
            Self::Changed => f.write_str("The file changed while it was being read."),
            Self::Io => f.write_str("The selected file could not be read."),
            Self::Kind => f.write_str("Choose a grayscale softcopy presentation state file."),
            Self::Metadata => f.write_str("The presentation state is incomplete or malformed."),
            Self::Association => f.write_str(
                "The presentation state does not reference images in the selected loaded DICOM series.",
            ),
            Self::Limit => f.write_str("The presentation state exceeds the supported size."),
            Self::Unsupported(feature) => write!(
                f,
                "Not applied: this presentation state uses {feature}, which is not supported. Nothing was changed."
            ),
        }
    }
}
type Result<T> = std::result::Result<T, PresentationError>;

/// Display settings to apply while viewing the referenced images.
#[derive(Clone, Debug, PartialEq)]
pub struct PresentationState {
    /// Sorted indices of referenced images in the selected series.
    pub images: Vec<usize>,
    pub window: WindowLevel,
    /// Presentation LUT shape INVERSE: lower display values are brighter.
    pub inverse: bool,
    pub transform: DisplayTransform,
}

fn text(object: &Object, tag: Tag) -> Option<String> {
    object
        .element(tag)
        .ok()
        .and_then(|e| e.to_str().ok())
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
}
fn items(object: &Object, tag: Tag) -> Result<Option<&[Object]>> {
    let Ok(element) = object.element(tag) else {
        return Ok(None);
    };
    element
        .value()
        .items()
        .map(Some)
        .ok_or(PresentationError::Metadata)
}
fn single(object: &Object, tag: Tag) -> Result<f64> {
    let values = object
        .element(tag)
        .map_err(|_| PresentationError::Metadata)?
        .to_multi_float64()
        .map_err(|_| PresentationError::Metadata)?;
    match values.as_slice() {
        [value] if value.is_finite() => Ok(*value),
        _ => Err(PresentationError::Metadata),
    }
}

/// (SOP class, SOP instance, one-based frame) for every plane in the series.
type Planes = HashMap<(String, String), Vec<(u32, usize)>>;
fn planes(source: &IndexedSeries, cancel: &impl Fn() -> bool) -> Result<Planes> {
    let mut planes: Planes = HashMap::new();
    let mut checked = HashSet::new();
    for (index, entry) in source.frames.iter().enumerate() {
        if cancel() {
            return Err(PresentationError::Cancelled);
        }
        if checked.insert(entry.source.id) {
            entry
                .source
                .check()
                .map_err(|_| PresentationError::Changed)?;
        }
        let object = entry
            .header
            .dicom()
            .map_err(|_| PresentationError::Association)?;
        let class = object.meta().media_storage_sop_class_uid().to_owned();
        let instance = object.meta().media_storage_sop_instance_uid().to_owned();
        planes
            .entry((class, instance))
            .or_default()
            .push((entry.frame + 1, index));
    }
    Ok(planes)
}

/// Refuse rather than partly apply: each of these changes the picture.
fn refuse_unsupported(root: &Object) -> Result<()> {
    for (tag, feature) in [
        (Tag(0x0070, 0x0001), "graphic or text annotations"),
        (Tag(0x0070, 0x0060), "graphic layers"),
        (Tag(0x0018, 0x1600), "display shutters"),
        (Tag(0x0028, 0x3000), "a modality lookup table"),
        (Tag(0x0028, 0x1052), "a modality rescale"),
        (Tag(0x0028, 0x1053), "a modality rescale"),
        (Tag(0x0028, 0x6100), "mask subtraction"),
        (Tag(0x2050, 0x0010), "a presentation lookup table"),
        (Tag(0x0070, 0x0234), "graphic groups"),
    ] {
        if root.element(tag).is_ok() {
            return Err(PresentationError::Unsupported(feature));
        }
    }
    if root
        .iter()
        .any(|element| (0x6000..=0x601e).contains(&element.header().tag.group()))
    {
        return Err(PresentationError::Unsupported("overlay planes"));
    }
    Ok(())
}

fn window(root: &Object) -> Result<WindowLevel> {
    let items = items(root, Tag(0x0028, 0x3110))?
        .ok_or(PresentationError::Unsupported("no softcopy window"))?;
    let [item] = items else {
        return Err(PresentationError::Unsupported("several softcopy windows"));
    };
    if item.element(Tag(0x0008, 0x1140)).is_ok() {
        return Err(PresentationError::Unsupported(
            "image-specific softcopy windows",
        ));
    }
    if item.element(Tag(0x0028, 0x3010)).is_ok() {
        return Err(PresentationError::Unsupported("a VOI lookup table"));
    }
    let function = match text(item, Tag(0x0028, 0x1056)).as_deref() {
        None | Some("LINEAR") => VoiFunction::Linear,
        Some("LINEAR_EXACT") => VoiFunction::LinearExact,
        Some("SIGMOID") => VoiFunction::Sigmoid,
        Some(_) => return Err(PresentationError::Unsupported("an unknown VOI function")),
    };
    let window = WindowLevel {
        function,
        center: single(item, Tag(0x0028, 0x1050))? as f32,
        width: single(item, Tag(0x0028, 0x1051))? as f32,
    };
    if !window.is_valid() {
        return Err(PresentationError::Metadata);
    }
    Ok(window)
}

fn transform(root: &Object) -> Result<DisplayTransform> {
    let rotation = match root.element(Tag(0x0070, 0x0042)) {
        Err(_) => 0,
        Ok(element) => match element.to_int::<u16>() {
            Ok(value @ (0 | 90 | 180 | 270)) => value / 90,
            _ => return Err(PresentationError::Metadata),
        },
    };
    let flip = match text(root, Tag(0x0070, 0x0041)).as_deref() {
        None | Some("N") => false,
        Some("Y") => true,
        Some(_) => return Err(PresentationError::Metadata),
    };
    // The standard flips horizontally first, then rotates clockwise. A flip
    // reverses the rotation sense, so this is a counter-rotation followed by a
    // horizontal flip in displayed axes, matching DisplayTransform's order.
    Ok(DisplayTransform {
        quarter_turns: if flip {
            (4 - rotation as u8) % 4
        } else {
            rotation as u8
        },
        flip_horizontal: flip,
        flip_vertical: false,
    })
}

fn displayed_area(root: &Object, sizes: &[(usize, [u32; 2])]) -> Result<()> {
    let areas = items(root, Tag(0x0070, 0x005a))?
        .ok_or(PresentationError::Unsupported("no displayed area"))?;
    if areas.is_empty() {
        return Err(PresentationError::Metadata);
    }
    for area in areas {
        if text(area, Tag(0x0070, 0x0100)).as_deref() != Some("SCALE TO FIT") {
            return Err(PresentationError::Unsupported(
                "a true-size or magnified display",
            ));
        }
        let corner = |tag| -> Result<[i64; 2]> {
            area.element(tag)
                .ok()
                .and_then(|e| e.to_multi_int::<i64>().ok())
                .and_then(|v| v.try_into().ok())
                .ok_or(PresentationError::Metadata)
        };
        let (top_left, bottom_right) = (corner(Tag(0x0070, 0x0052))?, corner(Tag(0x0070, 0x0053))?);
        // Columns then rows, one-based. Only the whole image matches Fit.
        if top_left != [1, 1]
            || sizes
                .iter()
                .any(|(_, [w, h])| bottom_right != [i64::from(*w), i64::from(*h)])
        {
            return Err(PresentationError::Unsupported(
                "a displayed area other than the whole image",
            ));
        }
    }
    Ok(())
}

pub fn read_presentation_state(
    path: &Path,
    series: &Series,
    cancelled: impl Fn() -> bool,
) -> Result<PresentationState> {
    let result = read_inner(path, series, &cancelled);
    if cancelled() {
        Err(PresentationError::Cancelled)
    } else {
        result
    }
}
fn read_inner(
    path: &Path,
    series: &Series,
    cancel: &impl Fn() -> bool,
) -> Result<PresentationState> {
    let source = series
        .source
        .as_ref()
        .ok_or(PresentationError::Association)?;
    let before = fs::symlink_metadata(path).map_err(|_| PresentationError::Io)?;
    if !before.is_file() || before.file_type().is_symlink() {
        return Err(PresentationError::Io);
    }
    if before.len() > MAX_BYTES {
        return Err(PresentationError::Limit);
    }
    let stamp = Stamp::new(&before);
    let mut file = fs::File::open(path).map_err(|_| PresentationError::Io)?;
    let mut bytes = Vec::new();
    file.by_ref()
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| PresentationError::Io)?;
    if bytes.len() as u64 > MAX_BYTES
        || Stamp::new(&file.metadata().map_err(|_| PresentationError::Io)?) != stamp
    {
        return Err(PresentationError::Changed);
    }
    let object = dicom_object::from_reader(std::io::Cursor::new(bytes))
        .map_err(|_| PresentationError::Kind)?;
    if object.meta().media_storage_sop_class_uid() != CLASS
        || !matches!(
            object.meta().transfer_syntax(),
            "1.2.840.10008.1.2" | "1.2.840.10008.1.2.1"
        )
    {
        return Err(PresentationError::Kind);
    }
    refuse_unsupported(&object)?;
    let inverse = match text(&object, Tag(0x2050, 0x0020)).as_deref() {
        Some("IDENTITY") => false,
        Some("INVERSE") => true,
        _ => return Err(PresentationError::Metadata),
    };
    let window = window(&object)?;
    let transform = transform(&object)?;
    let planes = planes(source, cancel)?;
    let mut images = Vec::new();
    let mut count = 0usize;
    for referenced in items(&object, Tag(0x0008, 0x1115))?.ok_or(PresentationError::Metadata)? {
        for image in items(referenced, Tag(0x0008, 0x1140))?.ok_or(PresentationError::Metadata)? {
            count += 1;
            if count > MAX_REFERENCES || cancel() {
                return Err(if cancel() {
                    PresentationError::Cancelled
                } else {
                    PresentationError::Limit
                });
            }
            let class = text(image, Tag(0x0008, 0x1150)).ok_or(PresentationError::Metadata)?;
            let instance = text(image, Tag(0x0008, 0x1155)).ok_or(PresentationError::Metadata)?;
            // References outside this series would need another series' view.
            let frames = planes
                .get(&(class, instance))
                .ok_or(PresentationError::Association)?;
            match image.element(Tag(0x0008, 0x1160)).ok() {
                None => images.extend(frames.iter().map(|(_, index)| *index)),
                Some(element) => {
                    for number in element
                        .to_multi_int::<u32>()
                        .map_err(|_| PresentationError::Metadata)?
                    {
                        let (_, index) = frames
                            .iter()
                            .find(|(frame, _)| *frame == number)
                            .ok_or(PresentationError::Association)?;
                        images.push(*index);
                    }
                }
            }
        }
    }
    images.sort_unstable();
    images.dedup();
    if images.is_empty() {
        return Err(PresentationError::Association);
    }
    let sizes: Vec<(usize, [u32; 2])> = images
        .iter()
        .map(|index| {
            let entry = &source.frames[*index];
            (*index, [entry.header.width, entry.header.height])
        })
        .collect();
    displayed_area(&object, &sizes)?;
    Ok(PresentationState {
        images,
        window,
        inverse,
        transform,
    })
}
