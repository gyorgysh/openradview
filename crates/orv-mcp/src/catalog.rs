// SPDX-License-Identifier: GPL-3.0-or-later
//! Approved selected sources. Tools cannot supply paths or browse other files.
use orv_core::{
    Plane,
    scene::{Reconstruction, SceneState, fingerprint_series, frame_pixel_hash},
    study::{Frame, Series},
    volume::Volume,
};
use serde::Serialize;
use std::sync::{Arc, OnceLock};

#[derive(Clone, Debug, Serialize)]
pub struct SeriesDescription {
    pub series_index: u32,
    pub source_sha256: String,
    pub modality: &'static str,
    pub image_count: usize,
}

pub struct PreparedSeries {
    description: SeriesDescription,
    source: Arc<Series>,
    volume: OnceLock<Arc<Volume>>,
    frame_hashes: Option<Vec<[u8; 32]>>,
}

pub enum Resolved {
    Image(Arc<Frame>),
    Volume(Arc<Volume>),
}
impl Resolved {
    pub fn image(&self) -> Result<&Frame, &'static str> {
        match self {
            Self::Image(frame) => Ok(frame),
            Self::Volume(_) => Err(
                "Projected volume views have no physical image plane; choose a slice or oblique plane",
            ),
        }
    }
}

impl PreparedSeries {
    /// Hashing can be expensive. Call on a cancellable worker before consent.
    pub fn new(
        series_index: u32,
        source: Arc<Series>,
        cancelled: impl Fn() -> bool,
    ) -> Result<Self, &'static str> {
        let fingerprint = fingerprint_series(&source, &cancelled)?;
        let description = SeriesDescription {
            series_index,
            source_sha256: fingerprint.source_sha256,
            modality: source.modality,
            image_count: source.frame_count(),
        };
        Ok(Self {
            description,
            source,
            volume: OnceLock::new(),
            frame_hashes: fingerprint.frame_sha256,
        })
    }
    pub fn description(&self) -> &SeriesDescription {
        &self.description
    }

    pub fn check_scene(&self, scene: &SceneState) -> Result<(), &'static str> {
        scene.validate()?;
        if scene.series_index != self.description.series_index
            || scene.source_sha256 != self.description.source_sha256
        {
            return Err("The scene is outside the approved series");
        }
        if scene.reconstruction.is_none()
            && scene.view.plane == Plane::Acquired
            && scene.view.slice as usize >= self.source.frame_count()
        {
            return Err("The image index is outside the approved series");
        }
        Ok(())
    }

    pub fn resolve_scene(
        &self,
        scene: &SceneState,
        cancelled: impl Fn() -> bool,
    ) -> Result<Resolved, &'static str> {
        self.check_scene(scene)?;
        if matches!(scene.reconstruction, Some(Reconstruction::Volume(_))) {
            self.prepared_volume(&cancelled).map(Resolved::Volume)
        } else {
            self.resolve(scene, cancelled).map(Resolved::Image)
        }
    }

    fn prepared_volume(&self, cancelled: &impl Fn() -> bool) -> Result<Arc<Volume>, &'static str> {
        if cancelled() {
            return Err("Cancelled");
        }
        if let Some(volume) = self.volume.get() {
            return Ok(volume.clone());
        }
        let volume = self.source.volume(cancelled).map_err(|_| "The selected series exceeds the shared volume budget or has unsupported geometry; use 2D views")?;
        for (index, frame) in volume.source_frames().iter().enumerate() {
            self.verify_frame(index, frame, cancelled)?;
        }
        if cancelled() {
            return Err("Cancelled");
        }
        let _ = self.volume.set(volume.clone());
        Ok(volume)
    }
    fn verify_frame(
        &self,
        index: usize,
        frame: &Frame,
        cancelled: &impl Fn() -> bool,
    ) -> Result<(), &'static str> {
        if let Some(expected) = &self.frame_hashes
            && expected.get(index) != Some(&frame_pixel_hash(frame, cancelled)?)
        {
            return Err(
                "Source pixels changed since review; reopen the study and approve access again",
            );
        }
        Ok(())
    }

    pub fn resolve(
        &self,
        scene: &SceneState,
        cancelled: impl Fn() -> bool,
    ) -> Result<Arc<Frame>, &'static str> {
        self.check_scene(scene)?;
        if cancelled() {
            return Err("Cancelled");
        }
        match &scene.reconstruction {
            Some(Reconstruction::Volume(_)) => {
                return Err("Volume projections require volume resolution");
            }
            Some(Reconstruction::Oblique(plane)) => {
                return self
                    .prepared_volume(&cancelled)?
                    .reformat_oblique(plane, scene.reformat_side, cancelled)
                    .map_err(|_| "The oblique reformat could not be completed");
            }
            None => (),
        }
        if scene.view.plane == Plane::Acquired {
            let index = scene.view.slice as usize;
            let frame = self
                .source
                .read_frame(index, &cancelled)
                .map_err(|_| "The approved source changed, was cancelled or is unavailable")?;
            self.verify_frame(index, &frame, &cancelled)?;
            if frame.rgba.is_some() && scene.view.window != frame.default_window {
                return Err(
                    "Window/level applies only to scalar images; RGB color uses its original presentation",
                );
            }
            return Ok(frame);
        }
        let volume = self.prepared_volume(&cancelled)?;
        let point = scene
            .crosshair_mm
            .ok_or("A physical MPR position is required")?;
        let clamped = volume.clamp_world(point).ok_or("Invalid MPR position")?;
        // Never silently substitute a different physical point in a manifest.
        if point.iter().zip(clamped).any(|(a, b)| (a - b).abs() > 1e-6) {
            return Err("MPR position lies outside the volume");
        }
        volume
            .reformat(scene.view.plane, point, scene.reformat_side, cancelled)
            .map_err(|_| "The reformat could not be completed")
    }
}
