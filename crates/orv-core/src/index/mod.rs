// SPDX-License-Identifier: GPL-3.0-or-later
//! Metadata-first local study index with ZIP64 and a shared bounded decoded cache.
mod decode;
mod encapsulated;
mod header;
mod import;
mod nifti;
mod nifti_input;
mod pairs;
mod readers;
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

pub use crate::resources::{MAX_CACHE_BYTES, MAX_VOLUME_BYTES};

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
    _reservation: crate::resources::Reservation,
}
struct Cache {
    entries: VecDeque<CacheEntry>,
    bytes: u64,
    readers: Arc<readers::Readers>,
    budget: Arc<MemoryBudget>,
}
impl Default for Cache {
    fn default() -> Self {
        Self {
            entries: VecDeque::new(),
            bytes: 0,
            readers: Arc::new(readers::Readers::default()),
            budget: MemoryBudget::decoded_cache(),
        }
    }
}
impl Cache {
    fn evict(&mut self) -> bool {
        let Some(old) = self.entries.pop_front() else {
            return false;
        };
        self.bytes = self.bytes.saturating_sub(old.image.byte_len());
        true
    }
    fn trim(&mut self) {
        while self.budget.used() > self.budget.limit() && self.evict() {}
    }
    fn get(&mut self, source: u64, frame: u32) -> Option<Arc<Frame>> {
        self.trim();
        let position = self
            .entries
            .iter()
            .position(|entry| entry.source == source && entry.frame == frame)?;
        let entry = self.entries.remove(position)?;
        let image = entry.image.clone();
        self.entries.push_back(entry);
        Some(image)
    }
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
        crate::resources::refresh_memory_policy();
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
        let reservation = self.budget.reserve(self.decoded_bytes()).ok_or_else(|| format!(
            "The active volumes exceed the current shared {} MiB memory allowance. Close a volume view or use indexed 2D browsing.",
            self.budget.limit() / (1024 * 1024)
        ))?;
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
    pub(crate) fn navigation_frame(&self, index: usize) -> Option<crate::navigation::SpatialFrame> {
        let entry = self.frames.get(index)?;
        let geometry = match &entry.header.format {
            header::Format::Nifti(nifti) => nifti.frame_geometry(&entry.header, entry.frame),
            header::Format::Dicom(_) => entry.header.geometry_for(entry.frame).cloned(),
        };
        Some(crate::navigation::SpatialFrame {
            size: [entry.header.width, entry.header.height],
            geometry,
        })
    }
    pub fn original_window(&self, index: usize) -> Option<crate::WindowLevel> {
        let entry = self.frames.get(index)?;
        let header = &entry.header;
        if let header::Format::Nifti(nifti) = &header.format {
            return nifti.window;
        }
        let object = header.dicom().ok()?;
        if crate::dicom_color::layout(object).ok()?.is_some() {
            return Some(crate::WindowLevel {
                function: Default::default(),
                width: 256.,
                center: 128.,
            });
        }
        header.scalar.get(entry.frame)?.choices().original()
    }
    pub fn supports_window(&self, index: usize, window: crate::WindowLevel) -> bool {
        let Some(entry) = self.frames.get(index) else {
            return false;
        };
        window.is_valid()
            && entry.header.scalar.get(entry.frame).map_or_else(
                || !matches!(window.function, crate::VoiFunction::Lookup { .. }),
                |data| data.supports(window),
            )
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
        crate::resources::refresh_memory_policy();
        let entry = self.frames.get(index).ok_or(LoadError::Unsupported)?;
        let readers = {
            let mut cache = self.lock_cache(&cancelled)?;
            if let Some(image) = cache.get(entry.source.id, entry.frame) {
                return Ok(image);
            }
            cache.readers.clone()
        };
        let mut reader = readers.acquire(entry.source.id, &cancelled)?;
        // Another worker may have completed this frame while a reader was busy.
        if let Some(image) = self
            .lock_cache(&cancelled)?
            .get(entry.source.id, entry.frame)
        {
            return Ok(image);
        }
        let image = decode::decode(
            &entry.source,
            &entry.header,
            entry.frame,
            &mut reader,
            &cancelled,
        )?;
        let mut cache = self.lock_cache(&cancelled)?;
        if let Some(image) = cache.get(entry.source.id, entry.frame) {
            return Ok(image);
        }
        let size = image.byte_len();
        let mut reservation = None;
        if size <= cache.budget.limit() {
            loop {
                reservation = cache.budget.reserve(size);
                if reservation.is_some() || !cache.evict() {
                    break;
                }
            }
        }
        if let Some(reservation) = reservation {
            cache.bytes += size;
            cache.entries.push_back(CacheEntry {
                source: entry.source.id,
                frame: entry.frame,
                image: image.clone(),
                _reservation: reservation,
            });
        }
        Ok(image)
    }
    /// Worker-only read that bypasses decoded pixels retained in the cache.
    pub fn read_fresh_frame(
        &self,
        index: usize,
        cancelled: impl Fn() -> bool,
    ) -> Result<Arc<Frame>, LoadError> {
        let entry = self.frames.get(index).ok_or(LoadError::Unsupported)?;
        let readers = self.lock_cache(&cancelled)?.readers.clone();
        let mut reader = readers.acquire(entry.source.id, &cancelled)?;
        decode::decode(
            &entry.source,
            &entry.header,
            entry.frame,
            &mut reader,
            &cancelled,
        )
    }
    fn lock_cache(
        &self,
        cancelled: &impl Fn() -> bool,
    ) -> Result<std::sync::MutexGuard<'_, Cache>, LoadError> {
        loop {
            if cancelled() {
                return Err(LoadError::Cancelled);
            }
            match self.cache.try_lock() {
                Ok(cache) => return Ok(cache),
                Err(std::sync::TryLockError::Poisoned(_)) => return Err(LoadError::Io),
                Err(std::sync::TryLockError::WouldBlock) => {
                    std::thread::sleep(Duration::from_millis(1))
                }
            }
        }
    }
    pub fn materialize(&self, cancelled: impl Fn() -> bool) -> Result<Vec<Arc<Frame>>, LoadError> {
        if self.decoded_bytes() > self.budget.limit().min(MAX_VOLUME_BYTES) {
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
            let readers = cache.readers.clone();
            drop(cache);
            readers.clear()
        } else {
            false
        }
    }
    fn sort(&mut self) {
        let normal = self
            .frames
            .first()
            .and_then(|f| f.header.geometry_for(f.frame))
            .map(|g| g.normal());
        if let Some(normal) = normal.filter(|_| {
            self.frames
                .iter()
                .all(|f| f.header.geometry_for(f.frame).is_some())
        }) {
            let location = |frame: &IndexedFrame| {
                frame.header.geometry_for(frame.frame).map_or(0., |g| {
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
