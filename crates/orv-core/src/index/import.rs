// SPDX-License-Identifier: GPL-3.0-or-later
//! Background metadata/CRC import; no retained full-study pixel buffers.
use super::*;
use super::{
    header::read_header_detailed,
    source::{Encoding, Stamp},
};
use crate::{
    import_report::{ImportError, ImportReport, InputFailure, SkipReason},
    study::{Series, Study},
};
use std::{
    collections::HashMap,
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
};
pub(super) const MAX_ENTRIES: usize = 200_000;
const MAX_IMAGES: usize = 1_000_000;
const MAX_SERIES: usize = 4096;
const MAX_METADATA: u64 = 128 * 1024 * 1024;
pub(super) const MAX_EXPANDED: u64 = 64 * 1024 * 1024 * 1024;
#[path = "media.rs"]
mod media;

struct Builder {
    study: Study,
    groups: HashMap<String, HashMap<Option<[u8; 32]>, usize>>,
    series: Vec<IndexedSeries>,
    cache: Arc<Mutex<Cache>>,
    budget: Arc<MemoryBudget>,
    metadata: u64,
    images: usize,
    next_source: u64,
}
impl Builder {
    fn add(
        &mut self,
        mut source: Source,
        header: Header,
        cancelled: &impl Fn() -> bool,
    ) -> Result<(), LoadError> {
        self.metadata = self
            .metadata
            .checked_add(match &header.format {
                super::header::Format::Nifti(nifti) => nifti.metadata_bytes,
                _ => header.offset,
            })
            .and_then(|bytes| bytes.checked_add(header.spatial_bytes()))
            .ok_or(LoadError::Limit)?;
        self.images = self
            .images
            .checked_add(header.frames as usize)
            .ok_or(LoadError::Limit)?;
        if self.metadata > MAX_METADATA || self.images > MAX_IMAGES {
            return Err(LoadError::Limit);
        }
        if header.native {
            let expected_bits = u64::from(header.width)
                * u64::from(header.height)
                * u64::from(header.bits)
                * u64::from(header.frames)
                * if let Ok(object) = header.dicom() {
                    crate::dicom_color::native_samples(object)?
                } else {
                    1
                };
            let expected = expected_bits.div_ceil(8);
            let length = header.pixel_length.ok_or(LoadError::Unsupported)?;
            if header.dicom().is_ok() && !length.is_multiple_of(2) {
                return Err(LoadError::Unsupported);
            }
            // The bounded prefix stops at Float Pixel Data, so a later integer
            // Pixel Data element would go unseen. Require the float payload to
            // end the object instead of guessing which element is the image.
            let float = header.pixel_vr == dicom_core::VR::OF;
            if expected > length
                || length - expected > 1
                || (float
                    && (length != expected
                        || header.offset.checked_add(length) != Some(source.length)))
                || header
                    .offset
                    .checked_add(length)
                    .is_none_or(|n| n > source.length)
            {
                return Err(LoadError::Unsupported);
            }
        }
        source.assign_id(self.next_source);
        self.next_source += 1;
        let source = Arc::new(source);
        let header = Arc::new(header);
        let groups = self.groups.entry(header.key.clone()).or_default();
        for frame in 0..header.frames {
            if frame % 1024 == 0 && cancelled() {
                return Err(LoadError::Cancelled);
            }
            let acquisition = header.acquisition_for(frame);
            let index = if let Some(index) = groups.get(&acquisition) {
                *index
            } else {
                let index = self.series.len();
                if index >= MAX_SERIES {
                    return Err(LoadError::Limit);
                }
                let part = if acquisition.is_some() {
                    groups.len() as u32 + u32::from(!groups.contains_key(&None))
                } else {
                    0
                };
                groups.insert(acquisition, index);
                let key = acquisition.map_or_else(
                    || header.key.clone(),
                    |hash| format!("{}|{}", header.key, crate::scene::hex(&hash)),
                );
                self.study.series.push(Series {
                    key,
                    acquisition,
                    part,
                    volume: Default::default(),
                    modality: header.modality,
                    number: header.series_number,
                    frames: Vec::new(),
                    source: None,
                });
                self.series.push(IndexedSeries {
                    frames: Vec::new(),
                    cache: self.cache.clone(),
                    budget: self.budget.clone(),
                    volume: Mutex::new(std::sync::Weak::new()),
                });
                index
            };
            self.series[index].frames.push(IndexedFrame {
                source: source.clone(),
                header: header.clone(),
                frame,
            });
        }
        Ok(())
    }
}

