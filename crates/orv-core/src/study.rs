// SPDX-License-Identifier: GPL-3.0-or-later
use crate::WindowLevel;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// The NIfTI reference may be scanner, aligned, Talairach, MNI or another template.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NiftiMethod {
    Qform,
    Sform,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NiftiTransform {
    pub method: NiftiMethod,
    pub code: u16,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Geometry {
    /// Present for NIfTI; coordinates must not be presumed to be scanner/patient space.
    pub nifti_transform: Option<NiftiTransform>,
    /// Used locally to prevent combining different patient coordinate references.
    pub frame_of_reference: Option<String>,
    /// DICOM LPS millimetres. First direction is increasing column index.
    pub position: [f64; 3],
    pub orientation: [f64; 6],
    /// DICOM order: row spacing, column spacing (mm).
    pub spacing: [f64; 2],
}

impl Geometry {
    pub fn normal(&self) -> [f64; 3] {
        let o = self.orientation;
        [
            o[1] * o[5] - o[2] * o[4],
            o[2] * o[3] - o[0] * o[5],
            o[0] * o[4] - o[1] * o[3],
        ]
    }
    pub fn location(&self) -> f64 {
        self.normal()
            .iter()
            .zip(self.position)
            .map(|(a, b)| a * b)
            .sum()
    }
}

#[derive(Clone, Debug)]
pub struct Frame {
    pub slab: Option<crate::slab::Slab>,
    pub scalar: Option<Arc<crate::scalar::ScalarData>>,
    /// Optional scalar source mask: 0 is DICOM padding, 1 is a sample. Derived
    /// scalar no-data uses NaN; color validity uses RGBA alpha. No mask means
    /// every finite scalar source value is present.
    pub validity: Option<Arc<[u8]>>,
    pub color: Option<Arc<crate::color::ColorData>>,
    /// Explicit numeric subvolume and complex presentation, when loaded from NIfTI.
    pub nifti: Option<crate::nifti_view::NiftiView>,
    pub width: u32,
    pub height: u32,
    /// Rescaled modality values. No display window has been baked into them.
    pub pixels: Arc<[f32]>,
    /// sRGB-encoded display bytes for color images; pixels is empty. Alpha is
    /// 255 for valid samples and 0 for absent pixels, including palette padding.
    /// ICC-to-sRGB presentation is described by color.info; monitor calibration
    /// and scalar modality values are not implied.
    pub rgba: Option<Arc<[u8]>>,
    pub geometry: Option<Geometry>,
    pub instance: i32,
    pub frame_index: u32,
    pub monochrome1: bool,
    pub default_window: WindowLevel,
}

impl Frame {
    pub fn supports_window(&self, window: WindowLevel) -> bool {
        window.is_valid()
            && self.scalar.as_ref().map_or_else(
                || !matches!(window.function, crate::VoiFunction::Lookup { .. }),
                |scalar| scalar.supports(window),
            )
    }
    pub fn normalize(&self, window: WindowLevel, value: f32) -> Option<f32> {
        if self.rgba.is_some() || !value.is_finite() || !self.supports_window(window) {
            return None;
        }
        self.scalar.as_ref().map_or_else(
            || Some(window.normalize(value)),
            |scalar| scalar.normalize(window, value),
        )
    }
    pub fn has_sample(&self, index: usize) -> bool {
        if self
            .validity
            .as_ref()
            .is_some_and(|mask| mask.get(index) != Some(&1))
        {
            return false;
        }
        if let Some(rgba) = &self.rgba {
            index
                .checked_mul(4)
                .and_then(|i| i.checked_add(3))
                .and_then(|i| rgba.get(i))
                .is_some_and(|alpha| *alpha == 255)
        } else {
            self.pixels
                .get(index)
                .is_some_and(|value| value.is_finite())
        }
    }
    pub fn byte_len(&self) -> u64 {
        self.pixels.len() as u64 * 4
            + self.validity.as_ref().map_or(0, |v| v.len() as u64)
            + self.rgba.as_ref().map_or(0, |v| v.len() as u64)
            + self.color.as_ref().map_or(0, |v| v.rgb.len() as u64 * 2)
    }
    pub fn valid_dimensions(&self) -> bool {
        let count = u64::from(self.width) * u64::from(self.height);
        self.width > 0
            && self.slab.is_none_or(|slab| {
                slab.validate().is_ok()
                    && self.rgba.is_none()
                    && self.geometry.as_ref().is_some_and(|g| g.validate().is_ok())
            })
            && self.height > 0
            && (self.scalar.is_none() || self.rgba.is_none())
            && self.validity.as_ref().is_none_or(|mask| {
                self.rgba.is_none()
                    && mask.len() as u64 == count
                    && mask.iter().all(|v| matches!(v, 0 | 1))
            })
            && self.color.as_ref().is_none_or(|data| {
                self.rgba.is_some()
                    && (1..=16).contains(&data.info.bits)
                    && Some(data.rgb.len() as u64) == count.checked_mul(3)
                    && data.rgb.iter().all(|v| *v <= data.info.maximum())
            })
            && match &self.rgba {
                Some(bytes) => {
                    self.pixels.is_empty()
                        && Some(bytes.len() as u64) == count.checked_mul(4)
                        && bytes
                            .as_chunks::<4>()
                            .0
                            .iter()
                            .all(|p| matches!(p[3], 0 | 255))
                }
                None => self.pixels.len() as u64 == count,
            }
    }
    pub fn aspect_ratio(&self) -> f32 {
        let spacing = self
            .geometry
            .as_ref()
            .map(|g| g.spacing)
            .unwrap_or([1.0, 1.0]);
        self.width as f32 * spacing[1] as f32 / (self.height as f32 * spacing[0] as f32)
    }
}

#[derive(Clone, Debug)]
pub struct Series {
    pub(crate) acquisition: Option<[u8; 32]>,
    pub(crate) part: u32,
    pub(crate) volume: Arc<std::sync::Mutex<std::sync::Weak<crate::volume::Volume>>>,
    /// Internal grouping key. Never exported/displayed by default.
    pub(crate) key: String,
    pub modality: &'static str,
    pub number: i32,
    pub frames: Vec<Arc<Frame>>,
    /// Indexed local backing; decoded pixels are acquired on cancellable workers.
    pub source: Option<Arc<crate::index::IndexedSeries>>,
}

impl Series {
    pub fn volume(
        &self,
        cancelled: impl Fn() -> bool,
    ) -> Result<Arc<crate::volume::Volume>, String> {
        if let Some(source) = &self.source {
            source.volume(cancelled)
        } else {
            let mut cache = loop {
                if cancelled() {
                    return Err("Cancelled".into());
                }
                match self.volume.try_lock() {
                    Ok(cache) => break cache,
                    Err(std::sync::TryLockError::Poisoned(_)) => {
                        return Err("Volume preparation unavailable".into());
                    }
                    Err(std::sync::TryLockError::WouldBlock) => {
                        std::thread::sleep(std::time::Duration::from_millis(1))
                    }
                }
            };
            if let Some(volume) = cache.upgrade()
                && volume.source_frames().len() == self.frames.len()
                && volume
                    .source_frames()
                    .iter()
                    .zip(&self.frames)
                    .all(|(a, b)| Arc::ptr_eq(a, b))
            {
                return Ok(volume);
            }
            let volume = crate::volume::Volume::prepare(self.frames.clone(), cancelled)?;
            *cache = Arc::downgrade(&volume);
            Ok(volume)
        }
    }
    pub fn frame_count(&self) -> usize {
        self.source.as_ref().map_or(self.frames.len(), |s| s.len())
    }
    pub fn supports_window(&self, index: usize, window: WindowLevel) -> bool {
        self.source.as_ref().map_or_else(
            || {
                self.frames
                    .get(index)
                    .is_some_and(|frame| frame.supports_window(window))
            },
            |source| source.supports_window(index, window),
        )
    }
    pub fn read_frame(
        &self,
        index: usize,
        cancelled: impl Fn() -> bool,
    ) -> Result<Arc<Frame>, crate::load::LoadError> {
        if let Some(source) = &self.source {
            source.read_frame(index, cancelled)
        } else if cancelled() {
            Err(crate::load::LoadError::Cancelled)
        } else {
            self.frames
                .get(index)
                .cloned()
                .ok_or(crate::load::LoadError::Unsupported)
        }
    }
    /// Verify against backing files instead of a previously decoded cache entry.
    pub fn read_fresh_frame(
        &self,
        index: usize,
        cancelled: impl Fn() -> bool,
    ) -> Result<Arc<Frame>, crate::load::LoadError> {
        if let Some(source) = &self.source {
            source.read_fresh_frame(index, cancelled)
        } else {
            self.read_frame(index, cancelled)
        }
    }
    pub fn volume_frames(
        &self,
        cancelled: impl Fn() -> bool,
    ) -> Result<Vec<Arc<Frame>>, crate::load::LoadError> {
        if let Some(source) = &self.source {
            source.materialize(cancelled)
        } else {
            Ok(self.frames.clone())
        }
    }

    pub fn label(&self) -> String {
        if let Some(view) = self.source.as_ref().and_then(|source| source.nifti_view()) {
            return view.label();
        }
        let base = format!("{} · Series {}", self.modality, self.number);
        if self.part > 0 {
            format!("{base} · Acquisition {}", self.part)
        } else {
            base
        }
    }
    pub fn sort(&mut self) {
        // Use one common reference normal; MPR must separately validate geometry.
        let normal = self
            .frames
            .first()
            .and_then(|f| f.geometry.as_ref())
            .map(Geometry::normal);
        if let Some(normal) = normal.filter(|_| self.frames.iter().all(|f| f.geometry.is_some())) {
            let location = |f: &Frame| {
                f.geometry
                    .as_ref()
                    .map(|g| {
                        g.position
                            .iter()
                            .zip(normal)
                            .map(|(a, b)| a * b)
                            .sum::<f64>()
                    })
                    .unwrap_or(0.0)
            };
            self.frames.sort_by(|a, b| {
                location(a)
                    .total_cmp(&location(b))
                    .then(a.instance.cmp(&b.instance))
                    .then(a.frame_index.cmp(&b.frame_index))
            });
        } else {
            self.frames.sort_by_key(|f| (f.instance, f.frame_index));
        }
    }
}

#[derive(Debug, Default)]
pub struct Study {
    pub series: Vec<Series>,
    pub skipped_files: usize,
    pub synthetic: bool,
}

impl Study {
    pub fn frame_count(&self) -> usize {
        self.series.iter().map(Series::frame_count).sum()
    }
}

/// Generated scalar phantom for UI/render checks; never presented as patient data.
pub fn demo_study() -> Study {
    let mut frames = Vec::new();
    for z in 0..48 {
        let mut pixels = Vec::with_capacity(256 * 256);
        for y in 0..256 {
            for x in 0..256 {
                let a = (x as f32 - 127.5) / 105.0;
                let b = (y as f32 - 127.5) / 88.0;
                let r = a * a + b * b;
                let slice = ((z as f32 - 23.5) / 30.0).powi(2);
                let value = if r + slice > 1.0 {
                    -1000.0
                } else if r + slice > 0.88 {
                    800.0
                } else if (a + 0.3).powi(2) + (b * 1.4).powi(2) < 0.12
                    || (a - 0.3).powi(2) + (b * 1.4).powi(2) < 0.12
                {
                    -700.0
                } else {
                    35.0 + (a * 12.0).sin() * 15.0 + z as f32
                };
                pixels.push(value);
            }
        }
        frames.push(Arc::new(Frame {
            slab: None,
            scalar: None,
            validity: None,
            color: None,
            nifti: None,
            rgba: None,
            width: 256,
            height: 256,
            pixels: pixels.into(),
            geometry: Some(Geometry {
                nifti_transform: None,
                frame_of_reference: Some("2.25.3141592653589793238".into()),
                position: [0.0, 0.0, z as f64 * 2.0],
                orientation: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
                spacing: [1.0, 1.0],
            }),
            instance: z,
            frame_index: 0,
            monochrome1: false,
            default_window: WindowLevel::SOFT_TISSUE,
        }));
    }
    Study {
        series: vec![Series {
            acquisition: None,
            part: 0,
            volume: Default::default(),
            key: "synthetic".into(),
            source: None,
            modality: "DEMO",
            number: 1,
            frames,
        }],
        synthetic: true,
        skipped_files: 0,
    }
}
