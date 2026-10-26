// SPDX-License-Identifier: GPL-3.0-or-later
//! Metadata-first local study index with ZIP64 and a shared bounded decoded cache.
mod decode;
mod encapsulated;
mod header;
mod import;
mod nifti;
mod nifti_input;
mod pairs;
pub use import::index_paths;
mod source;
#[cfg(test)]
mod tests;

use crate::{load::LoadError, resources::MemoryBudget, study::Frame, volume::Volume};
use header::Header;
use source::{ReadState, Source};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::Duration,
};

pub const CACHE_BYTES: u64 = 128 * 1024 * 1024;
pub use crate::resources::VOLUME_BYTES;

#[derive(Clone, Copy, Default)]
pub struct Progress {
    pub files: usize,
    pub checked_bytes: u64,
}

struct IndexedFrame {
    source: Arc<Source>,
    header: Arc<Header>,
    frame: u32,
}
struct CacheEntry {
    source: u64,
    frame: u32,
    image: Arc<Frame>,
}
#[derive(Default)]
struct Cache {
    entries: VecDeque<CacheEntry>,
    bytes: u64,
    reader: ReadState,
}

/// Does not expose source paths, raw headers or arbitrary file operations.
pub struct IndexedSeries {
    frames: Vec<IndexedFrame>,
    cache: Arc<Mutex<Cache>>,
    budget: Arc<MemoryBudget>,
    volume: Mutex<std::sync::Weak<Volume>>,
}
impl std::fmt::Debug for IndexedSeries {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IndexedSeries")
            .field("image_count", &self.frames.len())
            .finish_non_exhaustive()
    }
}
impl IndexedSeries {
    /// Worker-only shared preparation. A weak cache never keeps a closed view alive.
    pub fn volume(&self, cancelled: impl Fn() -> bool) -> Result<Arc<Volume>, String> {
        let mut cached = loop {
            if cancelled() {
                return Err(LoadError::Cancelled.to_string());
            }
            match self.volume.try_lock() {
                Ok(guard) => break guard,
                Err(std::sync::TryLockError::Poisoned(_)) => return Err(LoadError::Io.to_string()),
                Err(std::sync::TryLockError::WouldBlock) => {
                    std::thread::sleep(Duration::from_millis(1))
                }
            }
        };
        if let Some(volume) = cached.upgrade() {
            return Ok(volume);
        }
        let reservation = self.budget.reserve(self.decoded_bytes()).ok_or(
            "The active volumes exceed the shared 1.5 GiB budget. Close a volume view or use indexed 2D browsing."
        )?;
        let first = self
            .read_frame(0, &cancelled)
            .map_err(|error| error.to_string())?;
        first
            .geometry
            .as_ref()
            .ok_or(crate::spatial::GeometryError::Missing.to_string())?
            .validate()
            .map_err(|error| error.to_string())?;
        let frames = self
            .materialize(&cancelled)
            .map_err(|error| error.to_string())?;
        let mut volume =
            Volume::new_cancellable(frames, &cancelled).map_err(|error| error.to_string())?;
        volume.retain_reservation(reservation);
        let volume = Arc::new(volume);
        *cached = Arc::downgrade(&volume);
        Ok(volume)
    }
    pub fn nifti_view(&self) -> Option<&crate::nifti_view::NiftiView> {
        match &self.frames.first()?.header.format {
            header::Format::Nifti(nifti) => Some(&nifti.view),
            _ => None,
        }
    }
    pub fn len(&self) -> usize {
        self.frames.len()
    }
    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }
    pub fn original_window(&self, index: usize) -> Option<crate::WindowLevel> {
        let header = &self.frames.get(index)?.header;
        if let header::Format::Nifti(nifti) = &header.format {
            return nifti.window;
        }
        let object = header.dicom().ok()?;
        if crate::dicom_color::layout(object).ok()?.is_some() {
            return Some(crate::WindowLevel {
                width: 256.,
                center: 128.,
            });
        }
        crate::load::number(object, "WindowWidth")
            .zip(crate::load::number(object, "WindowCenter"))
            .map(|(width, center)| crate::WindowLevel {
                width: width as f32,
                center: center as f32,
            })
            .filter(|w| w.is_valid())
    }
    /// Conservative retained-pixel estimate before decoding. Declared scalar
    /// padding reserves a byte per sample even if no matching code is found.
    pub fn decoded_bytes(&self) -> u64 {
        self.frames
            .iter()
            .map(|f| {
                let sample_bytes = f.header.dicom().map_or(4, |object| {
                    if crate::dicom_color::layout(object).ok().flatten().is_some() {
                        10 // raw u16 RGB plus display RGBA
                    } else if object.element(dicom_core::Tag(0x0028, 0x0120)).is_ok() {
                        5 // f32 scalar plus possible stored-padding validity
                    } else {
                        4
                    }
                });
                u64::from(f.header.width) * u64::from(f.header.height) * sample_bytes
            })
            .sum()
    }
    /// Worker-only: cancellable local I/O and pixel conversion. Never call in UI.
    pub fn read_frame(
        &self,
        index: usize,
        cancelled: impl Fn() -> bool,
    ) -> Result<Arc<Frame>, LoadError> {
        let entry = self.frames.get(index).ok_or(LoadError::Unsupported)?;
        // A worker waiting behind another decoder can still be cancelled promptly.
        let mut cache = loop {
            if cancelled() {
                return Err(LoadError::Cancelled);
            }
            match self.cache.try_lock() {
                Ok(cache) => break cache,
                Err(std::sync::TryLockError::Poisoned(_)) => return Err(LoadError::Io),
                Err(std::sync::TryLockError::WouldBlock) => {
                    std::thread::sleep(Duration::from_millis(1))
                }
            }
        };
        if let Some(position) = cache
            .entries
            .iter()
            .position(|e| e.source == entry.source.id && e.frame == entry.frame)
        {
            let cached = cache.entries.remove(position).ok_or(LoadError::Io)?;
            let image = cached.image.clone();
            cache.entries.push_back(cached);
            return Ok(image);
        }
        let image = decode::decode(
            &entry.source,
            &entry.header,
            entry.frame,
            &mut cache.reader,
            &cancelled,
        )?;
        if cancelled() {
            return Err(LoadError::Cancelled);
        }
        let size = image.byte_len();
        while cache.bytes + size > CACHE_BYTES {
            let Some(old) = cache.entries.pop_front() else {
                break;
            };
            cache.bytes = cache.bytes.saturating_sub(old.image.byte_len());
        }
        if size <= CACHE_BYTES {
            cache.bytes += size;
            cache.entries.push_back(CacheEntry {
                source: entry.source.id,
                frame: entry.frame,
                image: image.clone(),
            });
        }
        Ok(image)
    }
    pub fn materialize(&self, cancelled: impl Fn() -> bool) -> Result<Vec<Arc<Frame>>, LoadError> {
        if self.decoded_bytes() > VOLUME_BYTES {
            return Err(LoadError::Limit);
        }
        (0..self.len())
            .map(|i| self.read_frame(i, &cancelled))
            .collect()
    }
    pub fn cache_bytes(&self) -> Option<u64> {
        self.cache.try_lock().ok().map(|c| c.bytes)
    }
    /// Releases optional cache payloads without waiting for an in-flight worker.
    /// Displayed/granted images retain their own immutable references.
    pub fn discard_cached_frames(&self) -> bool {
        if let Ok(mut cache) = self.cache.try_lock() {
            cache.entries.clear();
            cache.bytes = 0;
            cache.reader.stream = None;
            true
        } else {
            false
        }
    }
    fn sort(&mut self) {
        let normal = self
            .frames
            .first()
            .and_then(|f| f.header.geometry.as_ref())
            .map(|g| g.normal());
        if let Some(normal) =
            normal.filter(|_| self.frames.iter().all(|f| f.header.geometry.is_some()))
        {
            let location = |frame: &IndexedFrame| {
                frame.header.geometry.as_ref().map_or(0., |g| {
                    g.position
                        .iter()
                        .zip(normal)
                        .map(|(a, b)| a * b)
                        .sum::<f64>()
                })
            };
            self.frames.sort_by(|a, b| {
                location(a)
                    .total_cmp(&location(b))
                    .then(a.header.instance.cmp(&b.header.instance))
                    .then(a.frame.cmp(&b.frame))
            });
        } else {
            self.frames.sort_by_key(|f| (f.header.instance, f.frame));
        }
    }
}