/// Indexes metadata, validates supported ZIP members' CRC by streaming, and never
/// retains full pixel objects or extracts archives to disk. Call on a worker.
pub fn index_paths(
    paths: &[PathBuf],
    cancelled: impl Fn() -> bool,
    progress: impl FnMut(Progress),
) -> Result<Study, LoadError> {
    index_paths_detailed(paths, cancelled, progress).map_err(|failure| failure.error)
}

/// Retains bounded, non-identifying reasons even when no image can be indexed.
pub fn index_paths_detailed(
    paths: &[PathBuf],
    cancelled: impl Fn() -> bool,
    progress: impl FnMut(Progress),
) -> Result<Study, ImportError> {
    let mut summary = ImportReport::default();
    index_paths_inner(paths, cancelled, progress, &mut summary).map_err(|error| ImportError {
        error,
        report: summary,
    })
}

fn index_paths_inner(
    paths: &[PathBuf],
    cancelled: impl Fn() -> bool,
    mut progress: impl FnMut(Progress),
    summary: &mut ImportReport,
) -> Result<Study, LoadError> {
    crate::resources::refresh_memory_policy();
    let mut resolver = super::pairs::Resolver::default();
    let files = collect(paths, &cancelled, &mut resolver)?;
    let mut builder = Builder {
        study: Study::default(),
        groups: HashMap::new(),
        series: Vec::new(),
        cache: Arc::new(Mutex::new(Cache::default())),
        budget: MemoryBudget::volumes(),
        metadata: 0,
        images: 0,
        next_source: 0,
    };
    let mut report = Progress::default();
    let mut expanded = 0u64;
    for path in files {
        if cancelled() {
            return Err(LoadError::Cancelled);
        }
        if super::dicomdir::is_name(&path) {
            if paths.contains(&path) {
                if paths.len() != 1 {
                    return Err(LoadError::Directory);
                }
                media::Context {
                    builder: &mut builder,
                    cancelled: &cancelled,
                    expanded: &mut expanded,
                    report: &mut report,
                    progress: &mut progress,
                    summary,
                }
                .local(&path)?;
            }
            // Open folder is an explicit scan, independent of the media index.
            continue;
        }
        let mut file = File::open(&path).map_err(|_| LoadError::Io)?;
        let metadata = file.metadata().map_err(|_| LoadError::Io)?;
        let stamp = Stamp::new(&metadata);
        if path
            .extension()
            .is_some_and(|s| s.eq_ignore_ascii_case("zip"))
        {
            let mut archive = zip::ZipArchive::new(file).map_err(|_| LoadError::Unsupported)?;
            if archive.len() > MAX_ENTRIES {
                return Err(LoadError::Limit);
            }
            if paths.contains(&path)
                && (media::Context {
                    builder: &mut builder,
                    cancelled: &cancelled,
                    expanded: &mut expanded,
                    report: &mut report,
                    progress: &mut progress,
                    summary,
                })
                .archive(&mut archive, &path, &stamp)?
            {
                continue;
            }
            let members = super::pairs::zip_sources(&mut archive, &path, &stamp, &cancelled)?;
            for index in 0..archive.len() {
                if cancelled() {
                    return Err(LoadError::Cancelled);
                }
                let mut entry = archive
                    .by_index(index)
                    .map_err(|_| LoadError::Unsupported)?;
                if entry.is_dir() {
                    continue;
                }
                if entry.enclosed_name().is_none() || entry.is_symlink() {
                    return Err(LoadError::Unsupported);
                }
                if super::dicomdir::is_name(Path::new(
                    entry.name().map_err(|_| LoadError::Unsupported)?.as_ref(),
                )) {
                    continue;
                }
                expanded = expanded.checked_add(entry.size()).ok_or(LoadError::Limit)?;
                if expanded > MAX_EXPANDED {
                    return Err(LoadError::Limit);
                }
                report.files += 1;
                if report.files > MAX_ENTRIES {
                    return Err(LoadError::Limit);
                }
                let encoding = match entry.compression() {
                    zip::CompressionMethod::Stored => Encoding::Stored,
                    zip::CompressionMethod::Deflated => Encoding::Deflated,
                    _ => {
                        summary.skip(report.files, SkipReason::UnsupportedEncoding);
                        progress(report);
                        continue;
                    }
                };
                let source = Source {
                    id: 0,
                    path: path.clone(),
                    scope: None,
                    stamp: stamp.clone(),
                    offset: entry.data_start().ok_or(LoadError::Unsupported)?,
                    length: entry.size(),
                    compressed: entry.compressed_size(),
                    encoding,
                };
                let name = entry
                    .name()
                    .map_err(|_| LoadError::Unsupported)?
                    .to_ascii_lowercase();
                if super::pairs::is_image(&name) {
                    drain_entry(&mut entry, &cancelled, &mut report, &mut progress)?;
                    if super::pairs::zip_companion(&name, true, &members)?.is_none() {
                        summary.skip(report.files, SkipReason::MissingCompanion);
                    }
                    continue;
                }
                let pair = if super::pairs::is_header(&name) {
                    super::pairs::zip_companion(&name, false, &members)?
                } else {
                    None
                };
                let input = if super::pairs::is_header(&name) && pair.is_none() {
                    Err(InputFailure::skipped(SkipReason::MissingCompanion))
                } else if super::pairs::is_nifti(&name) {
                    super::nifti_input::read(
                        super::nifti_input::NiftiInput {
                            source,
                            gzip: name.ends_with(".gz"),
                            paired: super::pairs::is_header(&name),
                            companion: pair,
                        },
                        builder.next_source,
                        &cancelled,
                        &mut expanded,
                        &mut report,
                        &mut progress,
                    )
                    .map_err(InputFailure::from)
                } else {
                    read_header_detailed(&mut entry, &cancelled)
                        .map(|header| (source, vec![header]))
                };
                match input {
                    Ok((source, headers)) => {
                        // The DICOM parser has consumed this prefix from `entry`.
                        // NIfTI uses independent range readers; its entry is still at zero.
                        if let Some(header) = headers.first()
                            && matches!(header.format, super::header::Format::Dicom(_))
                        {
                            report.checked_bytes =
                                report.checked_bytes.saturating_add(header.offset);
                        }
                        drain_entry(&mut entry, &cancelled, &mut report, &mut progress)?;
                        for header in headers {
                            let source = prepare(
                                source.clone(),
                                &header,
                                &cancelled,
                                &mut expanded,
                                &mut report,
                                &mut progress,
                            )?;
                            builder.add(source.clone(), header, &cancelled)?;
                        }
                        summary.indexed();
                    }
                    Err(failure) if failure.error == LoadError::Unsupported => {
                        summary.skip(report.files, failure.reason);
                    }
                    Err(failure) => return Err(failure.error),
                }
                progress(report);
            }
        } else {
            report.files += 1;
            let name = path.to_string_lossy().to_ascii_lowercase();
            let source = Source {
                id: 0,
                path,
                scope: None,
                stamp,
                offset: 0,
                length: metadata.len(),
                compressed: metadata.len(),
                encoding: Encoding::Stored,
            };
            let pair = if super::pairs::is_header(&name) {
                resolver
                    .companion(&source.path, false, &cancelled)?
                    .map(|path| {
                        let gzip = path.to_string_lossy().to_ascii_lowercase().ends_with(".gz");
                        super::pairs::file_source(path).map(|source| (source, gzip))
                    })
                    .transpose()?
            } else {
                None
            };
            let input = if super::pairs::is_image(&name)
                || (super::pairs::is_header(&name) && pair.is_none())
            {
                Err(InputFailure::skipped(SkipReason::MissingCompanion))
            } else if super::pairs::is_nifti(&name) {
                super::nifti_input::read(
                    super::nifti_input::NiftiInput {
                        source,
                        gzip: name.ends_with(".gz"),
                        paired: super::pairs::is_header(&name),
                        companion: pair,
                    },
                    builder.next_source,
                    &cancelled,
                    &mut expanded,
                    &mut report,
                    &mut progress,
                )
                .map_err(InputFailure::from)
            } else {
                read_header_detailed(&mut file, &cancelled).map(|header| (source, vec![header]))
            };
            match input {
                Ok((source, headers)) => {
                    for header in headers {
                        report.checked_bytes = report.checked_bytes.saturating_add(header.offset);
                        let source = prepare(
                            source.clone(),
                            &header,
                            &cancelled,
                            &mut expanded,
                            &mut report,
                            &mut progress,
                        )?;
                        builder.add(source.clone(), header, &cancelled)?;
                    }
                    summary.indexed();
                }
                Err(failure) if failure.error == LoadError::Unsupported => {
                    summary.skip(report.files, failure.reason);
                }
                Err(failure) => return Err(failure.error),
            }
            progress(report);
        }
    }
    if builder.series.is_empty() {
        return Err(LoadError::Empty);
    }
    for (series, mut indexed) in builder.study.series.iter_mut().zip(builder.series) {
        indexed.sort();
        series.source = Some(Arc::new(indexed));
    }
    builder.study.series.sort_by_key(|s| s.number);
    builder.study.skipped_files = summary.skipped_files();
    builder.study.import_report = std::mem::take(summary);
    Ok(builder.study)
}

