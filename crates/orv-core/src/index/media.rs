// SPDX-License-Identifier: GPL-3.0-or-later
//! Directory references enter the same image index and cache as ordinary inputs.
use super::*;
use crate::index::{dicomdir, fileset::FileSet};

pub(super) struct Context<'a, F, P> {
    pub builder: &'a mut Builder,
    pub cancelled: &'a F,
    pub expanded: &'a mut u64,
    pub report: &'a mut Progress,
    pub progress: &'a mut P,
    pub summary: &'a mut ImportReport,
}
impl<F: Fn() -> bool, P: FnMut(Progress)> Context<'_, F, P> {
    fn tick(&mut self) -> Result<(), LoadError> {
        if (self.cancelled)() {
            return Err(LoadError::Cancelled);
        }
        self.report.files += 1;
        if self.report.files > MAX_ENTRIES {
            return Err(LoadError::Limit);
        }
        (self.progress)(*self.report);
        Ok(())
    }
    pub fn local(&mut self, path: &Path) -> Result<(), LoadError> {
        self.tick()?;
        let parent = path
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let mut files = FileSet::new(parent)?;
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or(LoadError::Directory)?;
        let source = files
            .resolve(&[name.to_owned()], self.cancelled)?
            .ok_or(LoadError::Directory)?;
        let directory = dicomdir::read(
            &mut source.reader(self.cancelled)?,
            source.length,
            self.cancelled,
        )?;
        self.summary.directory();
        self.report.checked_bytes = self.report.checked_bytes.saturating_add(directory.bytes);
        for reference in &directory.references {
            self.tick()?;
            let Some(source) = files.resolve(&reference.components, self.cancelled)? else {
                self.summary
                    .skip(self.report.files, SkipReason::MissingReference);
                continue;
            };
            self.reference(
                source.clone(),
                &mut source.reader(self.cancelled)?,
                reference,
            )?;
        }
        (self.progress)(*self.report);
        Ok(())
    }
    pub fn archive(
        &mut self,
        archive: &mut zip::ZipArchive<File>,
        path: &Path,
        stamp: &Stamp,
    ) -> Result<bool, LoadError> {
        let mut selected = None;
        for index in 0..archive.len() {
            if (self.cancelled)() {
                return Err(LoadError::Cancelled);
            }
            let entry = archive
                .by_index(index)
                .map_err(|_| LoadError::Unsupported)?;
            let name = entry.name().map_err(|_| LoadError::Unsupported)?;
            if !entry.is_dir()
                && name
                    .rsplit('/')
                    .next()
                    .is_some_and(|name| name.eq_ignore_ascii_case("DICOMDIR"))
                && selected.replace(index).is_some()
            {
                return Err(LoadError::Directory);
            }
        }
        let Some(index) = selected else {
            return Ok(false);
        };
        let mut members = HashMap::new();
        let mut namespace = HashMap::new();
        let mut name_bytes = 0usize;
        for index in 0..archive.len() {
            if (self.cancelled)() {
                return Err(LoadError::Cancelled);
            }
            let entry = archive.by_index(index).map_err(|_| LoadError::Directory)?;
            let name = entry.name().map_err(|_| LoadError::Directory)?;
            if entry.is_symlink()
                || entry.enclosed_name().is_none()
                || name.contains(['\\', ':'])
                || name
                    .trim_end_matches('/')
                    .split('/')
                    .any(|part| part.is_empty() || matches!(part, "." | ".."))
            {
                return Err(LoadError::Directory);
            }
            let mut prefix = String::new();
            let mut components = name.trim_end_matches('/').split('/').peekable();
            let mut depth = 0;
            while let Some(component) = components.next() {
                if depth >= 64 {
                    return Err(LoadError::Limit);
                }
                depth += 1;
                if !prefix.is_empty() {
                    prefix.push('/')
                }
                prefix.push_str(component);
                let key = prefix.to_ascii_lowercase();
                let directory = components.peek().is_some() || entry.is_dir();
                if let Some((previous, was_directory)) = namespace.get(&key) {
                    if previous != &prefix || directory != *was_directory {
                        return Err(LoadError::Directory);
                    }
                } else {
                    name_bytes = name_bytes
                        .checked_add(prefix.len())
                        .ok_or(LoadError::Limit)?;
                    if name_bytes > 16 * 1024 * 1024 || namespace.len() >= MAX_ENTRIES {
                        return Err(LoadError::Limit);
                    }
                    namespace.insert(key, (prefix.clone(), directory));
                }
            }
            if entry.is_dir() {
                continue;
            }
            *self.expanded = self
                .expanded
                .checked_add(entry.size())
                .ok_or(LoadError::Limit)?;
            if *self.expanded > MAX_EXPANDED {
                return Err(LoadError::Limit);
            }
            name_bytes = name_bytes.checked_add(name.len()).ok_or(LoadError::Limit)?;
            if name_bytes > 16 * 1024 * 1024 {
                return Err(LoadError::Limit);
            }
            if members.insert(name.to_ascii_lowercase(), index).is_some() {
                return Err(LoadError::Directory);
            }
        }
        self.tick()?;
        let mut entry = archive.by_index(index).map_err(|_| LoadError::Directory)?;
        let name = entry.name().map_err(|_| LoadError::Directory)?;
        let prefix = name
            .rsplit_once('/')
            .map_or(String::new(), |(prefix, _)| format!("{prefix}/"))
            .to_ascii_lowercase();
        if !matches!(
            entry.compression(),
            zip::CompressionMethod::Stored | zip::CompressionMethod::Deflated
        ) {
            return Err(LoadError::Directory);
        }
        let length = entry.size();
        let directory = dicomdir::read(&mut entry, length, self.cancelled)?;
        drop(entry);
        self.summary.directory();
        self.report.checked_bytes = self.report.checked_bytes.saturating_add(directory.bytes);
        for reference in &directory.references {
            self.tick()?;
            let name = format!(
                "{prefix}{}",
                reference.components.join("/").to_ascii_lowercase()
            );
            let Some(&index) = members.get(&name) else {
                self.summary
                    .skip(self.report.files, SkipReason::MissingReference);
                continue;
            };
            let mut entry = archive.by_index(index).map_err(|_| LoadError::Directory)?;
            let encoding = match entry.compression() {
                zip::CompressionMethod::Stored => Encoding::Stored,
                zip::CompressionMethod::Deflated => Encoding::Deflated,
                _ => {
                    self.summary
                        .skip(self.report.files, SkipReason::UnsupportedEncoding);
                    continue;
                }
            };
            let source = Source {
                id: 0,
                path: path.to_owned(),
                scope: None,
                stamp: stamp.clone(),
                offset: entry.data_start().ok_or(LoadError::Directory)?,
                length: entry.size(),
                compressed: entry.compressed_size(),
                encoding,
            };
            self.reference(source, &mut entry, reference)?;
            drain_entry(&mut entry, self.cancelled, self.report, self.progress)?;
        }
        (self.progress)(*self.report);
        Ok(true)
    }
    fn reference(
        &mut self,
        source: Source,
        reader: &mut impl Read,
        reference: &dicomdir::Reference,
    ) -> Result<(), LoadError> {
        if !reference.image {
            self.summary
                .skip(self.report.files, SkipReason::UnsupportedObject);
            return Ok(());
        }
        match read_header_detailed(reader, self.cancelled) {
            Ok(header) => {
                self.report.checked_bytes = self.report.checked_bytes.saturating_add(header.offset);
                if !reference.matches(&header) {
                    self.summary
                        .skip(self.report.files, SkipReason::ConflictingReference);
                    return Ok(());
                }
                let source = prepare(
                    source,
                    &header,
                    self.cancelled,
                    self.expanded,
                    self.report,
                    self.progress,
                )?;
                self.builder.add(source, header, self.cancelled)?;
                self.summary.indexed();
            }
            Err(failure) if failure.error == LoadError::Unsupported => {
                self.summary.skip(self.report.files, failure.reason)
            }
            Err(failure) => return Err(failure.error),
        }
        Ok(())
    }
}
