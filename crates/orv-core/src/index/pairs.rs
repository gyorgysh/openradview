// SPDX-License-Identifier: GPL-3.0-or-later
//! Same-directory NIfTI companion resolution; never extract or trust header paths.
use super::{
    import::MAX_ENTRIES,
    source::{Encoding, Source, Stamp},
};
use crate::load::LoadError;
use std::{
    collections::HashMap,
    fs::{self, File},
    path::PathBuf,
};
pub(super) fn is_header(name: &str) -> bool {
    name.ends_with(".hdr") || name.ends_with(".hdr.gz")
}
pub(super) fn is_image(name: &str) -> bool {
    name.ends_with(".img") || name.ends_with(".img.gz")
}
pub(super) fn is_nifti(name: &str) -> bool {
    name.ends_with(".nii") || name.ends_with(".nii.gz") || is_header(name)
}
fn candidates(name: &str, from: &str, to: &str) -> Option<[String; 2]> {
    let bare = name.strip_suffix(".gz").unwrap_or(name);
    let stem = bare.strip_suffix(from)?;
    Some([format!("{stem}{to}"), format!("{stem}{to}.gz")])
}
#[derive(Default)]
pub(super) struct Resolver {
    directories: HashMap<PathBuf, HashMap<String, Option<String>>>,
    entries: usize,
}
impl Resolver {
    pub fn companion(
        &mut self,
        path: &std::path::Path,
        image: bool,
        cancelled: &impl Fn() -> bool,
    ) -> Result<Option<PathBuf>, LoadError> {
        let parent = path.parent().ok_or(LoadError::Unsupported)?;
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or(LoadError::Unsupported)?
            .to_ascii_lowercase();
        let Some(candidates) = candidates(
            &name,
            if image { ".img" } else { ".hdr" },
            if image { ".hdr" } else { ".img" },
        ) else {
            return Ok(None);
        };
        if !self.directories.contains_key(parent) {
            let mut names = HashMap::new();
            for entry in fs::read_dir(parent).map_err(|_| LoadError::Io)? {
                if cancelled() {
                    return Err(LoadError::Cancelled);
                }
                self.entries += 1;
                if self.entries > MAX_ENTRIES {
                    return Err(LoadError::Limit);
                }
                let entry = entry.map_err(|_| LoadError::Io)?;
                if let Some(name) = entry.file_name().to_str() {
                    let key = name.to_ascii_lowercase();
                    names
                        .entry(key)
                        .and_modify(|value| *value = None)
                        .or_insert_with(|| Some(name.to_owned()));
                }
            }
            self.directories.insert(parent.to_owned(), names);
        }
        let names = self.directories.get(parent).ok_or(LoadError::Io)?;
        let mut found = None;
        for name in candidates {
            if let Some(name) = names.get(&name) {
                if found.is_some() {
                    return Err(LoadError::Unsupported);
                }
                found = Some(parent.join(name.as_ref().ok_or(LoadError::Unsupported)?));
            }
        }
        Ok(found)
    }
}
pub(super) fn zip_companion(
    name: &str,
    image: bool,
    members: &HashMap<String, Source>,
) -> Result<Option<(Source, bool)>, LoadError> {
    let Some(candidates) = candidates(
        name,
        if image { ".img" } else { ".hdr" },
        if image { ".hdr" } else { ".img" },
    ) else {
        return Ok(None);
    };
    let mut found = None;
    for name in candidates {
        if let Some(source) = members.get(&name) {
            if found.is_some() {
                return Err(LoadError::Unsupported);
            }
            found = Some((source.clone(), name.ends_with(".gz")));
        }
    }
    Ok(found)
}
pub(super) fn file_source(path: PathBuf) -> Result<Source, LoadError> {
    let metadata = fs::symlink_metadata(&path).map_err(|_| LoadError::Io)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(LoadError::Unsupported);
    }
    Ok(Source {
        id: 0,
        path,
        stamp: Stamp::new(&metadata),
        offset: 0,
        length: metadata.len(),
        compressed: metadata.len(),
        encoding: Encoding::Stored,
    })
}
pub(super) fn zip_sources(
    archive: &mut zip::ZipArchive<File>,
    path: &std::path::Path,
    stamp: &Stamp,
    cancelled: &impl Fn() -> bool,
) -> Result<HashMap<String, Source>, LoadError> {
    let mut sources = HashMap::new();
    for i in 0..archive.len() {
        if cancelled() {
            return Err(LoadError::Cancelled);
        }
        let entry = archive.by_index(i).map_err(|_| LoadError::Unsupported)?;
        if entry.is_dir() {
            continue;
        }
        if entry.enclosed_name().is_none() || entry.is_symlink() {
            return Err(LoadError::Unsupported);
        }
        let name = entry
            .name()
            .map_err(|_| LoadError::Unsupported)?
            .to_ascii_lowercase();
        if !is_header(&name) && !is_image(&name) {
            continue;
        }
        let encoding = match entry.compression() {
            zip::CompressionMethod::Stored => Encoding::Stored,
            zip::CompressionMethod::Deflated => Encoding::Deflated,
            _ => continue,
        };
        let source = Source {
            id: 0,
            path: path.to_owned(),
            stamp: stamp.clone(),
            offset: entry.data_start().ok_or(LoadError::Unsupported)?,
            length: entry.size(),
            compressed: entry.compressed_size(),
            encoding,
        };
        if sources.insert(name, source).is_some() {
            return Err(LoadError::Unsupported);
        }
    }
    Ok(sources)
}