fn collect(
    paths: &[PathBuf],
    cancelled: &impl Fn() -> bool,
    resolver: &mut super::pairs::Resolver,
) -> Result<Vec<PathBuf>, LoadError> {
    let mut pending = paths.to_vec();
    let mut files = Vec::new();
    let mut visited = 0usize;
    while let Some(path) = pending.pop() {
        if cancelled() {
            return Err(LoadError::Cancelled);
        }
        visited += 1;
        if visited + pending.len() > MAX_ENTRIES {
            return Err(LoadError::Limit);
        }
        let metadata = fs::symlink_metadata(&path).map_err(|_| LoadError::Io)?;
        if metadata.file_type().is_symlink() || hidden(&path) {
            continue;
        }
        if metadata.is_dir() {
            for entry in fs::read_dir(&path).map_err(|_| LoadError::Io)? {
                pending.push(entry.map_err(|_| LoadError::Io)?.path());
                if pending.len() + visited > MAX_ENTRIES {
                    return Err(LoadError::Limit);
                }
            }
        } else if metadata.is_file() {
            files.push(path);
        }
    }
    let mut resolved = Vec::new();
    for file in files {
        if super::pairs::is_image(&file.to_string_lossy().to_ascii_lowercase()) {
            if let Some(header) = resolver.companion(&file, true, cancelled)? {
                resolved.push(header);
            } else {
                resolved.push(file);
            }
        } else {
            resolved.push(file);
        }
    }
    resolved.sort();
    resolved.dedup();
    Ok(resolved)
}
fn hidden(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|s| s.to_string_lossy().starts_with('.'))
}

