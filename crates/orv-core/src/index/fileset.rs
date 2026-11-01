// SPDX-License-Identifier: GPL-3.0-or-later
//! Case resolution within one retained directory capability, never ambient paths.
use super::{
    import::MAX_ENTRIES,
    source::{Encoding, Source, Stamp, scoped_open},
};
use crate::load::LoadError;
use cap_std::fs::Dir;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};

pub(super) struct FileSet {
    root: Arc<Dir>,
    names: HashMap<PathBuf, HashMap<String, Option<String>>>,
    entries: usize,
}
impl FileSet {
    pub fn new(parent: &Path) -> Result<Self, LoadError> {
        let root = Dir::open_ambient_dir(parent, cap_std::ambient_authority())
            .map_err(|_| LoadError::Io)?;
        Ok(Self {
            root: Arc::new(root),
            names: HashMap::new(),
            entries: 0,
        })
    }
    pub fn resolve(
        &mut self,
        components: &[String],
        cancelled: &impl Fn() -> bool,
    ) -> Result<Option<Source>, LoadError> {
        let mut relative = PathBuf::new();
        for component in components {
            if cancelled() {
                return Err(LoadError::Cancelled);
            }
            if !self.names.contains_key(&relative) {
                let directory = self
                    .root
                    .open_dir(if relative.as_os_str().is_empty() {
                        Path::new(".")
                    } else {
                        &relative
                    })
                    .map_err(|_| LoadError::Directory)?;
                let mut names = HashMap::new();
                for entry in directory.entries().map_err(|_| LoadError::Directory)? {
                    if cancelled() {
                        return Err(LoadError::Cancelled);
                    }
                    self.entries += 1;
                    if self.entries > MAX_ENTRIES {
                        return Err(LoadError::Limit);
                    }
                    let name = entry.map_err(|_| LoadError::Directory)?.file_name();
                    let Some(name) = name
                        .to_str()
                        .filter(|name| name.len() <= 8 && name.is_ascii())
                    else {
                        continue;
                    };
                    names
                        .entry(name.to_ascii_lowercase())
                        .and_modify(|value| *value = None)
                        .or_insert_with(|| Some(name.to_owned()));
                }
                self.names.insert(relative.clone(), names);
            }
            let names = self.names.get(&relative).ok_or(LoadError::Directory)?;
            let Some(name) = names.get(&component.to_ascii_lowercase()) else {
                return Ok(None);
            };
            relative.push(name.as_ref().ok_or(LoadError::Directory)?);
            let metadata = self
                .root
                .symlink_metadata(&relative)
                .map_err(|_| LoadError::Directory)?;
            if super::source::is_link(&metadata) {
                return Err(LoadError::Directory);
            }
        }
        let file = scoped_open(&self.root, &relative).map_err(|_| LoadError::Directory)?;
        let metadata = file.metadata().map_err(|_| LoadError::Io)?;
        Ok(Some(Source {
            id: 0,
            path: relative,
            scope: Some(self.root.clone()),
            stamp: Stamp::new(&metadata),
            offset: 0,
            length: metadata.len(),
            compressed: metadata.len(),
            encoding: Encoding::Stored,
        }))
    }
}
