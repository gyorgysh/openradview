// SPDX-License-Identifier: GPL-3.0-or-later
//! Local-only bounded loading. Never stores paths or raw tags in the resulting study.
use crate::{
    WindowLevel,
    study::{Frame, Series, Study},
};
use dicom_object::DefaultDicomObject;
use dicom_pixeldata::PixelDecoder;
use std::{
    fs,
    io::{Cursor, Read, Seek},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

const MAX_FILE: u64 = 64 * 1024 * 1024;
const MAX_SAMPLES: u64 = 32 * 1024 * 1024;
const MAX_STUDY_BYTES: u64 = 512 * 1024 * 1024;
const MAX_ENTRIES: usize = 20_000;

#[cfg(test)]
#[path = "load_tests.rs"]
mod tests;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadError {
    Io,
    Unsupported,
    Limit,
    Cancelled,
    Changed,
    Empty,
    ScalarRequired,
}
impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Io => "Could not read local data. Check file permissions.",
            Self::Unsupported => "Unsupported or invalid image. Check the supported DICOM, color and NIfTI formats in Help.",
            Self::Limit => "The input exceeds a file, image, entry or memory limit. Open a smaller series.",
            Self::Cancelled => "Import cancelled.",
            Self::Changed => "The selected source changed. Close and reopen the study.",
            Self::ScalarRequired => "This operation needs scalar pixel values.",
            Self::Empty => "No supported DICOM or NIfTI images were found.",
        })
    }
}
impl std::error::Error for LoadError {}

pub(crate) fn number(obj: &DefaultDicomObject, name: &str) -> Option<f64> {
    obj.element_by_name(name)
        .ok()?
        .to_float64()
        .ok()
        .filter(|v| v.is_finite())
}
pub(crate) fn text(obj: &DefaultDicomObject, name: &str) -> Option<String> {
    Some(
        obj.element_by_name(name)
            .ok()?
            .to_str()
            .ok()?
            .trim()
            .to_owned(),
    )
}
pub(crate) fn identifier(
    obj: &DefaultDicomObject,
    name: &str,
) -> Result<Option<String>, LoadError> {
    let value = text(obj, name).filter(|value| !value.is_empty());
    if value.as_ref().is_some_and(|value| !valid_identifier(value)) {
        return Err(LoadError::Unsupported);
    }
    Ok(value)
}
pub(crate) fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.split('.').all(|part| {
            !part.is_empty()
                && part.bytes().all(|byte| byte.is_ascii_digit())
                && (part.len() == 1 || !part.starts_with('0'))
        })
}
pub(crate) fn integer(obj: &DefaultDicomObject, name: &str) -> Option<u64> {
    obj.element_by_name(name).ok()?.to_int::<u64>().ok()
}

