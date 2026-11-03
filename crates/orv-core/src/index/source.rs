// SPDX-License-Identifier: GPL-3.0-or-later
//! Private local sources. Paths are never serialized, returned to agents or logged.
use crate::load::LoadError;
use std::{
    fs::{self, File, Metadata},
    io::{Read, Seek, SeekFrom},
    path::PathBuf,
    sync::Arc,
    time::SystemTime,
};

#[derive(Clone, PartialEq, Eq)]
pub(super) struct Stamp {
    length: u64,
    modified: Option<SystemTime>,
    #[cfg(unix)]
    identity: (u64, u64),
    // Windows copies preserve the last-write time, so a same-size replacement
    // is invisible to (length, modified). A replacement is a new file object
    // with a fresh creation time, which closes that gap with stable APIs.
    #[cfg(windows)]
    created: Option<SystemTime>,
}
impl Stamp {
    pub fn new(metadata: &Metadata) -> Self {
        Self {
            length: metadata.len(),
            modified: metadata.modified().ok(),
            #[cfg(unix)]
            identity: {
                use std::os::unix::fs::MetadataExt;
                (metadata.dev(), metadata.ino())
            },
            #[cfg(windows)]
            created: metadata.created().ok(),
        }
    }
}
#[derive(Clone)]
pub(super) enum Encoding {
    Stored,
    Deflated,
    Gzip {
        container: Box<Source>,
    },
    Dataset {
        container: Box<Source>,
        start: u64,
    },
    Paired {
        data: Box<Source>,
        header: Box<Source>,
    },
}
#[derive(Clone)]
pub(super) struct Source {
    pub id: u64,
    pub path: PathBuf,
    /// A selected file-set retains its directory handle for every subsequent read.
    pub scope: Option<Arc<cap_std::fs::Dir>>,
    pub stamp: Stamp,
    pub offset: u64,
    pub length: u64,
    pub compressed: u64,
    pub encoding: Encoding,
}
pub(super) struct Stream {
    source: u64,
    position: u64,
    reader: Box<dyn Read + Send>,
}
#[derive(Default)]
pub(super) struct ReadState {
    pub stream: Option<Stream>,
}
pub(super) fn is_link(metadata: &cap_std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use cap_std::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return true;
        }
    }
    metadata.file_type().is_symlink()
}
pub(super) fn scoped_open(
    root: &cap_std::fs::Dir,
    path: &std::path::Path,
) -> Result<File, LoadError> {
    let mut prefix = PathBuf::new();
    for component in path.components() {
        if !matches!(component, std::path::Component::Normal(_)) {
            return Err(LoadError::Changed);
        }
        prefix.push(component);
        let metadata = root
            .symlink_metadata(&prefix)
            .map_err(|_| LoadError::Changed)?;
        if is_link(&metadata) {
            return Err(LoadError::Changed);
        }
        if prefix == path && !metadata.is_file() {
            return Err(LoadError::Changed);
        }
    }
    // Capability lookup also contains symlinks substituted between these checks.
    let file = root.open(path).map_err(|_| LoadError::Changed)?.into_std();
    if !file.metadata().map_err(|_| LoadError::Io)?.is_file() {
        return Err(LoadError::Changed);
    }
    Ok(file)
}
impl Source {
    pub fn assign_id(&mut self, id: u64) {
        self.id = id;
        if let Encoding::Paired { data, .. } = &mut self.encoding {
            data.assign_id(id);
        }
    }
    pub(super) fn check(&self) -> Result<(), LoadError> {
        if let Encoding::Paired { header, .. } = &self.encoding {
            header.check()?;
        }
        if let Some(scope) = &self.scope {
            let file = scoped_open(scope, &self.path)?;
            return self.check_stamp(&file);
        }
        let metadata = fs::symlink_metadata(&self.path).map_err(|_| LoadError::Io)?;
        if !metadata.is_file()
            || metadata.file_type().is_symlink()
            || Stamp::new(&metadata) != self.stamp
        {
            return Err(LoadError::Changed);
        }
        Ok(())
    }
    fn open(&self) -> Result<File, LoadError> {
        if let Some(scope) = &self.scope {
            let file = scoped_open(scope, &self.path)?;
            self.check_stamp(&file)?;
            return Ok(file);
        }
        self.check()?;
        let file = File::open(&self.path).map_err(|_| LoadError::Io)?;
        self.check_stamp(&file)?;
        Ok(file)
    }
    fn check_stamp(&self, file: &File) -> Result<(), LoadError> {
        if Stamp::new(&file.metadata().map_err(|_| LoadError::Io)?) != self.stamp {
            return Err(LoadError::Changed);
        }
        Ok(())
    }
    pub(super) fn reader(
        &self,
        cancelled: &impl Fn() -> bool,
    ) -> Result<Box<dyn Read + Send>, LoadError> {
        match &self.encoding {
            Encoding::Paired { data, .. } => data.reader(cancelled),
            Encoding::Stored | Encoding::Deflated => {
                let mut file = self.open()?;
                file.seek(SeekFrom::Start(self.offset))
                    .map_err(|_| LoadError::Io)?;
                let input =
                    std::io::BufReader::with_capacity(64 * 1024, file.take(self.compressed));
                Ok(if matches!(self.encoding, Encoding::Stored) {
                    Box::new(input)
                } else {
                    Box::new(flate2::read::DeflateDecoder::new(input))
                })
            }
            Encoding::Gzip { container } => Ok(Box::new(flate2::read::MultiGzDecoder::new(
                container.reader(cancelled)?,
            ))),
            Encoding::Dataset { container, start } => {
                let mut input = container.reader(cancelled)?;
                let mut prefix = vec![0; usize::try_from(*start).map_err(|_| LoadError::Limit)?];
                read_exact(&mut input, &mut prefix, cancelled)?;
                Ok(Box::new(
                    std::io::Cursor::new(prefix).chain(flate2::read::DeflateDecoder::new(input)),
                ))
            }
        }
    }
    /// Keeps nested ZIP/dataset decompression streaming; no extracted pixel files.
    pub fn inflate_dataset(
        self,
        start: u64,
        limit: u64,
        cancelled: &impl Fn() -> bool,
        progress: impl FnMut(u64),
    ) -> Result<Self, LoadError> {
        let output = Self {
            id: self.id,
            path: self.path.clone(),
            scope: self.scope.clone(),
            stamp: self.stamp.clone(),
            offset: 0,
            length: 0,
            compressed: 0,
            encoding: Encoding::Dataset {
                container: Box::new(self),
                start,
            },
        };
        output.scan(limit, cancelled, progress)
    }
    pub fn inflate_gzip(
        self,
        limit: u64,
        cancelled: &impl Fn() -> bool,
        progress: impl FnMut(u64),
    ) -> Result<Self, LoadError> {
        let output = Self {
            id: self.id,
            path: self.path.clone(),
            scope: self.scope.clone(),
            stamp: self.stamp.clone(),
            offset: 0,
            length: 0,
            compressed: 0,
            encoding: Encoding::Gzip {
                container: Box::new(self),
            },
        };
        output.scan(limit, cancelled, progress)
    }
    fn scan(
        mut self,
        limit: u64,
        cancelled: &impl Fn() -> bool,
        mut progress: impl FnMut(u64),
    ) -> Result<Self, LoadError> {
        let mut reader = self.reader(cancelled)?;
        let mut scratch = [0; 64 * 1024];
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
            self.length = self.length.checked_add(n as u64).ok_or(LoadError::Limit)?;
            if self.length > limit {
                return Err(LoadError::Limit);
            }
            progress(n as u64);
        }
        self.check()?;
        Ok(self)
    }
    pub fn range(
        &self,
        offset: u64,
        length: usize,
        state: &mut ReadState,
        cancelled: &impl Fn() -> bool,
    ) -> Result<Vec<u8>, LoadError> {
        if offset
            .checked_add(length as u64)
            .is_none_or(|end| end > self.length)
        {
            return Err(LoadError::Unsupported);
        }
        if cancelled() {
            return Err(LoadError::Cancelled);
        }
        self.check()?;
        let result = (|| {
            let mut output = Vec::new();
            match &self.encoding {
                Encoding::Paired { data, .. } => {
                    output = data.range(offset, length, state, cancelled)?;
                }
                Encoding::Stored => {
                    output.resize(length, 0);
                    let mut file = self.open()?;
                    file.seek(SeekFrom::Start(
                        self.offset.checked_add(offset).ok_or(LoadError::Limit)?,
                    ))
                    .map_err(|_| LoadError::Io)?;
                    read_exact(&mut file, &mut output, cancelled)?;
                }
                Encoding::Deflated | Encoding::Dataset { .. } | Encoding::Gzip { .. } => {
                    output.resize(length, 0);
                    if !state
                        .stream
                        .as_ref()
                        .is_some_and(|s| s.source == self.id && s.position <= offset)
                    {
                        let reader = self.reader(cancelled)?;
                        state.stream = Some(Stream {
                            source: self.id,
                            position: 0,
                            reader: Box::new(reader),
                        });
                    }
                    let stream = state.stream.as_mut().ok_or(LoadError::Io)?;
                    let mut scratch = [0u8; 64 * 1024];
                    while stream.position < offset {
                        let count = (offset - stream.position).min(scratch.len() as u64) as usize;
                        read_exact(&mut stream.reader, &mut scratch[..count], cancelled)?;
                        stream.position += count as u64;
                    }
                    read_exact(&mut stream.reader, &mut output, cancelled)?;
                    stream.position += length as u64;
                }
            }
            self.check()?;
            Ok(output)
        })();
        if result.is_err() {
            state.stream = None;
        }
        result
    }
}
fn read_exact(
    reader: &mut impl Read,
    output: &mut [u8],
    cancelled: &impl Fn() -> bool,
) -> Result<(), LoadError> {
    for chunk in output.chunks_mut(64 * 1024) {
        if cancelled() {
            return Err(LoadError::Cancelled);
        }
        reader.read_exact(chunk).map_err(|_| LoadError::Io)?;
    }
    Ok(())
}