fn prepare(
    source: Source,
    header: &Header,
    cancelled: &impl Fn() -> bool,
    expanded: &mut u64,
    report: &mut Progress,
    progress: &mut impl FnMut(Progress),
) -> Result<Source, LoadError> {
    if let Some(start) = header.deflated_at {
        let source = source.inflate_dataset(
            start,
            MAX_EXPANDED.saturating_sub(*expanded),
            cancelled,
            |bytes| {
                report.checked_bytes = report.checked_bytes.saturating_add(bytes);
                progress(*report);
            },
        )?;
        *expanded = expanded
            .checked_add(source.length)
            .ok_or(LoadError::Limit)?;
        Ok(source)
    } else {
        Ok(source)
    }
}

fn drain_entry(
    reader: &mut impl Read,
    cancelled: &impl Fn() -> bool,
    report: &mut Progress,
    progress: &mut impl FnMut(Progress),
) -> Result<(), LoadError> {
    let mut scratch = [0u8; 64 * 1024];
    loop {
        if cancelled() {
            return Err(LoadError::Cancelled);
        }
        let n = reader
            .read(&mut scratch)
            .map_err(|_| LoadError::Unsupported)?;
        if n == 0 {
            break;
        }
        report.checked_bytes = report.checked_bytes.saturating_add(n as u64);
        progress(*report);
    }
    Ok(())
}