pub(crate) fn decode(bytes: &[u8]) -> Result<Series, LoadError> {
    if bytes.len() as u64 > MAX_FILE {
        return Err(LoadError::Limit);
    }
    let obj = dicom_object::from_reader(Cursor::new(bytes)).map_err(|_| LoadError::Unsupported)?;
    let width = integer(&obj, "Columns").ok_or(LoadError::Unsupported)?;
    let height = integer(&obj, "Rows").ok_or(LoadError::Unsupported)?;
    let count = integer(&obj, "NumberOfFrames").unwrap_or(1);
    let samples = width
        .checked_mul(height)
        .and_then(|n| n.checked_mul(count))
        .ok_or(LoadError::Limit)?;
    if width == 0
        || height == 0
        || width > 8192
        || height > 8192
        || count == 0
        || samples > MAX_SAMPLES
    {
        return Err(LoadError::Limit);
    }
    let color = crate::dicom_color::layout(&obj)?;
    let padding = crate::dicom_padding::Padding::read(&obj)?;
    if color.is_some() && samples * 3 > MAX_SAMPLES {
        return Err(LoadError::Limit);
    }
    let photometric = text(&obj, "PhotometricInterpretation").ok_or(LoadError::Unsupported)?;
    // Avoid silently treating unsupported nonlinear modality LUTs as affine rescale.
    if obj.element_by_name("ModalityLUTSequence").is_ok() {
        return Err(LoadError::Unsupported);
    }
    crate::dicom_codec::validate(&obj)?;
    let grayscale = crate::dicom_voi::read(&obj, count as u32, &|| false)?;
    let decoded = if color == Some(crate::dicom_color::Color::PackedYbr) {
        None
    } else {
        Some(
            obj.decode_pixel_data()
                .map_err(|_| LoadError::Unsupported)?,
        )
    };
    if decoded.as_ref().is_some_and(|decoded| {
        decoded.rows() as u64 != height
            || decoded.columns() as u64 != width
            || decoded.number_of_frames() as u64 != count
    }) {
        return Err(LoadError::Unsupported);
    }
    let series_uid = identifier(&obj, "SeriesInstanceUID")?.ok_or(LoadError::Unsupported)?;
    let modality = match text(&obj, "Modality").as_deref() {
        Some("CT") => "CT",
        Some("MR") => "MR",
        Some("PT") => "PT",
        Some("DX") => "DX",
        Some("CR") => "CR",
        Some("XA") => "XA",
        Some("US") => "US",
        _ => "IMAGE",
    };
    let series_number = number(&obj, "SeriesNumber").unwrap_or(1.0) as i32;
    let instance = number(&obj, "InstanceNumber").unwrap_or(0.0) as i32;
    let reference = identifier(&obj, "FrameOfReferenceUID")?;
    let key = format!(
        "{}|{}|{}",
        identifier(&obj, "StudyInstanceUID")?.unwrap_or_default(),
        series_uid,
        reference.as_deref().unwrap_or_default()
    );
    let spatial = crate::dicom_geometry::read(&obj, count as u32, &|| false)?;
    let acquisition = spatial.uniform_acquisition();
    let uniform_spatial = spatial.uniform();
    let key = acquisition.map_or(key.clone(), |hash| {
        format!("{key}|{}", crate::scene::hex(&hash))
    });
    let mut frames = Vec::with_capacity(count as usize);
    for frame_index in 0..count as usize {
        let size = (width * height) as usize;
        let pixels: Vec<f32> = if color.is_none() {
            let scalar = grayscale
                .get(frame_index as u32)
                .ok_or(LoadError::Unsupported)?;
            let pixels = decoded
                .as_ref()
                .ok_or(LoadError::Unsupported)?
                .to_vec_frame_with_options(
                    frame_index as u32,
                    &crate::dicom_voi::conversion(scalar),
                )
                .map_err(|_| LoadError::Unsupported)?;
            if pixels.len() != size || pixels.iter().any(|v: &f32| !v.is_finite()) {
                return Err(LoadError::Unsupported);
            }
            pixels
        } else {
            Vec::new()
        };
        let validity = if color.is_none() {
            padding
                .map(|p| {
                    p.mask(
                        decoded.as_ref().ok_or(LoadError::Unsupported)?,
                        frame_index as u32,
                        &|| false,
                    )
                })
                .transpose()?
                .flatten()
        } else {
            None
        };
        let presentation = color
            .map(|mode| {
                crate::dicom_color::present(
                    &obj,
                    decoded.as_ref(),
                    mode,
                    frame_index as u32,
                    padding,
                    &|| false,
                )
            })
            .transpose()?;
        let default_window = if color.is_some() {
            WindowLevel {
                function: Default::default(),
                width: 256.,
                center: 128.,
            }
        } else {
            grayscale
                .get(frame_index as u32)
                .and_then(|data| data.choices().original())
                .unwrap_or_else(|| {
                    crate::dicom_padding::automatic_window(&pixels, validity.as_deref())
                })
        };
        frames.push(Arc::new(Frame {
            slab: None,
            scalar: grayscale.get(frame_index as u32).cloned(),
            validity: validity.clone(),
            color: presentation.as_ref().map(|p| p.source.clone()),
            nifti: None,
            rgba: presentation.as_ref().map(|p| p.rgba.clone()),
            width: width as u32,
            height: height as u32,
            pixels: pixels.into(),
            geometry: uniform_spatial
                .then(|| spatial.geometry_for(frame_index).cloned())
                .flatten(),
            instance,
            frame_index: frame_index as u32,
            monochrome1: photometric == "MONOCHROME1",
            default_window,
        }));
    }
    Ok(Series {
        acquisition,
        part: 0,
        volume: Default::default(),
        key,
        source: None,
        modality,
        number: series_number,
        frames,
    })
}

pub(crate) fn collect(
    path: &Path,
    files: &mut Vec<PathBuf>,
    cancel: &AtomicBool,
    visited: &mut usize,
) -> Result<(), LoadError> {
    let mut pending = vec![path.to_owned()];
    while let Some(path) = pending.pop() {
        if cancel.load(Ordering::Relaxed) {
            return Err(LoadError::Cancelled);
        }
        *visited += 1;
        if *visited > MAX_ENTRIES {
            return Err(LoadError::Limit);
        }
        let metadata = fs::symlink_metadata(&path).map_err(|_| LoadError::Io)?;
        if metadata.file_type().is_symlink() {
            continue;
        }
        if path
            .file_name()
            .is_some_and(|v| v.to_string_lossy().starts_with('.'))
        {
            continue;
        }
        if metadata.is_dir() {
            for entry in fs::read_dir(&path).map_err(|_| LoadError::Io)? {
                pending.push(entry.map_err(|_| LoadError::Io)?.path());
                if pending.len() + *visited > MAX_ENTRIES {
                    return Err(LoadError::Limit);
                }
            }
        } else if metadata.is_file() {
            files.push(path);
        }
    }
    Ok(())
}

struct Import<'a, F> {
    study: Study,
    bytes: u64,
    processed: usize,
    cancel: &'a AtomicBool,
    progress: F,
}
impl<F: FnMut(usize)> Import<'_, F> {
    fn archive(&mut self, reader: impl Read + Seek) -> Result<(), LoadError> {
        let mut archive = zip::ZipArchive::new(reader).map_err(|_| LoadError::Unsupported)?;
        if archive.len() > MAX_ENTRIES {
            return Err(LoadError::Limit);
        }
        let mut expanded = 0u64;
        for i in 0..archive.len() {
            if self.cancel.load(Ordering::Relaxed) {
                return Err(LoadError::Cancelled);
            }
            let mut entry = archive.by_index(i).map_err(|_| LoadError::Unsupported)?;
            if entry.is_dir() {
                continue;
            }
            // Nothing is extracted; reject traversal/symlinks anyway.
            if entry.enclosed_name().is_none() || entry.is_symlink() {
                return Err(LoadError::Unsupported);
            }
            expanded = expanded.checked_add(entry.size()).ok_or(LoadError::Limit)?;
            if entry.size() > MAX_FILE || expanded > MAX_STUDY_BYTES {
                return Err(LoadError::Limit);
            }
            let mut bytes = Vec::new();
            entry
                .by_ref()
                .take(MAX_FILE + 1)
                .read_to_end(&mut bytes)
                .map_err(|_| LoadError::Io)?;
            self.add(&bytes)?;
        }
        Ok(())
    }

    fn finish(mut self) -> Result<Study, LoadError> {
        if self.study.series.is_empty() {
            return Err(LoadError::Empty);
        }
        for series in &mut self.study.series {
            series.sort();
        }
        self.study.series.sort_by_key(|s| s.number);
        Ok(self.study)
    }

    fn add(&mut self, bytes: &[u8]) -> Result<(), LoadError> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(LoadError::Cancelled);
        }
        self.processed += 1;
        if self.processed > MAX_ENTRIES {
            return Err(LoadError::Limit);
        }
        match decode(bytes) {
            Ok(Series {
                key,
                modality,
                number,
                frames,
                source: _,
                volume: _,
                acquisition,
                part: _,
            }) => {
                self.bytes += frames.iter().map(|f| f.byte_len()).sum::<u64>();
                if self.bytes > MAX_STUDY_BYTES {
                    return Err(LoadError::Limit);
                }
                if let Some(series) = self.study.series.iter_mut().find(|s| s.key == key) {
                    series.frames.extend(frames);
                } else {
                    self.study.series.push(Series {
                        acquisition,
                        part: 0,
                        volume: Default::default(),
                        key,
                        source: None,
                        modality,
                        number,
                        frames,
                    });
                }
            }
            Err(LoadError::Unsupported) => self.study.skipped_files += 1,
            Err(e) => return Err(e),
        }
        (self.progress)(self.processed);
        Ok(())
    }
}

/// Decodes a bounded, embedded demo ZIP on a worker, without filesystem writes.
/// Uses the same decoder and limits as the resident file loader.
pub fn load_zip(
    bytes: &[u8],
    cancel: &AtomicBool,
    progress: impl FnMut(usize),
) -> Result<Study, LoadError> {
    if bytes.len() as u64 > MAX_FILE {
        return Err(LoadError::Limit);
    }
    let mut import = Import {
        study: Study::default(),
        bytes: 0,
        processed: 0,
        cancel,
        progress,
    };
    import.archive(Cursor::new(bytes))?;
    import.finish()
}

/// Loads files/folders or ZIPs in memory. Call on a worker; progress contains counts only.
pub fn load_paths(
    paths: &[PathBuf],
    cancel: &AtomicBool,
    progress: impl FnMut(usize),
) -> Result<Study, LoadError> {
    let mut files = Vec::new();
    let mut visited = 0;
    for path in paths {
        collect(path, &mut files, cancel, &mut visited)?;
    }
    files.sort();
    let mut import = Import {
        study: Study::default(),
        bytes: 0,
        processed: 0,
        cancel,
        progress,
    };
    for path in files {
        if cancel.load(Ordering::Relaxed) {
            return Err(LoadError::Cancelled);
        }
        let mut file = fs::File::open(&path).map_err(|_| LoadError::Io)?;
        if path
            .extension()
            .is_some_and(|v| v.eq_ignore_ascii_case("zip"))
        {
            import.archive(file)?;
        } else {
            if file.metadata().map_err(|_| LoadError::Io)?.len() > MAX_FILE {
                return Err(LoadError::Limit);
            }
            let mut bytes = Vec::new();
            file.by_ref()
                .take(MAX_FILE + 1)
                .read_to_end(&mut bytes)
                .map_err(|_| LoadError::Io)?;
            import.add(&bytes)?;
        }
    }
    import.finish()
}
